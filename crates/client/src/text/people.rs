//! The text of the pictures of people (ADR 0783): the picture of the profile preset in the
//! settings.

pub const PICTURE: &str = "Picture";
pub const PROFILE_PICTURE: &str = "The picture of your profile";
pub const CHOOSE_PICTURE: &str = "Choose a picture…";
pub const REMOVE_PICTURE: &str = "Remove picture";
pub const SAVING_PICTURE: &str = "Saving…";
pub const PICTURE_HELP: &str = "A new account on a server starts with this picture. Use a PNG, JPEG or WebP file of at most 256 KiB. A server takes a picture of at most 1024 by 1024 pixels.";
pub const NO_PICTURE: &str = "No picture is set. Junto draws a generated picture for you.";
pub const PICTURE_NOT_SHOWN: &str =
    "The picture is stored. This window cannot draw it, and a server may not accept it.";
pub const PICTURE_TOO_LARGE: &str = "The file is larger than 256 KiB. Choose a smaller picture.";
pub const PICTURE_UNSUPPORTED: &str =
    "The file is not a PNG, JPEG or WebP picture. Choose a file in one of these formats.";
pub const PICTURE_UNREADABLE: &str =
    "The file could not be read. Check that the file exists and that you may open it.";

/// The readable alternative of the picture that an account set (ADR 0702).
pub fn picture_of(name: &str) -> String {
    format!("Picture of {name}")
}

/// The line of a server that refused the picture of the preset for the account that this
/// device created there (ADR 0717). The name is text of the server.
pub fn preset_refused(server: &str) -> String {
    format!(
        "{server} did not accept the picture when your account there was created. Your account there has no picture."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_server_is_named_in_its_line() {
        assert_eq!(
            preset_refused("Harbor"),
            "Harbor did not accept the picture when your account there was created. Your \
             account there has no picture."
        );
    }

    #[test]
    fn the_sentences_about_a_picture_have_no_exclamation() {
        for sentence in [
            PICTURE_HELP,
            NO_PICTURE,
            PICTURE_NOT_SHOWN,
            PICTURE_TOO_LARGE,
            PICTURE_UNSUPPORTED,
            PICTURE_UNREADABLE,
        ] {
            assert!(!sentence.contains('!'), "{sentence}");
            assert!(sentence.ends_with('.'), "{sentence}");
        }
    }
}
