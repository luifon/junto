//! The readable form of the messages of an export: one HTML file (ADR 0411).
//!
//! Every text that a person wrote, and every name, is escaped. The page loads nothing: its
//! content policy allows no script and no address outside the archive.

use std::collections::HashMap;
use std::fmt::Write;

use junto_protocol::ids::{AccountId, WorkspaceRoleId};
use junto_protocol::message::{Attachment, Block, Document, Inline, Mark, Message};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The names that the page shows for the accounts and roles that messages mention.
#[derive(Debug, Default)]
pub struct Names {
    pub accounts: HashMap<AccountId, String>,
    pub roles: HashMap<WorkspaceRoleId, String>,
}

/// `text` with every character that HTML reads as markup replaced by its reference.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// One part of a relative address, with every byte outside letters, digits, `-`, `.` and
/// `_` written as a percent sequence.
fn address_part(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// The name of an attachment as a name of a file in the archive: the name without the
/// characters that a path or a file system reads, never empty, never `.` or `..`.
pub fn file_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect();
    while safe.len() > 200 {
        safe.pop();
    }
    let trimmed =
        safe.trim_matches(|character: char| character == '.' || character.is_whitespace());
    if trimmed.is_empty() {
        "file".to_owned()
    } else if trimmed.len() == safe.len() {
        safe
    } else {
        trimmed.to_owned()
    }
}

/// The name of the entry of an attachment in the archive.
pub fn attachment_entry(attachment: &Attachment) -> String {
    format!(
        "attachments/{}/{}",
        attachment.id,
        file_name(&attachment.name)
    )
}

fn time(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

/// The start of the page. `title` is the name of the channel or of the conversation.
pub fn head(title: &str, exported_at: OffsetDateTime) -> String {
    let title = escape(title);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
         style-src 'unsafe-inline'\">\n<title>{title}</title>\n<style>\n\
         body{{font-family:sans-serif;max-width:60em;margin:2em auto;line-height:1.4}}\n\
         article{{border-top:1px solid #ccc;padding:.5em 0}}\n\
         article.reply{{margin-left:2em}}\n\
         header{{color:#555;font-size:.9em}}\n\
         .author{{color:#000;font-weight:bold}}\n\
         .system{{color:#555;font-style:italic}}\n\
         pre{{background:#f4f4f4;padding:.5em;overflow:auto}}\n\
         blockquote{{border-left:3px solid #ccc;margin-left:0;padding-left:1em}}\n\
         </style>\n</head>\n<body>\n<h1>{title}</h1>\n<p class=\"system\">Exported <time>{}</time></p>\n",
        time(exported_at)
    )
}

pub const TAIL: &str = "</body>\n</html>\n";

fn inline(node: &Inline, names: &Names, out: &mut String) {
    match node {
        Inline::Text { text, marks } => {
            let tags: Vec<&str> = marks
                .iter()
                .map(|mark| match mark {
                    Mark::Bold => "strong",
                    Mark::Italic => "em",
                    Mark::Strikethrough => "s",
                })
                .collect();
            for tag in &tags {
                let _ = write!(out, "<{tag}>");
            }
            out.push_str(&escape(text).replace('\n', "<br>"));
            for tag in tags.iter().rev() {
                let _ = write!(out, "</{tag}>");
            }
        }
        Inline::Code { text } => {
            let _ = write!(out, "<code>{}</code>", escape(text));
        }
        Inline::Link { text, url } => {
            let plain = ["https://", "http://", "mailto:"]
                .iter()
                .any(|scheme| url.starts_with(scheme));
            if plain {
                let _ = write!(
                    out,
                    "<a href=\"{}\" rel=\"noopener noreferrer\">{}</a>",
                    escape(url),
                    escape(text)
                );
            } else {
                out.push_str(&escape(text));
            }
        }
        Inline::Account { account_id } => {
            let name = names
                .accounts
                .get(account_id)
                .map_or_else(|| account_id.to_string(), Clone::clone);
            let _ = write!(out, "<span class=\"mention\">@{}</span>", escape(&name));
        }
        Inline::Role { role_id } => {
            let name = names
                .roles
                .get(role_id)
                .map_or_else(|| role_id.to_string(), Clone::clone);
            let _ = write!(out, "<span class=\"mention\">@{}</span>", escape(&name));
        }
        Inline::Everyone => out.push_str("<span class=\"mention\">@everyone</span>"),
    }
}

fn inlines(nodes: &[Inline], names: &Names, out: &mut String) {
    for node in nodes {
        inline(node, names, out);
    }
}

