//! `messages`: the message routes of a server from a terminal (ADR 0408, ADR 0721).

use std::path::PathBuf;
use std::str::FromStr;

use junto_client_core::api::PageQuery;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, AttachmentId, MessageId};
use junto_client_core::protocol::message::{
    AttachmentLimits, Block, Conversation, Document, Inline, Message, MessagePlace, MessageState,
    SendMessage,
};
use junto_client_core::transfers::{Transfers, UploadSource};
use junto_client_core::{Client, ServerHandle};

use crate::{Arguments, Failure, end_session, failed, logged_in, shown, usage};

enum Action {
    List(PlaceId, PageQuery),
    Show(MessageId),
    Send {
        place: PlaceId,
        text: String,
        reply_to: Option<MessageId>,
        files: Vec<PathBuf>,
    },
    Edit(MessageId, String),
    Delete(MessageId),
    React(MessageId, String, bool),
    Read(PlaceId, u64),
    Unread(PlaceId),
    Conversations,
    Open(AccountId),
    Limits,
    SetLimits(AttachmentLimits),
    Upload(PlaceId, PathBuf),
    Download {
        message_id: MessageId,
        attachment_id: AttachmentId,
        directory: PathBuf,
    },
}

/// A place as the tool takes it: `channel:<id>`, `direct:<conversation id>`, or the
/// identifier of a channel alone.
fn place(text: &str) -> Result<PlaceId, Failure> {
    let wrong = || usage("a place is channel:<id> or direct:<conversation id>");
    let (kind, id) = text.split_once(':').unwrap_or(("channel", text));
    match kind {
        "channel" => id.parse().map(PlaceId::Channel).map_err(|_| wrong()),
        "direct" => id.parse().map(PlaceId::Direct).map_err(|_| wrong()),
        _ => Err(wrong()),
    }
}

fn word<'a>(words: &'a [String], position: usize, what: &str) -> Result<&'a str, Failure> {
    words
        .get(position)
        .map(String::as_str)
        .ok_or_else(|| usage(format!("{what} is missing")))
}

fn identifier<T: FromStr>(words: &[String], position: usize, what: &str) -> Result<T, Failure> {
    word(words, position, what)?
        .parse()
        .map_err(|_| usage(format!("{what} is not an identifier")))
}

fn number(arguments: &Arguments, name: &str) -> Result<Option<u64>, Failure> {
    arguments
        .value(name)
        .map(|text| {
            text.parse()
                .map_err(|_| usage(format!("--{name} needs a whole number")))
        })
        .transpose()
}

fn parse(words: &[String], arguments: &Arguments) -> Result<Action, Failure> {
    let action = word(words, 0, "the action of messages")?;
    let text = || {
        arguments
            .value("text")
            .map(str::to_owned)
            .ok_or_else(|| usage("--text is missing"))
    };
    Ok(match action {
        "list" => Action::List(
            place(word(words, 1, "the place")?)?,
            PageQuery {
                before: number(arguments, "before")?,
                after: number(arguments, "after")?,
                limit: arguments.number("limit")?,
                ..PageQuery::default()
            },
        ),
        "show" => Action::Show(identifier(words, 1, "the message")?),
        "send" => Action::Send {
            place: place(word(words, 1, "the place")?)?,
            text: arguments.value("text").unwrap_or_default().to_owned(),
            reply_to: arguments
                .value("reply")
                .map(|id| id.parse().map_err(|_| usage("--reply needs a message")))
                .transpose()?,
            files: arguments.values("file").iter().map(PathBuf::from).collect(),
        },
        "edit" => Action::Edit(identifier(words, 1, "the message")?, text()?),
        "delete" => Action::Delete(identifier(words, 1, "the message")?),
        "react" | "unreact" => Action::React(
            identifier(words, 1, "the message")?,
            word(words, 2, "the emoji")?.to_owned(),
            action == "react",
        ),
        "read" => Action::Read(
            place(word(words, 1, "the place")?)?,
            word(words, 2, "the position")?
                .parse()
                .map_err(|_| usage("the position is a whole number"))?,
        ),
        "unread" => Action::Unread(place(word(words, 1, "the place")?)?),
        "conversations" => Action::Conversations,
        "open" => Action::Open(identifier(words, 1, "the account")?),
        "limits" => Action::Limits,
        "set-limits" => {
            let bytes = number(arguments, "bytes")?.ok_or_else(|| usage("--bytes is missing"))?;
            let files = arguments
                .number("files")?
                .ok_or_else(|| usage("--files is missing"))?;
            Action::SetLimits(AttachmentLimits {
                bytes_per_file: bytes,
                files_per_message: files,
            })
        }
        "upload" => Action::Upload(
            place(word(words, 1, "the place")?)?,
            PathBuf::from(word(words, 2, "the file")?),
        ),
        "download" => Action::Download {
            message_id: identifier(words, 1, "the message")?,
            attachment_id: identifier(words, 2, "the attachment")?,
            directory: PathBuf::from(
                arguments
                    .value("to")
                    .ok_or_else(|| usage("--to <directory> is missing"))?,
            ),
        },
        other => return Err(usage(format!("unknown action of messages: {other}"))),
    })
}

