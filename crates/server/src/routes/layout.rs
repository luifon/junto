//! The layout page of the recorder (ADR 0602, ADR 0615). The recorder loads the built page
//! from this server and passes the address of the media server and its room token in the
//! query of the page address. The page is the same for every caller and holds no secret, so
//! the route asks for no credential; it is not part of the public API and not in the
//! OpenAPI document.

use std::path::Path;

use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderName};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::extract::ApiPath;
use crate::state::AppState;

pub const INDEX: &str = "/recording-layout/";
pub const FILE: &str = "/recording-layout/{*path}";

const INDEX_FILE: &str = "index.html";
/// A file of the page that is larger is not served.
const MOST_BYTES: u64 = 16 * 1024 * 1024;
const NO_SNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

/// The media type of a file of the page, by its extension. A file with another extension is
/// not served.
fn media_type(path: &str) -> Option<&'static str> {
    let (_, extension) = path.rsplit_once('.')?;
    Some(match extension {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "woff2" => "font/woff2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "LICENSE" | "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// True for a relative path whose parts are plain file names: letters, digits, `.`, `-` and
/// `_`, and no part that starts with a dot.
fn plain(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
}

/// Opens one file below `directory` for reading. No part of `path` may be a link, and the
/// opened file is a regular file: the checks are made on the opened directories and on the
/// opened file, so a change of the directory during the request does not lead out of it.
#[cfg(unix)]
fn open(directory: &Path, path: &str) -> Option<std::fs::File> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, openat};

    let inside = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let (name, above) = path
        .rsplit_once('/')
        .map_or((path, None), |(above, name)| (name, Some(above)));
    let root = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let mut holder = rustix::fs::open(directory, root, Mode::empty()).ok()?;
    for part in above.into_iter().flat_map(|above| above.split('/')) {
        holder = openat(&holder, part, inside | OFlags::DIRECTORY, Mode::empty()).ok()?;
    }
    let file = openat(&holder, name, inside | OFlags::NONBLOCK, Mode::empty()).ok()?;
    let regular = FileType::from_raw_mode(fstat(&file).ok()?.st_mode) == FileType::RegularFile;
    regular.then(|| std::fs::File::from(file))
}

/// On another system the server serves no file of the page: it has no way there to open a
/// file without following links, as for the files of recordings.
#[cfg(not(unix))]
fn open(_directory: &Path, _path: &str) -> Option<std::fs::File> {
    None
}

/// The bytes of one file of the page, or nothing for a file that is absent, is no regular
/// file, is reached through a link, or has more than [`MOST_BYTES`].
async fn read(directory: &Path, path: &str) -> Option<Vec<u8>> {
    use std::io::Read;

    let (directory, path) = (directory.to_owned(), path.to_owned());
    tokio::task::spawn_blocking(move || {
        let file = open(&directory, &path)?;
        let mut bytes = Vec::new();
        file.take(MOST_BYTES + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() as u64 <= MOST_BYTES).then_some(bytes)
    })
    .await
    .ok()
    .flatten()
}

async fn serve(app: &AppState, path: &str) -> Response {
    let Some(directory) = &app.config.recorder_layout_directory else {
        return ApiError::not_found().into_response();
    };
    let (true, Some(media_type)) = (plain(path), media_type(path)) else {
        return ApiError::not_found().into_response();
    };
    match read(directory, path).await {
        Some(bytes) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, HeaderValue::from_static(media_type)),
                (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
                (NO_SNIFF, HeaderValue::from_static("nosniff")),
            ],
            bytes,
        )
            .into_response(),
        None => ApiError::not_found().into_response(),
    }
}

pub async fn index(State(app): State<AppState>) -> Response {
    serve(&app, INDEX_FILE).await
}

pub async fn file(State(app): State<AppState>, ApiPath(path): ApiPath<String>) -> Response {
    serve(&app, &path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_with_a_known_extension_are_files_of_the_page() {
        for path in [
            "index.html",
            "layout.js",
            "theme.css",
            "atkinson.woff2",
            "livekit-client.umd.js",
            "livekit-client.LICENSE",
            "fonts/atkinson-400.woff2",
        ] {
            assert!(plain(path) && media_type(path).is_some(), "{path}");
        }
        for path in [
            "",
            "..",
            "../index.html",
            "fonts/../index.html",
            "/index.html",
            "fonts//a.js",
            ".env",
            "fonts/.hidden.js",
            "a\\b.js",
            "a b.js",
            "a%2e.js",
            "nul\0.js",
        ] {
            assert!(!plain(path), "{path:?}");
        }
        for path in [
            "index",
            "archive.zip",
            "server.env",
            "index.HTML",
            "key.pem",
        ] {
            assert_eq!(media_type(path), None, "{path}");
        }
        assert_eq!(
            media_type("layout.js"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(media_type("atkinson.woff2"), Some("font/woff2"));
    }
}
