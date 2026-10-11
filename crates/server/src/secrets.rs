use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_GROUP: usize = 4;

pub const OWNER_CODE_SYMBOLS: usize = 20;
pub const INVITE_CODE_SYMBOLS: usize = 16;
pub const LINK_CODE_SYMBOLS: usize = 8;
pub const SESSION_TOKEN_PREFIX: &str = "jst_";

#[derive(Debug, thiserror::Error)]
#[error("the operating system gave no random bytes")]
pub struct RandomnessUnavailable;

pub fn random_bytes<const N: usize>() -> Result<[u8; N], RandomnessUnavailable> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|_| RandomnessUnavailable)?;
    Ok(bytes)
}

/// A code of `symbols` random symbols of 5 bits each, written in groups of four.
pub fn random_code(symbols: usize) -> Result<String, RandomnessUnavailable> {
    let mut bytes = vec![0u8; symbols];
    getrandom::fill(&mut bytes).map_err(|_| RandomnessUnavailable)?;
    let mut code = String::with_capacity(symbols + symbols / CODE_GROUP);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && index % CODE_GROUP == 0 {
            code.push('-');
        }
        code.push(char::from(CODE_ALPHABET[usize::from(byte & 31)]));
    }
    Ok(code)
}

/// The form of a code that is hashed: no separators, upper case, and the letters that the
/// alphabet leaves out mapped to the digits they resemble.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|symbol| !matches!(symbol, '-' | ' '))
        .map(|symbol| match symbol.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub enum CodePurpose {
    Owner,
    Invite,
    DeviceLink,
}

impl CodePurpose {
    const fn domain(self) -> &'static [u8] {
        match self {
            Self::Owner => b"junto:owner-code:",
            Self::Invite => b"junto:invite-code:",
            Self::DeviceLink => b"junto:link-code:",
        }
    }
}

pub fn hash_code(purpose: CodePurpose, code: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(purpose.domain());
    hasher.update(normalize_code(code).as_bytes());
    hasher.finalize().into()
}

pub fn new_session_token() -> Result<String, RandomnessUnavailable> {
    let bytes = random_bytes::<32>()?;
    Ok(format!(
        "{SESSION_TOKEN_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub fn hash_session_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"junto:session-token:");
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

pub fn new_access_token() -> Result<String, RandomnessUnavailable> {
    let bytes = random_bytes::<32>()?;
    Ok(format!(
        "{}{}",
        junto_protocol::access_token::ACCESS_TOKEN_PREFIX,
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub fn hash_access_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"junto:access-token:");
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.ct_eq(right).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_has_the_requested_symbols_in_groups_of_four() {
        let code = random_code(INVITE_CODE_SYMBOLS).unwrap();
        assert_eq!(code.len(), 19);
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 4);
        for group in groups {
            assert_eq!(group.len(), 4);
            assert!(group.bytes().all(|symbol| CODE_ALPHABET.contains(&symbol)));
        }
        assert_eq!(random_code(LINK_CODE_SYMBOLS).unwrap().len(), 9);
        assert_ne!(random_code(16).unwrap(), random_code(16).unwrap());
    }

    #[test]
    fn hash_ignores_case_separators_and_look_alike_letters() {
        let reference = hash_code(CodePurpose::Invite, "0O1I-LABC");
        for variant in ["0o1il abc", "0011-1ABC", "00111abc", " 0O1I LA-BC "] {
            assert_eq!(
                hash_code(CodePurpose::Invite, variant),
                reference,
                "{variant}"
            );
        }
        assert_ne!(hash_code(CodePurpose::Invite, "0011-1ABD"), reference);
    }

    #[test]
    fn hash_differs_by_purpose() {
        let code = "ABCD-EFGH";
        assert_ne!(
            hash_code(CodePurpose::Invite, code),
            hash_code(CodePurpose::Owner, code)
        );
        assert_ne!(
            hash_code(CodePurpose::Invite, code),
            hash_code(CodePurpose::DeviceLink, code)
        );
    }

    #[test]
    fn session_token_is_opaque_and_long() {
        let token = new_session_token().unwrap();
        assert!(token.starts_with("jst_"));
        assert_eq!(token.len(), 4 + 43);
        assert_ne!(token, new_session_token().unwrap());
        assert_ne!(hash_session_token(&token), hash_session_token("jst_other"));
    }

    #[test]
    fn access_token_is_opaque_and_hashed_apart_from_a_session_token() {
        let token = new_access_token().unwrap();
        assert!(token.starts_with("jat_"));
        assert_eq!(token.len(), 4 + 43);
        assert_ne!(token, new_access_token().unwrap());
        assert_ne!(hash_access_token(&token), hash_session_token(&token));
    }

    #[test]
    fn constant_time_eq_compares_content_and_length() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"sama"));
        assert!(!constant_time_eq(b"same", b"same-longer"));
    }
}