/// The document of a text without formatting: one paragraph for each part that an empty
/// line ends.
pub(crate) fn plain(text: &str) -> Document {
    let blocks = text
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| Block::Paragraph {
            content: vec![Inline::Text {
                text: paragraph.to_owned(),
                marks: Vec::new(),
            }],
        })
        .collect();
    Document { blocks }
}

fn inline_text(content: &[Inline]) -> String {
    content
        .iter()
        .map(|node| match node {
            Inline::Text { text, .. } | Inline::Code { text } => text.clone(),
            Inline::Link { text, url } => format!("{text} <{url}>"),
            Inline::Account { account_id } => format!("@account:{account_id}"),
            Inline::Role { role_id } => format!("@role:{role_id}"),
            Inline::Everyone => "@everyone".to_owned(),
        })
        .collect()
}

/// The text of a document as lines of a terminal, without its formatting.
fn document_lines(document: &Document) -> Vec<String> {
    let mut lines = Vec::new();
    for block in &document.blocks {
        match block {
            Block::Paragraph { content } => lines.push(inline_text(content)),
            Block::Quote { content } => lines.push(format!("> {}", inline_text(content))),
            Block::Code { text, language } => {
                lines.push(format!("```{}", language.as_deref().unwrap_or_default()));
                lines.extend(text.lines().map(str::to_owned));
                lines.push("```".to_owned());
            }
            Block::List { ordered, items } => {
                for (index, item) in items.iter().enumerate() {
                    let mark = if *ordered {
                        format!("{}.", index + 1)
                    } else {
                        "-".to_owned()
                    };
                    lines.push(format!("{mark} {}", inline_text(item)));
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    let mark = if item.checked { "[x]" } else { "[ ]" };
                    lines.push(format!("- {mark} {}", inline_text(&item.content)));
                }
            }
        }
    }
    lines
}

pub(crate) fn message_lines(message: &Message) -> Vec<String> {
    let author = match (&message.author_id, &message.system) {
        (Some(author_id), _) => format!(
            "{}  {}",
            author_id,
            shown(message.author_name.as_deref().unwrap_or_default())
        ),
        (None, Some(system)) => format!("system {}", shown(&system.action)),
        (None, None) => "nobody".to_owned(),
    };
    let mut marks = String::new();
    if message.edited_at.is_some() {
        marks.push_str("  edited");
    }
    if message.deleted {
        marks.push_str("  deleted");
    }
    if message.permanent {
        marks.push_str("  permanent");
    }
    if message.pinned {
        marks.push_str("  pinned");
    }
    if message.files_removed_by_retention {
        marks.push_str("  files removed by retention");
    }
    if let Some(reply_to) = message.reply_to {
        marks.push_str(&format!("  reply to {reply_to}"));
    }
    if let Some(thread_root) = message.thread_root {
        marks.push_str(&format!("  in thread {thread_root}"));
    }
    if message.reply_count > 0 {
        marks.push_str(&format!("  thread replies {}", message.reply_count));
    }
    let mut lines = vec![format!(
        "message: {}  position {}  {}  {author}{marks}",
        message.id, message.position, message.created_at
    )];
    if let Some(system) = &message.system {
        for (name, id) in [
            ("actor", system.actor_id.map(|id| id.to_string())),
            ("subject", system.subject_id.map(|id| id.to_string())),
            ("recording", system.recording_id.map(|id| id.to_string())),
        ] {
            if let Some(id) = id {
                lines.push(format!("  {name}: {id}"));
            }
        }
    }
    if let Some(document) = &message.document {
        lines.extend(
            document_lines(document)
                .iter()
                .map(|line| format!("  {}", shown(line))),
        );
    }
    for file in &message.attachments {
        lines.push(format!(
            "  file: {}  {}  {} bytes  {}",
            file.id,
            shown(&file.name),
            file.bytes,
            shown(&file.media_type)
        ));
    }
    for reaction in &message.reactions {
        let own = if reaction.own { "  own" } else { "" };
        lines.push(format!(
            "  reaction: {}  {}{own}",
            shown(&reaction.emoji),
            reaction.count
        ));
    }
    lines
}

/// A count of the server, which is exact up to 99 (ADR 0408).
pub(crate) fn count(count: u64) -> String {
    if count >= 100 {
        "100 or more".to_owned()
    } else {
        count.to_string()
    }
}

pub(crate) fn state_line(state: &MessageState) -> String {
    let place = match state.place {
        MessagePlace::Channel { channel_id, .. } => format!("channel:{channel_id}"),
        MessagePlace::Direct {
            conversation_id, ..
        } => format!("direct:{conversation_id}"),
    };
    let sign = match (state.muted, state.unread_sign) {
        (true, _) => "muted",
        (false, true) => "unread sign",
        (false, false) => "no sign",
    };
    format!(
        "messages: {place}  latest {}  read {}  unread {}  mentions {}  {sign}  \
         mentions that notify {}",
        state.latest,
        state.last_read,
        count(state.unread),
        count(state.mentions),
        count(state.notification_mentions)
    )
}

