use crate::ids::ServerRoleId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoleColor(u32);
impl RoleColor {
    pub const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self(((red as u32) << 16) | ((green as u32) << 8) | blue as u32)
    }
    pub const fn rgb(self) -> u32 {
        self.0
    }
}
impl fmt::Display for RoleColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:06X}", self.0)
    }
}
impl FromStr for RoleColor {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 7
            || !text.starts_with('#')
            || !text.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            return Err("a role colour must be #RRGGBB");
        }
        u32::from_str_radix(&text[1..], 16)
            .map(Self)
            .map_err(|_| "a role colour must be #RRGGBB")
    }
}
impl Serialize for RoleColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for RoleColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for RoleColor {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .pattern(Some("^#[0-9a-fA-F]{6}$"))
            .into()
    }
}
#[cfg(feature = "openapi")]
impl utoipa::ToSchema for RoleColor {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RoleColor")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HoistedRole {
    pub id: ServerRoleId,
    pub name: String,
    pub position: i32,
    pub color: Option<RoleColor>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccountAppearance {
    #[serde(default)]
    pub color: Option<RoleColor>,
    #[serde(default)]
    pub group: Option<HoistedRole>,
}
impl AccountAppearance {
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.group.is_none()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn colours_are_exact_rgb_and_canonical_on_the_wire() {
        let color: RoleColor = "#a0b1c2".parse().unwrap();
        assert_eq!(color, RoleColor::from_rgb(160, 177, 194));
        assert_eq!(serde_json::to_string(&color).unwrap(), "\"#A0B1C2\"");
        for bad in [
            "red",
            "#fff",
            "#12345678",
            "123456",
            "#12345G",
            "#12é34",
            " #123456",
            "#123456 ",
        ] {
            assert!(bad.parse::<RoleColor>().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<RoleColor>("16777215").is_err());
    }
    #[test]
    fn a_colour_patch_distinguishes_absent_clear_and_set() {
        use crate::roles::UpdateServerRole;
        let absent: UpdateServerRole = serde_json::from_str("{}").unwrap();
        let clear: UpdateServerRole = serde_json::from_str("{\"color\":null}").unwrap();
        let set: UpdateServerRole = serde_json::from_str("{\"color\":\"#336699\"}").unwrap();
        assert_eq!(absent.color, None);
        assert_eq!(clear.color, Some(None));
        assert_eq!(set.color, Some(Some(RoleColor::from_rgb(51, 102, 153))));
        assert_eq!(
            serde_json::to_value(clear).unwrap()["color"],
            serde_json::Value::Null
        );
    }
}
