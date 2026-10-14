//! The bounds of the messages that the client takes from a server (ADR 0747).
//!
//! ADR 0408 bounds a message on the server. A server can be hostile (ADR 0714), so the
//! client does not rely on that: it measures each message that an answer holds, takes none
//! above the bounds, and holds no more than a stated weight in each list of messages. The
//! bound of one answer of the transport (4 MiB) bounds no list, because a list is filled
//! by many answers.

use junto_protocol::message::{Block, Document, Inline, Message};

/// The blocks of the document of one message (ADR 0408).
pub const MOST_BLOCKS: usize = 100;
/// The inline nodes of the document of one message (ADR 0408).
pub const MOST_INLINE_NODES: usize = 1_000;
/// The items of the lists of the document of one message. ADR 0408 states no number: its
/// 16 KiB of compact JSON hold fewer than 5,500 items.
pub const MOST_LIST_ITEMS: usize = 5_500;
/// The files of one message: the highest limit that a server can be set to (ADR 0408).
pub const MOST_FILES: usize = 100;
/// The different emoji of the reactions of one message (ADR 0408).
pub const MOST_REACTIONS: usize = 20;
/// The weight of one message. A message inside the bounds of ADR 0408 weighs less than
/// half of it: a document of 16 KiB with 1,000 inline nodes, or with the 5,400 empty list
/// items that 16 KiB can state, and 100 files with names of 200 characters.
pub const MOST_MESSAGE_WEIGHT: usize = 512 * 1024;

/// The weight of the messages of one timeline (ADR 0721): at least 32 messages.
pub const MOST_LOADED_WEIGHT: usize = 16 * 1024 * 1024;
/// The weight of the messages of one pin list.
pub const MOST_PINNED_WEIGHT: usize = 8 * 1024 * 1024;
/// The weight of the messages that the unread threads and the notices name.
pub const MOST_NAMED_WEIGHT: usize = 8 * 1024 * 1024;

/// What a block, an inline node, a file and a reaction count in addition to their text:
/// the memory of their structure, rounded up.
const NODE: usize = 64;
/// What an item of a list counts in addition to its nodes.
const ITEM: usize = 24;

fn inline(nodes: &[Inline]) -> usize {
    nodes
        .iter()
        .map(|node| {
            NODE + match node {
                Inline::Text { text, marks } => text.len() + marks.len(),
                Inline::Code { text } => text.len(),
                Inline::Link { text, url } => text.len() + url.len(),
                Inline::Account { .. } | Inline::Role { .. } | Inline::Everyone => 0,
            }
        })
        .sum()
}

fn document(document: &Document) -> usize {
    document
        .blocks
        .iter()
        .map(|block| {
            NODE + match block {
                Block::Paragraph { content } | Block::Quote { content } => inline(content),
                Block::Code { text, language } => {
                    text.len() + language.as_ref().map_or(0, String::len)
                }
                Block::List { items, .. } => {
                    items.iter().map(|item| ITEM + inline(item)).sum::<usize>()
                }
                Block::Checklist { items } => items
                    .iter()
                    .map(|item| ITEM + inline(&item.content))
                    .sum::<usize>(),
            }
        })
        .sum()
}

/// The inline nodes of a document, as the server counts them.
fn inline_nodes(document: &Document) -> usize {
    document
        .blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph { content } | Block::Quote { content } => content.len(),
            Block::Code { .. } => 0,
            Block::List { items, .. } => items.iter().map(Vec::len).sum(),
            Block::Checklist { items } => items.iter().map(|item| item.content.len()).sum(),
        })
        .sum()
}

/// The items of the lists of a document.
fn list_items(document: &Document) -> usize {
    document
        .blocks
        .iter()
        .map(|block| match block {
            Block::List { items, .. } => items.len(),
            Block::Checklist { items } => items.len(),
            Block::Paragraph { .. } | Block::Quote { .. } | Block::Code { .. } => 0,
        })
        .sum()
}

/// The weight of a message: the bytes of its texts, and a fixed weight for the message
/// and for each part of its structure. The memory that the message takes is at most a
/// small multiple of it.
pub fn weight(message: &Message) -> usize {
    let texts = message.author_name.as_ref().map_or(0, String::len)
        + message
            .system
            .as_ref()
            .map_or(0, |system| NODE + system.action.len());
    let files: usize = message
        .attachments
        .iter()
        .map(|file| NODE + file.name.len() + file.sha256.len() + file.media_type.len())
        .sum();
    let reactions: usize = message
        .reactions
        .iter()
        .map(|reaction| NODE + reaction.emoji.len())
        .sum();
    std::mem::size_of::<Message>()
        + texts
        + message.document.as_ref().map_or(0, document)
        + files
        + reactions
        + message.reply_authors.len() * 16
}

/// True for a message inside the bounds. The client takes no other message from an
/// answer of a server.
pub fn fits(message: &Message) -> bool {
    message.attachments.len() <= MOST_FILES
        && message.reactions.len() <= MOST_REACTIONS
        && message.document.as_ref().is_none_or(|document| {
            document.blocks.len() <= MOST_BLOCKS
                && inline_nodes(document) <= MOST_INLINE_NODES
                && list_items(document) <= MOST_LIST_ITEMS
        })
        && weight(message) <= MOST_MESSAGE_WEIGHT
}

/// How many of `messages`, counted from the start or from the end, weigh more than
/// `most` together with the ones before them. Those leave a list that holds `most`.
pub(crate) fn excess<'a>(
    messages: impl DoubleEndedIterator<Item = &'a Message> + ExactSizeIterator,
    most: usize,
    from_start: bool,
) -> usize {
    let count = messages.len();
    let mut held = 0usize;
    let mut kept = 0usize;
    let mut add = |message: &Message| {
        held = held.saturating_add(weight(message));
        if held > most {
            return false;
        }
        kept += 1;
        true
    };
    if from_start {
        for message in messages {
            if !add(message) {
                break;
            }
        }
    } else {
        for message in messages.rev() {
            if !add(message) {
                break;
            }
        }
    }
    count - kept
}
