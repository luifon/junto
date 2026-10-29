//! Validation and content checks for the stored message document.

use crate::error::{ApiError, ApiResult};
use junto_protocol::message::{Block, Document, Inline};

pub fn validate(document: &Document) -> ApiResult<Vec<&Inline>> {
    let bytes = serde_json::to_vec(document).map_err(|error| ApiError::internal(&error))?;
    if document.blocks.len() > 100 {
        return Err(ApiError::invalid(
            "a message has at most 16 KiB of formatted text and 100 blocks",
        ));
    }
    let mut nodes = Vec::new();
    for block in &document.blocks {
        match block {
            Block::Forward { server, .. } => {
                let url = reqwest::Url::parse(server.as_str())
                    .map_err(|_| ApiError::invalid("invalid forward origin"))?;
                let public_ip = !server.is_loopback()
                    && url.host_str().is_some_and(|host| {
                        host.trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .is_ok()
                    });
                if server.as_str().len() > 255
                    || public_ip
                    || (server.scheme() != junto_protocol::origin::Scheme::Https
                        && !server.is_loopback())
                {
                    return Err(ApiError::invalid("invalid forward origin"));
                }
            }
            Block::Heading { level, .. } if !(1..=3).contains(level) => {
                return Err(ApiError::invalid("heading level is between 1 and 3"));
            }
            Block::Heading { .. }
            | Block::Small { .. }
            | Block::Paragraph { .. }
            | Block::Quote { .. } => {}
            Block::Code { text, language } => {
                text_ok(text)?;
                if language.as_ref().is_some_and(|language| {
                    language.len() > 32
                        || !language
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte))
                }) {
                    return Err(ApiError::invalid("invalid code language"));
                }
            }
            Block::List { .. } | Block::Checklist { .. } => {}
        }
    }
    for (node, depth) in document.inlines().take(1001) {
        if depth > junto_protocol::message::MAX_INLINE_DEPTH {
            return Err(ApiError::invalid("spoiler nesting is too deep"));
        }
        nodes.push(node);
    }
    if nodes.len() > 1000 {
        return Err(ApiError::invalid("too many inline nodes"));
    }
    let mut mentions = Vec::new();
    for node in &nodes {
        if matches!(
            node,
            Inline::Channel { .. }
                | Inline::Account { .. }
                | Inline::Role { .. }
                | Inline::Everyone
        ) && !mentions.contains(node)
        {
            mentions.push(*node);
        }
    }
    if mentions.len() > 100 {
        return Err(ApiError::invalid("a message has at most 100 mentions"));
    }
    if bytes.len() > 16 * 1024 {
        return Err(ApiError::invalid(
            "a message has at most 16 KiB of formatted text",
        ));
    }
    for node in &nodes {
        match node {
            Inline::Text { text, marks } => {
                text_ok(text)?;
                if marks.len() > 3
                    || marks
                        .iter()
                        .enumerate()
                        .any(|(i, m)| marks[..i].contains(m))
                {
                    return Err(ApiError::invalid("repeated text mark"));
                }
            }
            Inline::Code { text } => text_ok(text)?,
            Inline::Link { text, url } => {
                text_ok(text)?;
                let parsed =
                    reqwest::Url::parse(url).map_err(|_| ApiError::invalid("invalid link"))?;
                if url.len() > 2048
                    || url.chars().any(char::is_control)
                    || !matches!(parsed.scheme(), "https" | "http" | "mailto")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(ApiError::invalid(
                        "links use http, https or mailto without credentials",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(nodes)
}
fn text_ok(text: &str) -> ApiResult<()> {
    if text
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(ApiError::invalid("text contains a control character"));
    }
    Ok(())
}
pub fn has_content(document: &Document) -> bool {
    document.blocks.iter().any(|block| match block {
        Block::Forward { .. } => true,
        Block::Code { text, .. } => !text.trim().is_empty(),
        Block::Heading { content, .. }
        | Block::Small { content }
        | Block::Paragraph { content }
        | Block::Quote { content } => content.iter().any(inline_content),
        Block::List { items, .. } => items.iter().flatten().any(inline_content),
        Block::Checklist { items } => items
            .iter()
            .flat_map(|item| &item.content)
            .any(inline_content),
    })
}
fn inline_content(node: &Inline) -> bool {
    match node {
        Inline::Text { text, .. } | Inline::Code { text } | Inline::Link { text, .. } => {
            !text.trim().is_empty()
        }
        Inline::Spoiler { content } => content.iter().any(inline_content),
        _ => true,
    }
}

pub fn preview_text(document: &Document) -> String {
    fn visible(content: &[Inline], parts: &mut Vec<String>) {
        let mut hidden = None;
        for (node, depth) in junto_protocol::message::inline_nodes(content) {
            if hidden.is_some_and(|level| depth > level) {
                continue;
            }
            hidden = None;
            match node {
                Inline::Spoiler { .. } => {
                    parts.push("Spoiler".to_owned());
                    hidden = Some(depth);
                }
                Inline::Text { text, .. } | Inline::Code { text } | Inline::Link { text, .. } => {
                    parts.push(text.clone())
                }
                _ => {}
            }
        }
    }
    let mut parts = Vec::new();
    for block in &document.blocks {
        match block {
            Block::Paragraph { content }
            | Block::Quote { content }
            | Block::Heading { content, .. }
            | Block::Small { content } => visible(content, &mut parts),
            Block::Code { text, .. } => parts.push(text.clone()),
            Block::List { items, .. } => {
                for item in items {
                    visible(item, &mut parts);
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    visible(&item.content, &mut parts);
                }
            }
            Block::Forward { .. } => {}
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_protocol::message::Mark;

    fn paragraph(text: &str) -> Block {
        Block::Paragraph {
            content: vec![Inline::Text {
                text: text.to_owned(),
                marks: vec![],
            }],
        }
    }
    #[test]
    fn documents_bound_nodes_blocks_and_language_without_flattening_formatting() {
        assert!(
            validate(&Document {
                blocks: vec![paragraph("x"); 100]
            })
            .is_ok()
        );
        assert!(
            validate(&Document {
                blocks: vec![paragraph("x"); 101]
            })
            .is_err()
        );
        assert!(
            validate(&Document {
                blocks: vec![Block::Code {
                    text: "code".into(),
                    language: Some("x".repeat(33))
                }]
            })
            .is_err()
        );
        assert!(
            validate(&Document {
                blocks: vec![Block::Code {
                    text: "code".into(),
                    language: Some("<html>".into())
                }]
            })
            .is_err()
        );
        assert!(
            validate(&Document {
                blocks: vec![Block::Paragraph {
                    content: vec![Inline::Text {
                        text: "x".into(),
                        marks: vec![Mark::Bold, Mark::Bold]
                    }]
                }]
            })
            .is_err()
        );
        assert!(
            validate(&Document {
                blocks: vec![paragraph("\n\tallowed")]
            })
            .is_ok()
        );
        assert!(
            validate(&Document {
                blocks: vec![paragraph("\u{7}control")]
            })
            .is_err()
        );
        assert!(!has_content(&Document {
            blocks: vec![paragraph(" \n\t")]
        }));
    }
    #[test]
    fn inline_and_mention_counts_have_their_own_bounds() {
        let document = Document {
            blocks: vec![Block::Paragraph {
                content: vec![Inline::Everyone; 1001],
            }],
        };
        assert!(
            format!("{:?}", validate(&document).unwrap_err()).contains("too many inline nodes")
        );
        let document = Document {
            blocks: vec![Block::Paragraph {
                content: (0..101)
                    .map(|_| Inline::Account {
                        account_id: junto_protocol::ids::AccountId::generate(),
                    })
                    .collect(),
            }],
        };
        assert!(
            format!("{:?}", validate(&document).unwrap_err())
                .contains("a message has at most 100 mentions")
        );
    }
    #[test]
    fn forward_references_are_content_and_keep_the_document_bounds() {
        let value = serde_json::json!({"blocks": [{
            "kind": "forward", "server": "https://chat.example.test",
            "message_id": "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"
        }]});
        let parsed = serde_json::from_value::<Document>(value);
        assert!(parsed.is_ok(), "{parsed:?}");
        let document = parsed.unwrap();
        assert!(validate(&document).is_ok());
        assert!(has_content(&document));
        let block = document.blocks[0].clone();
        assert!(
            validate(&Document {
                blocks: vec![block.clone(); 100]
            })
            .is_ok()
        );
        assert!(
            validate(&Document {
                blocks: vec![block; 101]
            })
            .is_err()
        );
    }

    #[test]
    fn forward_origins_do_not_accept_paths_or_insecure_remote_servers() {
        for server in ["https://chat.example.test/path", "http://chat.example.test"] {
            let parsed = serde_json::from_value::<Document>(serde_json::json!({"blocks": [{
                "kind": "forward", "server": server,
                "message_id": "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"
            }]}));
            assert!(
                parsed
                    .as_ref()
                    .map_or(true, |document| validate(document).is_err())
            );
        }
    }
}
