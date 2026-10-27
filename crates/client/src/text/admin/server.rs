//! The text of the page of the server itself: its logo (ADR 0101) and the limits of the
//! files of messages (ADR 0408).

use junto_client_core::profile::InvalidAvatar;

pub const LOGO: &str = "Logo";
pub const LOGO_HELP: &str = "The logo is public: a person can see it before joining the server. Choose a PNG, JPEG, \
     WebP or GIF file of at most 10 MiB, then the square that the logo shows.";
pub const NO_LOGO: &str = "This server has no logo.";
pub const LOGO_NOT_SHOWN: &str = "This server has a logo. This window does not hold its image.";
pub const CHOOSE_IMAGE: &str = "Choose image…";
pub const REMOVE_LOGO: &str = "Remove logo";
pub const REMOVE_LOGO_QUESTION: &str = "Remove the logo of this server?";
pub const LOGO_NEEDS: &str = "Only an administrator can change the logo.";

pub const ATTACHMENTS: &str = "Files of messages";
pub const ATTACHMENTS_HELP: &str = "The server refuses a larger file and a message with more files. A \
     lower largest file applies to the next upload, and fewer files to the next message. \
     Files that were sent stay.";
pub const LARGEST_FILE: &str = "Largest file";
pub const FILES_PER_MESSAGE: &str = "Files per message";
pub const MEGABYTES_FIELD: &str = "Megabytes";
pub const FILES_FIELD: &str = "Files";
pub const LARGEST_FILE_RANGE: &str =
    "A whole number of megabytes from 1 to 1,073. One megabyte is 1,000,000 bytes.";
pub const FILES_RANGE: &str = "A whole number of files from 1 to 100.";
pub const LIMITS_REFUSED: &str = "The server did not accept the limits. A file has at most \
     1,073,741,824 bytes, and a message has 1 to 100 files.";

/// The number of files that a message may hold.
pub fn files(count: u32) -> String {
    match count {
        1 => "1 file".to_owned(),
        count => format!("{count} files"),
    }
}

// Why a file is not sent as the logo, and why the server did not take it.
pub const LOGO_NOT_READ: &str =
    "The file could not be read. Check that the file exists and that you may open it.";
pub const LOGO_TOO_LARGE: &str = "The server did not take the image: it is larger than this server takes.";
pub const LOGO_UNSUPPORTED: &str = "The server did not take the image. It takes a still PNG, JPEG or WebP image of at most \
     1024 by 1024 pixels.";
pub const LOGO_NOT_ALLOWED: &str =
    "Your account may not do this. Only an administrator can change the logo.";

/// Why a file is not sent as the logo.
pub const fn invalid_logo(invalid: InvalidAvatar) -> &'static str {
    match invalid {
        InvalidAvatar::TooLarge => "The image is larger than 2 MiB. Choose a smaller image.",
        InvalidAvatar::Unsupported => "The file is not a PNG, JPEG or WebP image.",
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::api::{ApiError, Rejection};
    use junto_client_core::protocol::error::ErrorCode;

    use super::*;
    use crate::state::admin::AskKind;
    use crate::text::admin::refusal;

    fn rejected(code: ErrorCode) -> ApiError {
        ApiError::Rejected(Rejection {
            code,
            status: code.http_status(),
            retry_after: None,
            message: String::new(),
            left: None,
        })
    }

    #[test]
    fn a_refusal_of_the_logo_states_the_bound_that_the_image_broke() {
        assert_eq!(
            refusal(AskKind::SetLogo, &rejected(ErrorCode::ImageTooLarge)),
            LOGO_TOO_LARGE
        );
        assert_eq!(
            refusal(AskKind::SetLogo, &rejected(ErrorCode::ImageUnsupported)),
            LOGO_UNSUPPORTED
        );
        for what in [AskKind::SetLogo, AskKind::RemoveLogo] {
            assert_eq!(
                refusal(what, &rejected(ErrorCode::Forbidden)),
                LOGO_NOT_ALLOWED
            );
        }
        // Every other refusal has the sentence of its code.
        assert_eq!(
            refusal(AskKind::SetLogo, &rejected(ErrorCode::RateLimited)),
            crate::text::flow_error(&junto_client_core::flow::FlowError::refused(
                ErrorCode::RateLimited
            ))
        );
    }

    #[test]
    fn a_file_that_is_not_sent_has_its_reason() {
        assert!(invalid_logo(InvalidAvatar::TooLarge).contains("256 KiB"));
        assert!(invalid_logo(InvalidAvatar::Unsupported).contains("PNG, JPEG or WebP"));
    }
}
