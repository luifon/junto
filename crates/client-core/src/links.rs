//! Addresses that open a place in the application.

use junto_protocol::ids::{AttachmentId, ChannelId, ConversationId, MessageId, RecordingId};
use junto_protocol::secret::InviteCode;
use url::{Url, form_urlencoded};

use crate::address::ServerAddress;

const LONGEST_LINK: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppLink {
    Invite {
        server: ServerAddress,
        code: InviteCode,
    },
    Message {
        server: ServerAddress,
        id: MessageId,
    },
    Thread {
        server: ServerAddress,
        id: MessageId,
    },
    Channel {
        server: ServerAddress,
        id: ChannelId,
    },
    Conversation {
        server: ServerAddress,
        id: ConversationId,
    },
    File {
        server: ServerAddress,
        id: AttachmentId,
    },
    Recording {
        server: ServerAddress,
        id: RecordingId,
    },
}

/// Reads one application address without accepting ambiguous or additional parameters.
pub fn parse(text: &str) -> Option<AppLink> {
    if text.len() > LONGEST_LINK
        || text
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || !valid_escapes(text)
    {
        return None;
    }
    let link = Url::parse(text).ok()?;
    if link.scheme() != "junto"
        || !link.username().is_empty()
        || link.password().is_some()
        || link.port().is_some()
        || !link.path().is_empty()
        || link.fragment().is_some()
    {
        return None;
    }
    match link.host_str()? {
        "invite" => {
            let (server, code) = read_target(&link, "code")?;
            valid_invite_code(&code).then(|| AppLink::Invite {
                server,
                code: InviteCode::new(code),
            })
        }
        "message" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::Message {
                server,
                id: id.parse().ok()?,
            })
        }
        "thread" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::Thread {
                server,
                id: id.parse().ok()?,
            })
        }
        "channel" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::Channel {
                server,
                id: id.parse().ok()?,
            })
        }
        "conversation" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::Conversation {
                server,
                id: id.parse().ok()?,
            })
        }
        "file" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::File {
                server,
                id: id.parse().ok()?,
            })
        }
        "recording" => {
            let (server, id) = read_target(&link, "id")?;
            Some(AppLink::Recording {
                server,
                id: id.parse().ok()?,
            })
        }
        _ => None,
    }
}

fn read_target(link: &Url, parameter: &str) -> Option<(ServerAddress, String)> {
    let mut server = None;
    let mut target = None;
    for (key, value) in link.query_pairs() {
        match key.as_ref() {
            "server" if server.is_none() => {
                Url::parse(&value).ok()?;
                server = Some(ServerAddress::parse(&value).ok()?);
            }
            key if key == parameter && target.is_none() => target = Some(value.into_owned()),
            _ => return None,
        }
    }
    Some((server?, target?))
}

/// Writes the canonical origin and typed destination using percent-encoded parameters.
pub fn format(link: &AppLink) -> String {
    match link {
        AppLink::Invite { server, code } => write_target("invite", server, "code", code.expose()),
        AppLink::Message { server, id } => write_target("message", server, "id", &id.to_string()),
        AppLink::Thread { server, id } => write_target("thread", server, "id", &id.to_string()),
        AppLink::Channel { server, id } => write_target("channel", server, "id", &id.to_string()),
        AppLink::Conversation { server, id } => {
            write_target("conversation", server, "id", &id.to_string())
        }
        AppLink::File { server, id } => write_target("file", server, "id", &id.to_string()),
        AppLink::Recording { server, id } => {
            write_target("recording", server, "id", &id.to_string())
        }
    }
}

pub fn valid_invite_code(code: &str) -> bool {
    code.len() <= 32
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && code.bytes().filter(|byte| *byte != b'-').count() == 16
}

/// A browser page for a person who has not installed the application yet.
pub fn invite_page(server: &ServerAddress, code: &InviteCode) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("code", code.expose())
        .finish();
    format!("{}/join?{query}", server.as_str())
}

fn write_target(kind: &str, server: &ServerAddress, parameter: &str, target: &str) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("server", server.as_str())
        .append_pair(parameter, target)
        .finish();
    format!("junto://{kind}?{query}")
}