fn document(document: &Document, names: &Names, out: &mut String) {
    for block in &document.blocks {
        match block {
            Block::Paragraph { content } => {
                out.push_str("<p>");
                inlines(content, names, out);
                out.push_str("</p>\n");
            }
            Block::Quote { content } => {
                out.push_str("<blockquote>");
                inlines(content, names, out);
                out.push_str("</blockquote>\n");
            }
            Block::Code { text, .. } => {
                let _ = writeln!(out, "<pre><code>{}</code></pre>", escape(text));
            }
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let _ = write!(out, "<{tag}>");
                for item in items {
                    out.push_str("<li>");
                    inlines(item, names, out);
                    out.push_str("</li>");
                }
                let _ = writeln!(out, "</{tag}>");
            }
            Block::Checklist { items } => {
                out.push_str("<ul>");
                for item in items {
                    out.push_str(if item.checked { "<li>[x] " } else { "<li>[ ] " });
                    inlines(&item.content, names, out);
                    out.push_str("</li>");
                }
                out.push_str("</ul>\n");
            }
        }
    }
}

/// One message of the page.
pub fn message(message: &Message, names: &Names) -> String {
    let mut out = String::new();
    let class = if message.thread_root.is_some() {
        " class=\"reply\""
    } else {
        ""
    };
    let _ = write!(out, "<article id=\"m-{}\"{class}>\n<header>", message.id);
    match (&message.author_name, message.author_id) {
        (Some(name), _) => {
            let _ = write!(out, "<span class=\"author\">{}</span> ", escape(name));
        }
        (None, Some(author_id)) => {
            let _ = write!(out, "<span class=\"author\">{author_id}</span> ");
        }
        (None, None) => {}
    }
    let at = time(message.created_at);
    let _ = write!(out, "<time datetime=\"{at}\">{at}</time>");
    if let Some(edited_at) = message.edited_at {
        let _ = write!(out, " (edited {})", time(edited_at));
    }
    if message.permanent {
        out.push_str(" (permanent)");
    }
    if message.pinned {
        out.push_str(" (pinned)");
    }
    if let Some(root) = message.thread_root {
        let _ = write!(out, " <a href=\"#m-{root}\">in a thread</a>");
    } else if let Some(reply_to) = message.reply_to {
        let _ = write!(out, " <a href=\"#m-{reply_to}\">reply</a>");
    }
    out.push_str("</header>\n");
    if let Some(text) = &message.document {
        document(text, names, &mut out);
    }
    if let Some(system) = &message.system {
        let actor = system.actor_id.map_or_else(String::new, |actor_id| {
            let name = names
                .accounts
                .get(&actor_id)
                .map_or_else(|| actor_id.to_string(), Clone::clone);
            format!(" by {}", escape(&name))
        });
        let duration = system
            .duration_seconds
            .map_or_else(String::new, |seconds| format!(", {seconds} seconds"));
        let _ = writeln!(
            out,
            "<p class=\"system\">{}{actor}{duration}</p>",
            escape(&system.action.replace('_', " "))
        );
    }
    if message.deleted {
        out.push_str("<p class=\"system\">deleted message</p>\n");
    }
    if message.files_removed_by_retention {
        out.push_str("<p class=\"system\">files removed by retention</p>\n");
    }
    if !message.attachments.is_empty() {
        out.push_str("<ul class=\"attachments\">");
        for attachment in &message.attachments {
            let _ = write!(
                out,
                "<li><a href=\"attachments/{}/{}\">{}</a> ({} bytes)</li>",
                attachment.id,
                escape(&address_part(&file_name(&attachment.name))),
                escape(&attachment.name),
                attachment.bytes
            );
        }
        out.push_str("</ul>\n");
    }
    if !message.reactions.is_empty() {
        out.push_str("<p class=\"reactions\">");
        for reaction in &message.reactions {
            let _ = write!(out, "{} {} ", escape(&reaction.emoji), reaction.count);
        }
        out.push_str("</p>\n");
    }
    out.push_str("</article>\n");
    out
}

/// The accounts and roles that a page of messages mentions or names as actors.
pub fn mentioned(messages: &[Message]) -> (Vec<AccountId>, Vec<WorkspaceRoleId>) {
    let mut accounts = Vec::new();
    let mut roles = Vec::new();
    let mut take = |node: &Inline| match node {
        Inline::Account { account_id } if !accounts.contains(account_id) => {
            accounts.push(*account_id);
        }
        Inline::Role { role_id } if !roles.contains(role_id) => roles.push(*role_id),
        _ => {}
    };
    for message in messages {
        for block in message.document.iter().flat_map(|text| &text.blocks) {
            match block {
                Block::Paragraph { content } | Block::Quote { content } => {
                    content.iter().for_each(&mut take);
                }
                Block::List { items, .. } => items.iter().flatten().for_each(&mut take),
                Block::Checklist { items } => {
                    items
                        .iter()
                        .flat_map(|item| &item.content)
                        .for_each(&mut take);
                }
                Block::Code { .. } => {}
            }
        }
    }
    for message in messages {
        if let Some(actor_id) = message.system.as_ref().and_then(|system| system.actor_id)
            && !accounts.contains(&actor_id)
        {
            accounts.push(actor_id);
        }
    }
    (accounts, roles)
}

#[cfg(test)]
mod tests {
    use junto_protocol::ids::{AttachmentId, ChannelId, MessageId, WorkspaceId};
    use junto_protocol::message::{MessagePlace, Reaction, SystemMessage};

