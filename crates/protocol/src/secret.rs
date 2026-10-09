use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::{PartialSchema, ToSchema};

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

            /// Returns the secret text. The caller must not log it.
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(redacted)"))
            }
        }

        impl PartialSchema for $name {
            fn schema() -> RefOr<Schema> {
                ObjectBuilder::new().schema_type(Type::String).into()
            }
        }

        impl ToSchema for $name {
            fn name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }
        }
    };
}

secret_text!(
    /// The opaque token of one session. It is sent as `Authorization: Bearer <token>`.
    SessionToken
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
    }
}
