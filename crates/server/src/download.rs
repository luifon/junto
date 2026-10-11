//! The download of a file as the body of a response, whole or as one byte range (ADR 0608).

use std::io::SeekFrom;

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
    let length = file
        .metadata()
        .map_err(|error| ApiError::internal(&error))?
        .len();
    let (status, first, last) = match asked(headers, length) {
        Asked::Whole => (StatusCode::OK, 0, length.saturating_sub(1)),
        Asked::Range { first, last } => (StatusCode::PARTIAL_CONTENT, first, last),
        Asked::Outside => {
            let refusal = ApiError::new(
                ErrorCode::RangeNotSatisfiable,
                "the range lies outside the file",
            );
            let mut response = refusal.into_response();
            let range = format!("bytes */{length}");
            if let Ok(range) = HeaderValue::from_str(&range) {
                response.headers_mut().insert(CONTENT_RANGE, range);
            }
            return Ok(response);
        }
    };
    let count = if length == 0 { 0 } else { last - first + 1 };
    let (sender, receiver) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(2);
    tokio::spawn(async move {
        let _place = place;
        let mut file = tokio::fs::File::from_std(file);
        if let Err(error) = file.seek(SeekFrom::Start(first)).await {
            let _ = sender.send(Err(error)).await;
            return;
        }
        let mut left = count;
        let mut buffer = vec![0u8; CHUNK];
        while left > 0 {
            let want = usize::try_from(left).map_or(CHUNK, |left| left.min(CHUNK));
            let chunk = match file.read(&mut buffer[..want]).await {
                Ok(0) => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
                Ok(read) => {
                    left -= read as u64;
                    Ok(Bytes::copy_from_slice(&buffer[..read]))
                }
                Err(error) => Err(error),
            };
            let failed = chunk.is_err();
            let sent = tokio::time::timeout(stall, sender.send(chunk)).await;
            if failed || !matches!(sent, Ok(Ok(()))) {
                return;
            }
        }
    });
    let body = Body::from_stream(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|chunk| (chunk, receiver)) },
    ));
    let header =
        |text: &str| HeaderValue::from_str(text).map_err(|error| ApiError::internal(&error));
    let mut response = (status, body).into_response();
    let stated = response.headers_mut();
    stated.insert(CONTENT_TYPE, HeaderValue::from_static(described.media_type));
    stated.insert(CONTENT_LENGTH, header(&count.to_string())?);
    stated.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    stated.insert(
        CONTENT_DISPOSITION,
        header(&format!("attachment; filename=\"{}\"", described.file_name))?,
    );
    stated.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    stated.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if status == StatusCode::PARTIAL_CONTENT {
        stated.insert(
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
