use crate::error::{ApiError, ApiResult};

pub const NAME_MAX_CHARS: usize = 64;

/// A name of 1 to 64 characters without control characters, with outer whitespace removed.
pub fn name(field: &'static str, value: &str) -> ApiResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::invalid(format!("{field} is empty")));
    }
    checked(field, trimmed)
}

/// Like [`name`], and an empty value is allowed.
pub fn optional_name(field: &'static str, value: &str) -> ApiResult<String> {
    checked(field, value.trim())
}

fn checked(field: &'static str, trimmed: &str) -> ApiResult<String> {
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(ApiError::invalid(format!(
            "{field} is longer than {NAME_MAX_CHARS} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ApiError::invalid(format!(
            "{field} contains a control character"
        )));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_trimmed_and_bounded() {
        assert_eq!(name("name", "  Acme  ").unwrap(), "Acme");
        assert_eq!(name("name", &"é".repeat(64)).unwrap().chars().count(), 64);
        assert!(name("name", &"a".repeat(65)).is_err());
        assert!(name("name", "   ").is_err());
        assert!(name("name", "line\nbreak").is_err());
    }

    #[test]
    fn optional_name_allows_empty() {
        assert_eq!(optional_name("device_name", " ").unwrap(), "");
        assert!(optional_name("device_name", "tab\there").is_err());
    }
}
