//! The download of a file as the body of a response, whole or as one byte range (ADR 0608).

use axum::body::{Body, Bytes};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    RANGE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use junto_protocol::error::ErrorCode;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::OwnedSemaphorePermit;

use crate::error::ApiError;

/// The bytes that the server reads of a file at one time.
const CHUNK: usize = 64 * 1024;

/// The part of a file that a request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Whole,
    /// The bytes from `first` to `last`, both included.
    Range {
        first: u64,
        last: u64,
    },
    /// The range lies outside the file.
    Outside,
}

/// Reads the `Range` header of a request for a file of `length` bytes. One range of bytes
/// is served. A header of another form, and a header with several ranges, is not a range
/// request: the whole file is sent.
pub fn asked(headers: &HeaderMap, length: u64) -> Asked {
    let Some(text) = headers.get(RANGE).and_then(|value| value.to_str().ok()) else {
        return Asked::Whole;
    };
    let Some((first, last)) = text
        .trim()
        .strip_prefix("bytes=")
        .and_then(|range| range.trim().split_once('-'))
    else {
        return Asked::Whole;
    };
    let number = |text: &str| {
        let plain = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        plain.then(|| text.parse::<u64>().ok()).flatten()
    };
    // Each side is a number or empty. Any other text is no single range.
    if [first, last]
        .iter()
        .any(|side| !side.is_empty() && number(side).is_none())
    {
        return Asked::Whole;
    }
    match (
        first.is_empty(),
        last.is_empty(),
        number(first),
        number(last),
    ) {
        // The last bytes of the file.
        (true, false, _, Some(count)) if count > 0 && length > 0 => Asked::Range {
            first: length.saturating_sub(count),
            last: length - 1,
        },
        (true, false, _, Some(_)) => Asked::Outside,
        (false, true, Some(first), _) if first < length => Asked::Range {
            first,
            last: length - 1,
        },
        (false, false, Some(first), Some(last)) if first <= last && first < length => {
            Asked::Range {
                first,
                last: last.min(length - 1),
            }
        }
        (false, _, Some(first), _) if first >= length => Asked::Outside,
        _ => Asked::Whole,
    }
}

/// What a download states about its file.
pub struct Described<'a> {
    pub media_type: &'static str,
    /// The name that a client saves the file under: letters, digits, `.`, `-` and `_`.
    pub file_name: &'a str,
}

/// The response that sends `file`, whole or as the range that `headers` ask for. `place`
/// is held until the body has ended. A client that takes no bytes for `stall` ends the
/// download.
pub async fn send(
    file: std::fs::File,
    headers: &HeaderMap,
    described: Described<'_>,
    place: OwnedSemaphorePermit,
    stall: std::time::Duration,
) -> Result<Response, ApiError> {
    let always = || std::future::ready(true);
    send_while(file, headers, described, place, stall, always).await
}

/// Like [`send`]. `allowed` is asked before each part of the file is read; when it answers
/// false, the download ends without the rest of the file.
pub async fn send_while<A, F>(
    file: std::fs::File,
    headers: &HeaderMap,
    described: Described<'_>,
    place: OwnedSemaphorePermit,
    stall: std::time::Duration,
    allowed: A,
) -> Result<Response, ApiError>
where
    A: FnMut() -> F + Send + 'static,
    F: Future<Output = bool> + Send,
{
    let mut file = tokio::fs::File::from_std(file);
    let length = file
        .metadata()
        .await
        .map_err(|error| ApiError::internal(&error))?
        .len();
    let first = match asked(headers, length) {
        Asked::Range { first, .. } => first,
        _ => 0,
    };
    file.seek(std::io::SeekFrom::Start(first))
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let state = (file, allowed);
    let stream = futures_util::stream::try_unfold(state, |(mut file, mut allowed)| async move {
        if !allowed().await {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the download is no longer permitted",
            ));
        }
        let mut bytes = vec![0; CHUNK];
        let count = file.read(&mut bytes).await?;
        bytes.truncate(count);
        Ok::<_, std::io::Error>((count > 0).then(|| (Bytes::from(bytes), (file, allowed))))
    });
    let mut response = streamed(
        stream,
        length,
        first,
        headers,
        described.media_type,
        place,
        stall,
    )?;
    response.headers_mut().insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", described.file_name))
            .map_err(|error| ApiError::internal(&error))?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(response)
}

