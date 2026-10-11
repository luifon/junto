use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

macro_rules! secret_text {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(text: impl Into<String>) -> Self {
                Self(text.into())
            }

            /// Returns the secret text. The caller must not log it, and a copy that the
            /// caller makes is not erased with this value.
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl Zeroize for $name {
            fn zeroize(&mut self) {
                self.0.zeroize();
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                self.zeroize();
            }
        }

        impl ZeroizeOnDrop for $name {}

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(redacted)"))
            }
        }

        #[cfg(feature = "openapi")]
        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                utoipa::openapi::schema::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::Type::String)
                    .into()
            }
        }

        #[cfg(feature = "openapi")]
        impl utoipa::ToSchema for $name {
            fn name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }
        }
    };
}

secret_text!(
    /// The opaque token of one session. It is sent as `Authorization: Bearer <token>`.
    SessionToken
);
secret_text!(
    /// The opaque access token of an administrator (ADR 0203). It is sent as
    /// `Authorization: Bearer <token>` and is shown once, when it is created.
    AccessToken
);
secret_text!(
    /// The one-time code that the first start of a server prints.
    OwnerCode
);
secret_text!(
    /// An invite code. Letters are not case-sensitive and hyphens are ignored.
    InviteCode
);
secret_text!(
    /// The short-lived one-time code of a device link.
    LinkCode
);
secret_text!(
    /// The token with which a device joins one room of the media server. It is valid for
    /// about one minute.
    CallToken
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_does_not_contain_the_secret() {
        let token = SessionToken::new("jst_secret-value");
        assert_eq!(format!("{token:?}"), "SessionToken(redacted)");
        assert_eq!(token.expose(), "jst_secret-value");
        assert_eq!(
            serde_json::to_string(&token).unwrap(),
            "\"jst_secret-value\""
        );
        let token = AccessToken::new("jat_secret-value");
        assert_eq!(format!("{token:?}"), "AccessToken(redacted)");
    }

    #[test]
    fn a_secret_erases_its_text() {
        fn erased_when_dropped<T: ZeroizeOnDrop>() {}
        erased_when_dropped::<SessionToken>();
        erased_when_dropped::<AccessToken>();
        erased_when_dropped::<OwnerCode>();
        erased_when_dropped::<InviteCode>();
        erased_when_dropped::<LinkCode>();
        erased_when_dropped::<CallToken>();

        let mut token = SessionToken::new("jst_secret-value");
        token.zeroize();
        assert_eq!(token.expose(), "");
        let mut code: InviteCode = serde_json::from_str("\"ABCD-EFGH-JKMN-PQRS\"").unwrap();
        let copy = code.clone();
        code.zeroize();
        assert_eq!(code.expose(), "");
        assert_eq!(copy.expose(), "ABCD-EFGH-JKMN-PQRS");
    }
}
