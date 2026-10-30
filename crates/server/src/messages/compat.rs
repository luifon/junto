use std::collections::{HashMap, HashSet};

use junto_protocol::ids::{AccountId, ChannelId, WorkspaceRoleId};
use junto_protocol::message::{Block, Document, Inline};
use sqlx::PgConnection;

use crate::{access::Access, error::ApiResult};

#[derive(Default)]
pub struct Labels {
    pub accounts: HashMap<AccountId, String>,
    pub roles: HashMap<WorkspaceRoleId, String>,
    pub channels: HashMap<ChannelId, String>,
}

pub async fn labels(
    connection: &mut PgConnection,
    access: &Access,
    documents: &[Document],
) -> ApiResult<Labels> {
    let mut accounts = Vec::new();
    let mut roles = Vec::new();
    let mut channels = Vec::new();
    for (node, _) in documents.iter().flat_map(Document::inlines) {
        match node {
            Inline::Account { account_id } => accounts.push(*account_id),
            Inline::Role { role_id } => roles.push(*role_id),
            Inline::Channel { channel_id } => channels.push(channel_id.as_uuid()),
            _ => {}
        }
    }
    accounts.sort();
    accounts.dedup();
    roles.sort();
    roles.dedup();
    channels.sort();
    channels.dedup();
    let mut labels = Labels::default();
    if !accounts.is_empty() {
        let fellows =
            crate::store::fellow_members(connection, access.account_id, &accounts).await?;
        let contacts = crate::store::contacts(connection, access, &accounts).await?;
        let witnesses: Vec<AccountId> = sqlx::query_scalar(
            "select a.id from accounts a where a.id=any($1) and a.status in ('inactive','deleted')
             and (exists(select 1 from account_departures d join workspace_members m on m.workspace_id=d.workspace_id
                         where d.account_id=a.id and m.account_id=$2)
                  or exists(select 1 from conversations c where (c.low_account_id=a.id and c.high_account_id=$2)
                           or (c.high_account_id=a.id and c.low_account_id=$2)))")
            .bind(&accounts).bind(access.account_id).fetch_all(&mut *connection).await?;
        let witnesses: HashSet<_> = witnesses.into_iter().collect();
        accounts.retain(|id| {
            use junto_authz::{Acquaintance, Contact, Request};
            let standing = if *id == access.account_id {
                Acquaintance::Own
            } else if witnesses.contains(id) {
                Acquaintance::Departed
            } else if fellows.contains(id) {
                Acquaintance::SharedWorkspace
            } else if contacts.get(id) == Some(&Contact::Administrator) {
                Acquaintance::Administrator
            } else {
                Acquaintance::Unrelated
            };
            access.allows(Request::ViewAccount(standing))
        });
        labels.accounts =
            sqlx::query_as("select id,name from accounts where id=any($1) and name is not null")
                .bind(accounts)
                .fetch_all(&mut *connection)
                .await?
                .into_iter()
                .collect();
    }
    if !roles.is_empty() {
        labels.roles = sqlx::query_as("select id,name from workspace_roles where id=any($1)")
            .bind(roles)
            .fetch_all(&mut *connection)
            .await?
            .into_iter()
            .collect();
    }
    if !channels.is_empty() {
        let allowed = crate::access::visibility::load(
            connection,
            &[access.account_id],
            Some(&channels),
            false,
        )
        .await?;
        let ids = allowed
            .into_iter()
            .flat_map(|(_, places)| places.channels)
            .collect::<Vec<_>>();
        labels.channels = sqlx::query_as("select id,name from channels where id=any($1)")
            .bind(ids)
            .fetch_all(connection)
            .await?
            .into_iter()
            .collect();
    }
    Ok(labels)
}

pub fn reference(node: &Inline, labels: &Labels) -> Option<String> {
    Some(match node {
        Inline::Account { account_id } => format!(
            "@{}",
            labels
                .accounts
                .get(account_id)
                .map_or("Unknown person", String::as_str)
        ),
        Inline::Role { role_id } => format!(
            "@{}",
            labels
                .roles
                .get(role_id)
                .map_or("Unknown role", String::as_str)
        ),
        Inline::Channel { channel_id } => format!(
            "#{}",
            labels
                .channels
                .get(channel_id)
                .map_or("channel unavailable", String::as_str)
        ),
        Inline::Everyone => "@channel".into(),
        _ => return None,
    })
}

fn inline(nodes: &[Inline], labels: &Labels) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { text, .. } | Inline::Code { text } | Inline::Link { text, .. } => {
                text.clone()
            }
            Inline::Spoiler { .. } => "Spoiler".into(),
            _ => reference(node, labels).unwrap_or_default(),
        })
        .collect()
}

pub fn legacy(document: &Document, labels: &Labels) -> Document {
    let forwarded = document.forward_origins() > 0;
    let mut lines = Vec::new();
    for block in &document.blocks {
        match block {
            Block::Forward { server, message_id } => {
                lines.push(format!("Forwarded from {server} · message {message_id}"))
            }
            Block::Heading { content, .. }
            | Block::Small { content }
            | Block::Paragraph { content }
            | Block::Quote { content } => lines.push(inline(content, labels)),
            Block::Code { text, .. } => lines.push(text.clone()),
            Block::List { ordered, items } => {
                for (at, nodes) in items.iter().enumerate() {
                    let prefix = if *ordered {
                        format!("{}. ", at + 1)
                    } else {
                        "- ".into()
                    };
                    lines.push(format!("{prefix}{}", inline(nodes, labels)));
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    lines.push(format!(
                        "- [{}] {}",
                        if item.checked { "x" } else { " " },
                        inline(&item.content, labels)
                    ));
                }
            }
        }
    }
    let content = vec![Inline::Text {
        text: lines.join("\n"),
        marks: vec![],
    }];
    Document {
        blocks: vec![if forwarded {
            Block::Quote { content }
        } else {
            Block::Paragraph { content }
        }],
    }
}
