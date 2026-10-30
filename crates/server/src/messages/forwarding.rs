use junto_protocol::{
    ids::MessageId,
    message::{Block, Document, Inline, Message},
};
use sqlx::PgConnection;

use super::{Purpose, compat, document, uploads};
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    state::AppState,
};

fn freeze(nodes: &mut [Inline], labels: &compat::Labels) {
    for node in nodes {
        if let Some(text) = compat::reference(node, labels) {
            *node = Inline::Text {
                text,
                marks: vec![],
            };
        } else if let Inline::Spoiler { content } = node {
            freeze(content, labels);
        }
    }
}

async fn frozen(
    connection: &mut PgConnection,
    access: &Access,
    mut document: Document,
    author: Option<&Message>,
) -> ApiResult<Document> {
    let mut labels = compat::labels(connection, access, std::slice::from_ref(&document)).await?;
    if let Some((id, name)) =
        author.and_then(|message| message.author_id.zip(message.author_name.as_ref()))
    {
        labels.accounts.insert(id, name.clone());
    }
    for block in &mut document.blocks {
        match block {
            Block::Heading { content, .. }
            | Block::Small { content }
            | Block::Paragraph { content }
            | Block::Quote { content } => freeze(content, &labels),
            Block::List { items, .. } => {
                for item in items {
                    freeze(item, &labels);
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    freeze(&mut item.content, &labels);
                }
            }
            _ => {}
        }
    }
    document::validate(&document)?;
    Ok(document)
}

pub async fn prepare(
    connection: &mut PgConnection,
    app: &AppState,
    access: &Access,
    document: &Document,
) -> ApiResult<(Document, Option<Message>)> {
    let origin = document.blocks.iter().find_map(|block| match block {
        Block::Forward {
            server, message_id, ..
        } => Some((server.clone(), *message_id)),
        _ => None,
    });
    let Some((server, id)) = origin else {
        return Ok((document.clone(), None));
    };
    let source = if app.config.public_origins.contains(&server) {
        let (_, message) = super::get(
            connection,
            access,
            id,
            Purpose::Live(app.streams.live_since(
                access.account_id,
                access.device_id,
                access.session_id,
            )),
        )
        .await?;
        if message.deleted || message.document.is_none() {
            return Err(ApiError::not_found());
        }
        Some(message)
    } else {
        None
    };
    let mut copied = if let Some(source) = &source {
        let mut blocks = vec![Block::Forward {
            server,
            message_id: id,
            verified: true,
        }];
        blocks.extend(
            source
                .document
                .iter()
                .flat_map(Document::content_blocks)
                .cloned(),
        );
        if source.files_removed_by_retention && blocks.len() <= 100 {
            blocks.push(Block::Paragraph { content: vec![Inline::Text { text: "Files of this message were removed after the retention time of this workspace.".into(), marks: vec![] }] });
        }
        Document { blocks }
    } else {
        document.clone()
    };
    if source.is_none() {
        for block in &mut copied.blocks {
            if let Block::Forward { verified, .. } = block {
                *verified = false;
            }
        }
    }
    Ok((
        frozen(connection, access, copied, source.as_ref()).await?,
        source,
    ))
}

pub async fn edited(
    connection: &mut PgConnection,
    access: &Access,
    document: &Document,
) -> ApiResult<Document> {
    if document.forward_origins() == 0 {
        return Ok(document.clone());
    }
    let mut document = document.clone();
    for block in &mut document.blocks {
        if let Block::Forward { verified, .. } = block {
            *verified = false;
        }
    }
    frozen(connection, access, document, None).await
}

pub async fn verify_files(
    connection: &mut PgConnection,
    source: &Message,
    copy: MessageId,
) -> ApiResult<()> {
    let actual = uploads::of_messages(connection, &[copy])
        .await?
        .remove(&copy)
        .unwrap_or_default();
    let same = actual.len() == source.attachments.len()
        && actual
            .iter()
            .zip(&source.attachments)
            .all(|(actual, source)| {
                actual.name == source.name
                    && actual.bytes == source.bytes
                    && actual.sha256 == source.sha256
            });
    if !same {
        return Err(ApiError::invalid(
            "forwarded files must match the source snapshot",
        ));
    }
    Ok(())
}