fn valid_escapes(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'%'
            || bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f";

    #[test]
    fn invite_links_keep_the_origin_and_code_without_exposing_it_in_debug() {
        let link = AppLink::Invite {
            server: ServerAddress::parse("HTTPS://Chat.Example.Test:443/").unwrap(),
            code: InviteCode::new("abcd-efgh-jkmn-pqrs"),
        };
        assert_eq!(parse(&format(&link)), Some(link.clone()));
        assert!(!format!("{link:?}").contains("abcd"));
        let AppLink::Invite { server, code } = link else {
            unreachable!()
        };
        assert_eq!(
            invite_page(&server, &code),
            "https://chat.example.test/join?code=abcd-efgh-jkmn-pqrs"
        );
        for code in [
            "",
            "short",
            "0123456789ABCDEF%26other=value",
            "0123456789ABCDEF%0A",
            "0123456789ABCDE%2F",
        ] {
            assert!(
                parse(&format!(
                    "junto://invite?server=https%3A%2F%2Fchat.example.test&code={code}"
                ))
                .is_none()
            );
        }
        assert!(parse("junto://invite?server=https%3A%2F%2Fchat.example.test&code=0123456789ABCDEF&code=0123456789ABCDEF").is_none());
    }

    #[test]
    fn message_links_round_trip_with_canonical_origins() {
        for (address, canonical) in [
            (
                "HTTPS://Chat.Example.Test:443/",
                "https://chat.example.test",
            ),
            (
                "https://chat.example.test:8443",
                "https://chat.example.test:8443",
            ),
            ("http://LOCALHOST:80/", "http://localhost"),
            ("http://[::1]:8080", "http://[::1]:8080"),
        ] {
            let link = AppLink::Message {
                server: ServerAddress::parse(address).unwrap(),
                id: ID.parse().unwrap(),
            };
            let written = format(&link);
            assert_eq!(parse(&written), Some(link));
            let values: Vec<_> = Url::parse(&written)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            assert_eq!(
                values,
                [
                    ("server".to_owned(), canonical.to_owned()),
                    ("id".to_owned(), ID.to_owned())
                ]
            );
        }
        let canonical = format(&AppLink::Message {
            server: ServerAddress::parse("chat.example.test").unwrap(),
            id: ID.parse().unwrap(),
        });
        assert_eq!(
            canonical,
            format!("junto://message?server=https%3A%2F%2Fchat.example.test&id={ID}")
        );
    }

    #[test]
    fn ambiguous_or_unsafe_links_are_refused() {
        let valid = format!("junto://message?server=https%3A%2F%2Fchat.example.test&id={ID}");
        for bad in [
            String::new(),
            valid.replace("junto:", "https:"),
            valid.replace("//message", "//user@message"),
            valid.replace("//message", "//message:1234"),
            valid.replace("//message?", "//message/path?"),
            valid.replace("//message", "//invite"),
            format!("{valid}#fragment"),
            format!("{valid}&id={ID}"),
            format!("{valid}&server=https%3A%2F%2Fother.example.test"),
            format!("{valid}&unknown=value"),
            valid.replace(ID, "not-an-identifier"),
            valid.replace("https%3A%2F%2Fchat.example.test", "chat.example.test"),
            valid.replace("https%3A", "http%3A"),
            valid.replace("chat.example.test", "chat.example.test%2Fapi"),
            valid.replace("chat.example.test", "chat.example.test%ZZ"),
            valid.replace("chat.example.test", "chat.example.test%"),
            format!(" {valid}"),
            format!("{valid}\n"),
            format!("junto://message?id={ID}"),
            "junto://message?server=https%3A%2F%2Fchat.example.test".to_owned(),
            "x".repeat(LONGEST_LINK + 1),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn channel_conversation_file_recording_and_thread_links_round_trip() {
        for kind in ["channel", "conversation", "file", "recording", "thread"] {
            let written = format!("junto://{kind}?server=https%3A%2F%2Fchat.example.test&id={ID}");
            let link = parse(&written).unwrap_or_else(|| panic!("{kind} is not accepted"));
            assert_eq!(format(&link), written);
            for bad in [
                format!("{written}&id={ID}"),
                format!("{written}&unknown=value"),
                written.replace(ID, "not-an-identifier"),
                written.replace("https%3A", "file%3A"),
                format!("{written}#fragment"),
            ] {
                assert_eq!(parse(&bad), None, "{bad}");
            }
        }
    }
}