    use super::*;

    const HOSTILE: &str = "<script>alert('x')</script>\"&";

    fn sample(document: Option<Document>) -> Message {
        Message {
            id: MessageId::generate(),
            place: MessagePlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
            },
            position: 1,
            author_id: Some(AccountId::generate()),
            author_name: Some(HOSTILE.to_owned()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            edited_at: None,
            document,
            system: None,
            reply_to: None,
            permanent: false,
            deleted: false,
            attachments: Vec::new(),
            reactions: Vec::new(),
            thread_root: None,
            reply_count: 0,
            last_reply: None,
            pinned: false,
            files_removed_by_retention: false,
            reply_authors: Vec::new(),
        }
    }

    fn text(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: vec![Mark::Bold],
        }
    }

    #[test]
    fn every_text_of_a_message_is_escaped() {
        let account_id = AccountId::generate();
        let role_id = WorkspaceRoleId::generate();
        let names = Names {
            accounts: HashMap::from([(account_id, HOSTILE.to_owned())]),
            roles: HashMap::from([(role_id, HOSTILE.to_owned())]),
        };
        let mut message = sample(Some(Document {
            blocks: vec![
                Block::Paragraph {
                    content: vec![
                        text(HOSTILE),
                        Inline::Code {
                            text: HOSTILE.to_owned(),
                        },
                        Inline::Link {
                            text: HOSTILE.to_owned(),
                            url: format!("https://example.test/?q={HOSTILE}"),
                        },
                        Inline::Link {
                            text: "plain".to_owned(),
                            url: "javascript:alert(1)".to_owned(),
                        },
                        Inline::Account { account_id },
                        Inline::Role { role_id },
                        Inline::Everyone,
                    ],
                },
                Block::Quote {
                    content: vec![text(HOSTILE)],
                },
                Block::Code {
                    text: HOSTILE.to_owned(),
                    language: Some("rust".to_owned()),
                },
                Block::List {
                    ordered: true,
                    items: vec![vec![text(HOSTILE)]],
                },
                Block::Checklist {
                    items: vec![junto_protocol::message::CheckItem {
                        checked: true,
                        content: vec![text(HOSTILE)],
                    }],
                },
            ],
        }));
        message.attachments = vec![Attachment {
            id: AttachmentId::generate(),
            name: format!("{HOSTILE}.txt"),
            bytes: 3,
            sha256: String::new(),
            media_type: "text/plain".to_owned(),
            width: None,
            height: None,
        }];
        message.reactions = vec![Reaction {
            emoji: "<b>".to_owned(),
            count: 2,
            own: false,
        }];
        message.system = Some(SystemMessage {
            action: "<img src=x>".to_owned(),
            subject_id: None,
            actor_id: Some(account_id),
            recording_id: None,
            duration_seconds: Some(125),
            name: None,
            previous_name: None,
        });
        let page = format!(
            "{}{}{TAIL}",
            head(HOSTILE, message.created_at),
            self::message(&message, &names)
        );
        assert!(!page.contains("<script"), "{page}");
        assert!(!page.contains("alert('x')"), "{page}");
        assert!(!page.contains("<img"), "{page}");
        assert!(!page.contains("<b>"), "{page}");
        assert!(!page.contains("javascript:"), "{page}");
        assert!(page.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&quot;&amp;"));
        assert_eq!(page.matches("<a href=\"https://").count(), 1);
        assert!(page.contains("default-src 'none'"));
        assert!(page.contains(", 125 seconds</p>"), "{page}");
    }

    #[test]
    fn the_name_of_a_file_in_the_archive_has_no_path() {
        assert_eq!(file_name("report.pdf"), "report.pdf");
        assert_eq!(file_name(".."), "file");
        assert_eq!(file_name("."), "file");
        assert_eq!(file_name("  "), "file");
        assert_eq!(file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(file_name("a\\b:c*d?e\"f<g>h|i"), "a_b_c_d_e_f_g_h_i");
        assert_eq!(file_name("line\nbreak"), "line_break");
        assert_eq!(file_name(".hidden"), "hidden");
        assert!(file_name(&"ä".repeat(300)).len() <= 200);
        for name in ["..", "../x", "a/b", "ok.txt", ".hidden", "x."] {
            let entry = format!("attachments/id/{}", file_name(name));
            assert!(crate::exports::zip::is_entry_name(&entry), "{entry}");
        }
        assert_eq!(address_part("a b/ä\"<"), "a%20b%2F%C3%A4%22%3C");
    }

    #[test]
    fn the_mentioned_accounts_and_roles_of_a_page_are_collected_once() {
        let account_id = AccountId::generate();
        let role_id = WorkspaceRoleId::generate();
        let message = sample(Some(Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Account { account_id },
                    Inline::Account { account_id },
                    Inline::Role { role_id },
                ],
            }],
        }));
        assert_eq!(
            mentioned(&[message.clone(), message]),
            (vec![account_id], vec![role_id])
        );
    }
}