pub(crate) fn conversation_line(conversation: &Conversation) -> String {
    format!(
        "conversation: {}  {}  {}",
        conversation.id, conversation.low_account_id, conversation.high_account_id
    )
}

async fn upload(
    transfers: &Transfers,
    place: PlaceId,
    file: PathBuf,
) -> Result<AttachmentId, Failure> {
    let upload = transfers.upload(place, UploadSource::File(file.clone()));
    upload
        .finished()
        .await
        .map_err(|failure| Failure::Failed(format!("{}: {failure:?}", file.display())))
}

async fn perform(server: &ServerHandle, action: Action) -> Result<Vec<String>, Failure> {
    let api = server.api();
    let done = |()| vec!["done".to_owned()];
    Ok(match action {
        Action::List(place, query) => {
            let page = api.messages(place, &query).await.map_err(failed)?;
            let mut lines: Vec<String> = page.messages.iter().flat_map(message_lines).collect();
            lines.push(format!("more: {}", page.has_more));
            lines
        }
        Action::Show(message_id) => message_lines(&api.message(message_id).await.map_err(failed)?),
        Action::Send {
            place,
            text,
            reply_to,
            files,
        } => {
            let transfers = server.transfers();
            let mut attachments = Vec::new();
            for file in files {
                attachments.push(upload(&transfers, place, file).await?);
            }
            let message = SendMessage {
                document: plain(&text),
                reply_to,
                attachments,
            };
            message_lines(&api.send_message(place, &message).await.map_err(failed)?)
        }
        Action::Edit(message_id, text) => message_lines(
            &api.edit_message(message_id, plain(&text))
                .await
                .map_err(failed)?,
        ),
        Action::Delete(message_id) => done(api.delete_message(message_id).await.map_err(failed)?),
        Action::React(message_id, emoji, add) => {
            let asked = if add {
                api.add_reaction(message_id, &emoji).await
            } else {
                api.remove_reaction(message_id, &emoji).await
            };
            done(asked.map_err(failed)?)
        }
        Action::Read(place, position) => {
            vec![state_line(
                &api.mark_read(place, position).await.map_err(failed)?,
            )]
        }
        Action::Unread(place) => vec![state_line(&api.message_state(place).await.map_err(failed)?)],
        Action::Conversations => api
            .conversations()
            .await
            .map_err(failed)?
            .iter()
            .map(conversation_line)
            .collect(),
        Action::Open(account_id) => vec![conversation_line(
            &api.open_conversation(account_id).await.map_err(failed)?,
        )],
        Action::Limits | Action::SetLimits(_) => {
            let limits = match action {
                Action::SetLimits(limits) => api.set_attachment_limits(&limits).await,
                _ => api.attachment_limits().await,
            }
            .map_err(failed)?;
            vec![format!(
                "attachment limits: {} bytes for a file, {} files for a message",
                limits.bytes_per_file, limits.files_per_message
            )]
        }
        Action::Upload(place, file) => {
            vec![format!(
                "attachment: {}",
                upload(&server.transfers(), place, file).await?
            )]
        }
        Action::Download {
            message_id,
            attachment_id,
            directory,
        } => {
            let message = api.message(message_id).await.map_err(failed)?;
            let attachment = message
                .attachments
                .iter()
                .find(|file| file.id == attachment_id)
                .ok_or_else(|| Failure::Failed("the message has no such file".to_owned()))?;
            let saved = server
                .transfers()
                .download(attachment, directory)
                .finished()
                .await
                .map_err(|failure| Failure::Failed(format!("{failure:?}")))?;
            vec![format!("saved: {}", saved.display())]
        }
    })
}

/// `messages`: one action in one process.
pub(crate) async fn messages(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let words = arguments.positional.get(1..).unwrap_or_default();
    let action = parse(words, arguments)?;
    let server = logged_in(client, arguments).await?;
    let lines = perform(&server, action).await;
    end_session(&server).await;
    for line in lines? {
        crate::live::say(&line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::ids::{ChannelId, ConversationId};

    #[test]
    fn a_place_is_a_channel_or_a_direct_conversation() {
        let channel_id = ChannelId::generate();
        let conversation_id = ConversationId::generate();
        assert!(matches!(
            place(&channel_id.to_string()),
            Ok(PlaceId::Channel(id)) if id == channel_id
        ));
        assert!(matches!(
            place(&format!("channel:{channel_id}")),
            Ok(PlaceId::Channel(id)) if id == channel_id
        ));
        assert!(matches!(
            place(&format!("direct:{conversation_id}")),
            Ok(PlaceId::Direct(id)) if id == conversation_id
        ));
        assert!(place("").is_err());
        assert!(place("direct:").is_err());
        assert!(place("thread:1").is_err());
    }

    #[test]
    fn a_text_becomes_paragraphs_and_a_document_becomes_lines() {
        let document = plain("first line\nsecond line\n\n\n  third  ");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(
            document_lines(&document),
            ["first line\nsecond line", "third"]
        );
        assert!(plain("  \n\n ").blocks.is_empty());
    }
}
