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
            Block::Paragraph { content } | Block::Quote { content } => nodes.extend(content),
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
            Block::List { items, .. } => {
                for item in items {
                    nodes.extend(item);
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    nodes.extend(&item.content);
                }
            }
        }
    }
    if nodes.len() > 1000 {
        return Err(ApiError::invalid("too many inline nodes"));
    }
    let mut mentions = Vec::new();
    for node in &nodes {
        if matches!(
            node,
            Inline::Account { .. } | Inline::Role { .. } | Inline::Everyone
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
        Block::Code { text, .. } => !text.trim().is_empty(),
        Block::Paragraph { content } | Block::Quote { content } => {
            content.iter().any(inline_content)
        }
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
        _ => true,
    }
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
}
