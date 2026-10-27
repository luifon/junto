use super::{Access, role_scope, stored_override};
use crate::error::{ApiError, ApiResult};
use junto_authz::{ChannelScope, Request, WorkspacePermission, WorkspaceScope};
use junto_protocol::{
    ids::{CategoryId, ChannelId, WorkspaceId, WorkspaceRoleId},
    structure::Category,
};
use sqlx::PgConnection;
use std::collections::HashMap;

type Rule = (WorkspaceRoleId, bool, i64, i64);

impl Access {
    pub fn category_visible(&self, scope: &ChannelScope, visible_child: bool) -> bool {
        visible_child
            || self.allows(Request::Channel {
                scope,
                permission: WorkspacePermission::ManageChannel,
            })
    }

    pub async fn visible_category(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
    ) -> ApiResult<(Category, ChannelScope)> {
        let workspace = self.workspace(connection, workspace_id).await?;
        self.require(Request::ViewWorkspace(&workspace))?;
        self.categories_with_children(connection, workspace_id, &workspace, &[category_id])
            .await?
            .into_iter()
            .find(|(_, scope, child)| self.category_visible(scope, *child))
            .map(|(category, scope, _)| (category, scope))
            .ok_or_else(ApiError::not_found)
    }

    pub async fn categories_with_children(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
        ids: &[CategoryId],
    ) -> ApiResult<Vec<(Category, ChannelScope, bool)>> {
        let categories = self
            .categories(connection, workspace_id, workspace, Some(ids))
            .await?;
        let children: Vec<ChannelId> = sqlx::query_scalar(
            "select id from channels where category_id=any($1) and workspace_id=$2 and deleted_at is null",
        ).bind(ids).bind(workspace_id).fetch_all(&mut *connection).await?;
        let children = self
            .named_channels(connection, workspace_id, workspace, &children)
            .await?;
        let visible: std::collections::HashSet<_> = children
            .into_iter()
            .filter(|(_, scope)| {
                self.allows(Request::Channel {
                    scope,
                    permission: WorkspacePermission::ViewChannel,
                })
            })
            .filter_map(|(record, _)| record.channel.category_id)
            .collect();
        Ok(categories
            .into_iter()
            .map(|(category, scope)| {
                let child = visible.contains(&category.id);
                (category, scope, child)
            })
            .collect())
    }

    pub async fn categories(
        &self,
        connection: &mut PgConnection,
        workspace_id: WorkspaceId,
        workspace: &WorkspaceScope,
        named: Option<&[CategoryId]>,
    ) -> ApiResult<Vec<(Category, ChannelScope)>> {
        let rows: Vec<(CategoryId, String, i32)> = sqlx::query_as(
            "select id,name,position from categories where workspace_id=$1 and ($2::uuid[] is null or id=any($2)) order by position,id",
        ).bind(workspace_id).bind(named).fetch_all(&mut *connection).await?;
        let ids: Vec<_> = rows.iter().map(|row| row.0).collect();
        let roles: Vec<(CategoryId,WorkspaceRoleId,bool,i64,i64)> = sqlx::query_as(
            "select o.category_id,o.role_id,r.is_everyone,o.allow,o.deny from category_overrides o join workspace_roles r on r.id=o.role_id where o.category_id=any($1) and (r.is_everyone or exists(select 1 from member_roles m where m.role_id=r.id and m.account_id=$2))",
        ).bind(&ids).bind(self.account_id).fetch_all(&mut *connection).await?;
        let people: Vec<(CategoryId,i64,i64)> = sqlx::query_as(
            "select category_id,allow,deny from category_person_overrides where category_id=any($1) and account_id=$2",
        ).bind(&ids).bind(self.account_id).fetch_all(connection).await?;
        let mut rules: HashMap<CategoryId, Vec<Rule>> = HashMap::new();
        for (id, role, everyone, allow, deny) in roles {
            rules
                .entry(id)
                .or_default()
                .push((role, everyone, allow, deny));
        }
        let people: HashMap<_, _> = people
            .into_iter()
            .map(|(id, allow, deny)| Ok((id, stored_override(allow, deny)?)))
            .collect::<ApiResult<_>>()?;
        rows.into_iter()
            .map(|(id, name, position)| {
                let mut scope = role_scope(
                    workspace.clone(),
                    rules.remove(&id).unwrap_or_default().into_iter(),
                )?;
                scope.person_override = people.get(&id).copied().unwrap_or_default();
                Ok((
                    Category {
                        id,
                        workspace_id,
                        name,
                        position,
                    },
                    scope,
                ))
            })
            .collect()
    }
}