/// A bounded producer shared by recording and attachment downloads. The producer owns the
/// capacity guard even when the HTTP body is no longer polled.
pub fn streamed<S, G>(
    stream: S,
    length: u64,
    source_offset: u64,
    headers: &HeaderMap,
    media_type: &str,
    guard: G,
    stall: std::time::Duration,
) -> Result<Response, ApiError>
where
    S: futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static,
    G: Send + 'static,
{
    use futures_util::StreamExt;
    let (status, first, last) = match asked(headers, length) {
        Asked::Whole => (StatusCode::OK, 0, length.saturating_sub(1)),
        Asked::Range { first, last } => (StatusCode::PARTIAL_CONTENT, first, last),
        Asked::Outside => {
            let mut response = ApiError::new(
                ErrorCode::RangeNotSatisfiable,
                "the range lies outside the file",
            )
            .into_response();
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{length}"))
                    .map_err(|error| ApiError::internal(&error))?,
            );
            return Ok(response);
        }
    };
    let count = if length == 0 { 0 } else { last - first + 1 };
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    tokio::spawn(async move {
        let _guard = guard;
        futures_util::pin_mut!(stream);
        let mut skipped = first.saturating_sub(source_offset);
        let mut left = count;
        while left > 0 {
            let next = tokio::time::timeout(stall, stream.next()).await;
            let chunk = match next {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(error))) => {
                    let _ = tokio::time::timeout(stall, sender.send(Err(error))).await;
                    return;
                }
                Ok(None) => {
                    let _ = tokio::time::timeout(
                        stall,
                        sender.send(Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))),
                    )
                    .await;
                    return;
                }
                Err(_) => return,
            };
            if skipped >= chunk.len() as u64 {
                skipped -= chunk.len() as u64;
                continue;
            }
            let start = skipped as usize;
            skipped = 0;
            let end = start + (left.min((chunk.len() - start) as u64) as usize);
            left -= (end - start) as u64;
            if !matches!(
                tokio::time::timeout(stall, sender.send(Ok(chunk.slice(start..end)))).await,
                Ok(Ok(()))
            ) {
                return;
            }
        }
    });
    let body = Body::from_stream(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|chunk| (chunk, receiver)) },
    ));
    let mut response = (status, body).into_response();
    let values = response.headers_mut();
    let header =
        |text: &str| HeaderValue::from_str(text).map_err(|error| ApiError::internal(&error));
    values.insert(CONTENT_TYPE, header(media_type)?);
    values.insert(CONTENT_LENGTH, header(&count.to_string())?);
    values.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    values.insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    values.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    values.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if status == StatusCode::PARTIAL_CONTENT {
        values.insert(
            CONTENT_RANGE,
            header(&format!("bytes {first}-{last}/{length}"))?,
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(range: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_str(range).unwrap());
        headers
    }

    #[tokio::test]
    async fn an_error_behind_a_stalled_body_releases_download_capacity() {
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().acquire_owned().await.unwrap();
        let stall = std::time::Duration::from_millis(20);
        let stream = futures_util::stream::iter([
            Ok(Bytes::from_static(b"a")),
            Ok(Bytes::from_static(b"b")),
            Err(std::io::Error::other("failed disk read")),
        ]);
        let _response = streamed(
            stream,
            3,
            0,
            &HeaderMap::new(),
            "application/octet-stream",
            permit,
            stall,
        )
        .unwrap();
        let _returned = tokio::time::timeout(stall * 100, capacity.acquire())
            .await
            .expect("an error must not hold a stalled download forever")
            .unwrap();
    }

    #[test]
    fn one_range_of_bytes_is_read() {
        let range = |first, last| Asked::Range { first, last };
        assert_eq!(asked(&HeaderMap::new(), 100), Asked::Whole);
        assert_eq!(asked(&with("bytes=0-9"), 100), range(0, 9));
        assert_eq!(asked(&with("bytes=10-"), 100), range(10, 99));
        assert_eq!(asked(&with("bytes=-10"), 100), range(90, 99));
        assert_eq!(asked(&with("bytes=-500"), 100), range(0, 99));
        assert_eq!(asked(&with("bytes=90-500"), 100), range(90, 99));
        assert_eq!(asked(&with("bytes=99-99"), 100), range(99, 99));
        assert_eq!(asked(&with(" bytes=0-0 "), 100), range(0, 0));
    }

    #[test]
    fn a_range_outside_the_file_is_told_and_other_forms_are_not_ranges() {
        assert_eq!(asked(&with("bytes=100-"), 100), Asked::Outside);
        assert_eq!(asked(&with("bytes=100-200"), 100), Asked::Outside);
        assert_eq!(asked(&with("bytes=-0"), 100), Asked::Outside);
        assert_eq!(asked(&with("bytes=0-"), 0), Asked::Outside);
        assert_eq!(asked(&with("bytes=-5"), 0), Asked::Outside);
        for other in [
            "bytes=100-110,0-9",
            "bytes=200-x",
            "bytes=0-9,20-29",
            "bytes=9-0",
            "items=0-9",
            "bytes=a-b",
            "bytes=-",
            "bytes=+1-2",
            "bytes=1",
            "",
        ] {
            assert_eq!(asked(&with(other), 100), Asked::Whole, "{other:?}");
        }
    }
}
