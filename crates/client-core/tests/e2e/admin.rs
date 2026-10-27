//! The administration of the client core against the real server: the structure, archived
//! channels, the deletion of a workspace, and the collections that a reader opens
//! (ADR 0760).

use std::sync::Arc;

use junto_client_core::ServerHandle;
use junto_client_core::api::ErrorCategory;
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::live::{LoadError, Notice, StreamStatus};
use junto_client_core::model::{
    AdminPart, Collection, CollectionState, LoadFailure, PlaceId, ServerModel,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::access_token::{
    ACCESS_TOKEN_PREFIX, CreateAccessToken, TokenAction,
};
use junto_client_core::protocol::account::AccountStatus;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, ChannelId, InviteId};
use junto_client_core::protocol::invite::{
    ApproveJoinRequest, CreateInvite, InviteRevocation, JoinRequestState,
};
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::RuleSubject;
use junto_client_core::protocol::structure::{
    ArchivePeriod, CreateCategory, PermissionRule, SetChannelOverride, UpdateChannel,
};

use crate::messages::{both_open, document, message_when, texts};
use crate::stream::{
    Pair, agrees_with_a_snapshot, arrived, events, live, model_when, pair, text_channel,
    told_until, when,
};
use crate::support::{Device, TestServer, logged_in};

/// An owner and a member on one server, each with a running event stream.
async fn scene(name: &str) -> Pair {
    let server = TestServer::start(name).await;
    let address = server.address();
    let scene = pair(server, &address).await;
    live(&scene.first, &scene.owner).await;
    live(&scene.second, &scene.member).await;
    scene
}

fn message(text: &str) -> SendMessage {
    SendMessage {
        document: document(text),
        reply_to: None,
        attachments: Vec::new(),
    }
}

/// True when one of the notices is an event for which `holds` is true.
fn told_of(notices: &[Notice], holds: impl Fn(&Event) -> bool) -> bool {
    events(notices).into_iter().any(|(_, event)| holds(event))
}

/// True when the stream replaced the state by a snapshot.
fn reloaded(notices: &[Notice]) -> bool {
    notices
        .iter()
        .any(|notice| matches!(notice, Notice::Reloaded { .. }))
}

/// Opens a collection that the server refuses to the account. The reader gets the
/// refusal, and the state holds the failure until the reader releases the collection.
/// Returns the state with the failure.
async fn refused(handle: &ServerHandle, part: AdminPart) -> Arc<ServerModel> {
    match handle.live().open_admin(part).await {
        Err(LoadError::Request(error)) => {
            assert_eq!(error.category(), Some(ErrorCategory::NotAllowed), "{error}");
        }
        other => panic!("the collection was not refused: {other:?}"),
    }
    let failed = model_when(handle, "the failed collection", |model| {
        model.admin.state(part) == CollectionState::Failed(LoadFailure::NotAllowed)
    })
    .await;
    handle.live().release_admin(part);
    model_when(handle, "the released collection", |model| {
        model.admin.state(part) == CollectionState::NotLoaded
    })
    .await;
    failed
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_follows_a_new_channel_a_new_category_a_rename_a_move_and_a_deleted_category() {
    let scene = scene("Structure").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let mut told = member.live().notices();

    let plans = text_channel(owner, workspace_id, "plans").await;
    let projects = CreateCategory {
        name: "Projects".to_owned(),
        position: None,
    };
    let category = owner
        .api()
        .create_category(workspace_id, &projects)
        .await
        .unwrap();
    let model = model_when(member, "the new channel", |model| {
        model.channel(plans.id).is_some()
    })
    .await;
    assert_eq!(model.channel(plans.id).unwrap().channel, plans);
    assert!(
        !model
            .workspace(workspace_id)
            .unwrap()
            .categories
            .contains(&category)
    );

    let rename = UpdateChannel {
        name: Some("roadmap".to_owned()),
        ..UpdateChannel::default()
    };
    owner.api().update_channel(plans.id, &rename).await.unwrap();
    model_when(member, "the new name of the channel", |model| {
        model
            .channel(plans.id)
            .is_some_and(|held| held.channel.name == "roadmap")
    })
    .await;

    let moved = UpdateChannel {
        category_id: Some(Some(category.id)),
        ..UpdateChannel::default()
    };
    owner.api().update_channel(plans.id, &moved).await.unwrap();
    model_when(
        member,
        "the channel and its newly visible category",
        |model| {
            model
                .channel(plans.id)
                .is_some_and(|held| held.channel.category_id == Some(category.id))
                && model
                    .workspace(workspace_id)
                    .is_some_and(|held| held.categories.contains(&category))
        },
    )
    .await;

    owner
        .api()
        .delete_category(workspace_id, category.id)
        .await
        .unwrap();
    let model = model_when(member, "the channel without a category", |model| {
        let categories = &model.workspace(workspace_id).unwrap().categories;
        categories.iter().all(|held| held.id != category.id)
            && model
                .channel(plans.id)
                .is_some_and(|held| held.channel.category_id.is_none())
    })
    .await;
    assert_eq!(model.channel(plans.id).unwrap().channel.name, "roadmap");

    let told = told_until(&mut told, "the category visibility refreshes", |notices| {
        notices
            .iter()
            .filter(|notice| matches!(notice, Notice::Reloaded { .. }))
            .count()
            >= 2
    })
    .await;
    let created_channel = |event: &Event| {
        matches!(event, Event::ChannelCreated { channel }
            if channel.id == plans.id)
    };
    let created_category = |event: &Event| {
        matches!(event, Event::CategoryCreated { category: created }
            if created.id == category.id)
    };
    let was_renamed = |event: &Event| {
        matches!(event, Event::ChannelUpdated { channel }
            if channel.id == plans.id
                && channel.name == "roadmap"
                && channel.category_id.is_none())
    };
    let was_moved = |event: &Event| {
        matches!(event, Event::ChannelUpdated { channel }
            if channel.id == plans.id && channel.category_id == Some(category.id))
    };
    assert!(told_of(&told, created_channel), "{told:?}");
    assert!(!told_of(&told, created_category), "{told:?}");
    assert!(told_of(&told, was_renamed), "{told:?}");
    assert!(told_of(&told, was_moved), "{told:?}");
    assert!(reloaded(&told), "{told:?}");
    for handle in [owner, member] {
        agrees_with_a_snapshot(handle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archived_channel_leaves_the_list_stays_readable_where_it_is_open_and_returns() {
    let scene = scene("Archive").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let general = text_channel(owner, workspace_id, "general").await;
    let unread = text_channel(owner, workspace_id, "unread").await;
    let place = PlaceId::Channel(general.id);
    both_open(&scene, place).await;
    owner
        .api()
        .send_message(place, &message("before the archive"))
        .await
        .unwrap();
    message_when(member, place, "before the archive").await;
    model_when(member, "the second channel", |model| {
        model.channel(unread.id).is_some()
    })
    .await;
    let mut told = member.live().notices();

    let archived = owner.api().archive_channel(general.id).await.unwrap();
    assert_eq!(archived.channel.id, general.id);
    owner.api().archive_channel(unread.id).await.unwrap();

    // The member has the messages of the first channel open and not those of the second.
    let model = model_when(member, "no archived channel in the list", |model| {
        model.channel(general.id).is_none() && model.channel(unread.id).is_none()
    })
    .await;
    assert_eq!(texts(&model, place), ["before the archive"]);
    let known = model.admin.archived_channel(general.id).unwrap();
    assert_eq!(known.channel.id, general.id);
    assert_eq!(known.deletes_at, archived.deletes_at);
    assert!(model.readable_channel(general.id).is_some());
    assert!(model.admin.archived_channel(unread.id).is_none());
    assert!(model.readable_channel(unread.id).is_none());
    assert!(model.timeline(PlaceId::Channel(unread.id)).is_none());

    // An archived channel takes no message.
    let refusal = owner
        .api()
        .send_message(place, &message("after the archive"))
        .await
        .unwrap_err();
    assert_eq!(refusal.category(), Some(ErrorCategory::Conflict));

    owner.api().restore_channel(general.id).await.unwrap();
    let model = model_when(member, "the restored channel in the list", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    assert!(model.admin.archived_channel(general.id).is_none());
    assert!(model.channel(unread.id).is_none());
    assert_eq!(texts(&model, place), ["before the archive"]);
    owner
        .api()
        .send_message(place, &message("after the restore"))
        .await
        .unwrap();
    message_when(member, place, "after the restore").await;

    // The archive and the restore reached the member as events, and the state was not
    // read again.
    let mut seen = told_until(&mut told, "the restored channel", |notices| {
        told_of(notices, |event| {
            matches!(event, Event::ChannelRestored { channel }
                if channel.id == general.id)
        })
    })
    .await;
    seen.extend(arrived(&mut told).await);
    let was_archived = |event: &Event| {
        matches!(event, Event::ChannelArchived { channel }
            if channel.channel.id == general.id)
    };
    assert!(told_of(&seen, was_archived), "{seen:?}");
    assert!(!reloaded(&seen), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_archived_list_follows_an_archive_and_a_restore_and_is_dropped_when_released() {
    let scene = scene("Archived").await;
    let (owner, workspace_id) = (&scene.owner, scene.workspace_id);
    let general = text_channel(owner, workspace_id, "general").await;
    model_when(owner, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    let part = AdminPart::Archived(workspace_id);
    let listed = |model: &ServerModel| -> Option<Vec<ChannelId>> {
        let list = model.admin.archived(workspace_id).loaded()?;
        Some(list.iter().map(|held| held.channel.id).collect())
    };

    owner.live().open_admin(part).await.unwrap();
    model_when(owner, "the empty archived list", |model| {
        listed(model) == Some(Vec::new())
    })
    .await;

    let archived = owner.api().archive_channel(general.id).await.unwrap();
    let model = model_when(owner, "the archived channel in the list", |model| {
        listed(model) == Some(vec![general.id])
    })
    .await;
    assert!(model.channel(general.id).is_none());
    let held = model.admin.archived_channel(general.id).unwrap();
    assert_eq!(held.deletes_at, archived.deletes_at);
    // The workspace has the archive period of a new workspace: the channel has a time
    // of deletion.
    assert!(held.deletes_at.is_some());
    let on_server = owner.api().archived_channels(workspace_id).await.unwrap();
    assert_eq!(
        model.admin.archived(workspace_id).loaded(),
        Some(&on_server)
    );

    owner.api().restore_channel(general.id).await.unwrap();
    model_when(owner, "the empty list after the restore", |model| {
        listed(model) == Some(Vec::new()) && model.channel(general.id).is_some()
    })
    .await;

    owner.live().release_admin(part);
    model_when(owner, "the archived list is not held", |model| {
        model.admin.archived(workspace_id).is_not_loaded()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_gets_a_second_workspace_and_loses_all_of_it_when_it_is_deleted() {
    let scene = scene("Workspaces").await;
    let (owner, member) = (&scene.owner, &scene.member);

    let second = owner
        .api()
        .create_workspace("Second".to_owned())
        .await
        .unwrap();
    assert!(!second.is_default);
    let lobby = text_channel(owner, second.id, "lobby").await;
    owner
        .api()
        .add_workspace_member(second.id, logged_in(member))
        .await
        .unwrap();
    let model = model_when(member, "the second workspace with its channel", |model| {
        model
            .workspace(second.id)
            .is_some_and(|workspace| workspace.channel(lobby.id).is_some())
    })
    .await;
    assert!(model.account.workspace_ids.contains(&second.id));
    assert_eq!(model.workspace(second.id).unwrap().workspace, second);

    // The member holds messages of the second workspace.
    let place = PlaceId::Channel(lobby.id);
    member.live().open_messages(place).await.unwrap();
    member
        .live()
        .send_message(place, document("in the second workspace"), None)
        .unwrap();
    message_when(member, place, "in the second workspace").await;

    // The default workspace is not deleted.
    let refusal = owner
        .api()
        .delete_workspace(scene.workspace_id)
        .await
        .unwrap_err();
    assert_eq!(refusal.category(), Some(ErrorCategory::Conflict));

    owner.api().delete_workspace(second.id).await.unwrap();
    let model = model_when(member, "the state without the second workspace", |model| {
        model.workspace(second.id).is_none()
    })
    .await;
    assert!(model.channel(lobby.id).is_none());
    assert!(model.readable_channel(lobby.id).is_none());
    assert!(model.timeline(place).is_none());
    assert!(model.message_state(place).is_none());
    assert!(!model.account.workspace_ids.contains(&second.id));
    assert!(model.workspace(scene.workspace_id).is_some());
    for handle in [owner, member] {
        model_when(handle, "one workspace", |model| model.workspaces.len() == 1).await;
        agrees_with_a_snapshot(handle).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_invites_follow_a_creation_and_a_revocation_and_a_member_does_not_read_them() {
    let scene = scene("Invites").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let revocation = |model: &ServerModel, invite_id: InviteId| {
        let invites = model.admin.invites.loaded()?;
        let invite = invites.iter().find(|held| held.id == invite_id)?;
        Some(invite.revocation)
    };

    owner.live().open_admin(AdminPart::Invites).await.unwrap();
    // The invite with which the member joined.
    model_when(owner, "the invites", |model| {
        model
            .admin
            .invites
            .loaded()
            .is_some_and(|invites| invites.len() == 1)
    })
    .await;

    let created = owner
        .api()
        .create_invite(&CreateInvite::default())
        .await
        .unwrap();
    let invite_id = created.invite.id;
    let model = model_when(owner, "the new invite", |model| {
        revocation(model, invite_id) == Some(None)
    })
    .await;
    // The newest invite is the first.
    let invites = model.admin.invites.loaded().unwrap();
    assert_eq!(invites.len(), 2);
    assert_eq!(invites[0], created.invite);

    owner.api().revoke_invite(invite_id).await.unwrap();
    model_when(owner, "the revoked invite", |model| {
        revocation(model, invite_id) == Some(Some(InviteRevocation::Revoked))
    })
    .await;

    // The member holds no permission to create invites.
    let failed = refused(member, AdminPart::Invites).await;
    assert_eq!(
        failed.admin.invites,
        Collection::Failed(LoadFailure::NotAllowed)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_accounts_show_a_ban_and_an_unban_and_the_ban_ends_the_stream_of_the_member() {
    let scene = scene("Accounts").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let (owner_id, member_id) = (logged_in(owner), logged_in(member));
    let status = |model: &ServerModel, account_id: AccountId| {
        let accounts = model.admin.accounts.loaded()?;
        let account = accounts.iter().find(|held| held.id == account_id)?;
        Some(account.status)
    };

    owner.live().open_admin(AdminPart::Accounts).await.unwrap();
    let model = model_when(owner, "the two accounts", |model| {
        status(model, owner_id) == Some(AccountStatus::Active)
            && status(model, member_id) == Some(AccountStatus::Active)
    })
    .await;
    let accounts = model.admin.accounts.loaded().unwrap();
    assert_eq!(accounts.len(), 2);
    let owners: Vec<AccountId> = accounts
        .iter()
        .filter(|held| held.is_owner)
        .map(|held| held.id)
        .collect();
    assert_eq!(owners, [owner_id]);

    owner.api().ban_account(member_id).await.unwrap();
    let view = when(member, "off", |view| view.status == StreamStatus::Off).await;
    assert!(view.model.is_none());
    assert!(matches!(member.access().state, AccessState::Banned { .. }));
    model_when(owner, "the banned member", |model| {
        status(model, member_id) == Some(AccountStatus::Banned)
    })
    .await;

    owner.api().unban_account(member_id).await.unwrap();
    let model = model_when(owner, "the member is active again", |model| {
        status(model, member_id) == Some(AccountStatus::Active)
    })
    .await;
    let on_server = owner.api().accounts().await.unwrap();
    assert_eq!(model.admin.accounts.loaded(), Some(&on_server));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_join_requests_follow_a_request_and_its_approval_and_the_account_appears() {
    let scene = scene("Requests").await;
    let owner = &scene.owner;
    owner
        .live()
        .open_admin(AdminPart::JoinRequests)
        .await
        .unwrap();
    owner.live().open_admin(AdminPart::Accounts).await.unwrap();
    model_when(owner, "no join request and two accounts", |model| {
        model
            .admin
            .join_requests
            .loaded()
            .is_some_and(|requests| requests.is_empty())
            && model
                .admin
                .accounts
                .loaded()
                .is_some_and(|accounts| accounts.len() == 2)
    })
    .await;

    let third = Device::new();
    let asking = third.add(&scene.server.address()).await;
    let request = AccessAction::RequestAccess {
        name: "Linus".to_owned(),
    };
    asking.dispatch(request).await.unwrap();
    let AccessState::WaitingForApproval {
        join_request_id: Some(join_request_id),
        ..
    } = asking.access().state
    else {
        panic!("not waiting");
    };
    let model = model_when(owner, "the join request", |model| {
        model
            .admin
            .join_requests
            .loaded()
            .is_some_and(|requests| !requests.is_empty())
    })
    .await;
    let requests = model.admin.join_requests.loaded().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        (requests[0].id, requests[0].name.as_str()),
        (join_request_id, "Linus")
    );

    // An approval that names no workspace grants the default workspace.
    let decided = owner
        .api()
        .approve_join_request(join_request_id, &ApproveJoinRequest::default())
        .await
        .unwrap();
    assert_eq!(decided.state, JoinRequestState::Approved);
    let account_id = decided.account_id.unwrap();
    let model = model_when(owner, "no join request and the new account", |model| {
        model
            .admin
            .join_requests
            .loaded()
            .is_some_and(|requests| requests.is_empty())
            && model
                .admin
                .accounts
                .loaded()
                .is_some_and(|accounts| accounts.iter().any(|held| held.id == account_id))
    })
    .await;
    let accounts = model.admin.accounts.loaded().unwrap();
    let account = accounts.iter().find(|held| held.id == account_id).unwrap();
    assert_eq!(
        (account.name.as_str(), account.status, account.is_owner),
        ("Linus", AccountStatus::Active, false)
    );
    assert_eq!(accounts.len(), 3);
    let members = owner
        .api()
        .workspace_members(scene.workspace_id)
        .await
        .unwrap();
    assert!(members.iter().any(|held| held.account_id == account_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_access_tokens_follow_a_creation_and_a_revocation_and_a_member_creates_none() {
    let scene = scene("Tokens").await;
    let (owner, member) = (&scene.owner, &scene.member);
    let tokens = |model: &ServerModel| model.admin.access_tokens.loaded().cloned();

    owner
        .live()
        .open_admin(AdminPart::AccessTokens)
        .await
        .unwrap();
    model_when(owner, "no access token", |model| {
        tokens(model) == Some(Vec::new())
    })
    .await;

    let wanted = CreateAccessToken {
        name: "setup".to_owned(),
        actions: vec![TokenAction::ReadStructure],
        expires_at: time::OffsetDateTime::now_utc() + time::Duration::days(1),
    };
    let created = owner.api().create_access_token(&wanted).await.unwrap();
    // The answer to the creation is the one place that carries the token.
    assert!(created.token.expose().starts_with(ACCESS_TOKEN_PREFIX));
    assert_eq!(created.access_token.created_by, logged_in(owner));
    let model = model_when(owner, "the new access token", |model| {
        tokens(model).is_some_and(|tokens| !tokens.is_empty())
    })
    .await;
    let held = tokens(&model).unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, created.access_token.id);
    assert_eq!(held[0].name, "setup");
    assert_eq!(held[0].actions, [TokenAction::ReadStructure]);
    assert!(!model.admin.access_tokens_more);

    owner
        .api()
        .revoke_access_token(created.access_token.id)
        .await
        .unwrap();
    model_when(owner, "no access token after the revocation", |model| {
        tokens(model) == Some(Vec::new())
    })
    .await;

    // Only an administrator creates an access token.
    let refusal = member.api().create_access_token(&wanted).await.unwrap_err();
    assert_eq!(refusal.category(), Some(ErrorCategory::NotAllowed));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_override_hides_a_channel_from_a_member_and_its_removal_shows_the_channel_again() {
    let scene = scene("Overrides").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let plans = text_channel(owner, workspace_id, "plans").await;
    for handle in [owner, member] {
        model_when(handle, "the channel", |model| {
            model.channel(plans.id).is_some()
        })
        .await;
    }
    let part = AdminPart::Rules(plans.id);
    let overrides = |model: &ServerModel| {
        model
            .admin
            .rules(plans.id)
            .loaded()
            .map(|held| held.rules.clone())
    };

    // The member holds no permission to manage the channel: the server refuses the
    // overrides. What the member holds in the channel is what the snapshot states.
    let failed = refused(member, part).await;
    assert_eq!(
        *failed.admin.rules(plans.id),
        Collection::Failed(LoadFailure::NotAllowed)
    );
    assert!(failed.holds_in(plans.id, WorkspacePermission::ViewChannel));
    assert!(!failed.holds_in(plans.id, WorkspacePermission::ManageChannel));

    owner.live().open_admin(part).await.unwrap();
    let model = model_when(owner, "no override", |model| {
        overrides(model) == Some(Vec::new())
    })
    .await;
    assert!(model.holds_in(plans.id, WorkspacePermission::ManageChannel));
    let everyone = model
        .workspace(workspace_id)
        .unwrap()
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;

    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    owner
        .api()
        .set_channel_rule(plans.id, RuleSubject::Role { role_id: everyone }, &hide)
        .await
        .unwrap();
    let hidden = PermissionRule {
        subject: RuleSubject::Role { role_id: everyone },
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    model_when(owner, "the override", |model| {
        overrides(model) == Some(vec![hidden.clone()])
    })
    .await;
    model_when(member, "the channel is hidden", |model| {
        model.channel(plans.id).is_none()
    })
    .await;

    // Two empty sets remove the override.
    owner
        .api()
        .set_channel_rule(
            plans.id,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride::default(),
        )
        .await
        .unwrap();
    model_when(owner, "no override after the removal", |model| {
        overrides(model) == Some(Vec::new())
    })
    .await;
    model_when(member, "the channel is shown again", |model| {
        model.channel(plans.id).is_some()
    })
    .await;

    owner.live().release_admin(part);
    model_when(owner, "the overrides are not held", |model| {
        model.admin.rules(plans.id).is_not_loaded()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_export_follows_its_states_and_its_file_is_read_and_removed() {
    use futures_util::StreamExt as _;
    use junto_client_core::protocol::export::ExportState;

    let scene = scene("Exports").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let general = text_channel(owner, workspace_id, "general").await;
    model_when(member, "the channel", |model| {
        model.channel(general.id).is_some()
    })
    .await;
    owner
        .api()
        .send_message(PlaceId::Channel(general.id), &message("kept in the export"))
        .await
        .unwrap();
    owner.live().open_admin(AdminPart::Exports).await.unwrap();

    // The member holds no permission to export the channel.
    let refusal = member.api().export_channel(general.id).await.unwrap_err();
    assert_eq!(refusal.category(), Some(ErrorCategory::NotAllowed));

    // The export appears in the open collection and becomes ready without a read of the
    // list by the test.
    let asked = owner.api().export_channel(general.id).await.unwrap();
    let ready = model_when(owner, "the ready export", |model| {
        model.admin.exports.loaded().is_some_and(|exports| {
            exports
                .iter()
                .any(|held| held.id == asked.id && held.state == ExportState::Ready)
        })
    })
    .await;
    let export = ready
        .admin
        .exports
        .loaded()
        .unwrap()
        .iter()
        .find(|held| held.id == asked.id)
        .unwrap()
        .clone();
    assert_eq!(export.place, asked.place);
    let bytes = export.bytes.unwrap();
    assert!(bytes > 0 && export.expires_at.is_some());

    // The file is a ZIP archive of the stated size that holds the message.
    let mut body = owner.api().export_file(asked.id, 0).await.unwrap().body;
    let mut file = Vec::new();
    while let Some(part) = body.next().await {
        file.extend_from_slice(&part.unwrap());
    }
    assert_eq!(file.len() as u64, bytes);
    assert!(file.starts_with(b"PK"));
    let text = String::from_utf8_lossy(&file);
    assert!(text.contains("kept in the export"));
    // The export of one account is no export of another one.
    let foreign = member.api().export_file(asked.id, 0).await;
    assert_eq!(
        foreign.err().and_then(|error| error.category()),
        Some(ErrorCategory::NotFound)
    );

    owner.api().remove_export(asked.id).await.unwrap();
    model_when(owner, "no export", |model| {
        model
            .admin
            .exports
            .loaded()
            .is_some_and(|exports| exports.is_empty())
    })
    .await;
    let gone = owner.api().export(asked.id).await.unwrap_err();
    assert_eq!(gone.category(), Some(ErrorCategory::NotFound));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_of_a_workspace_are_read_again_after_a_change_of_the_archive_period() {
    let scene = scene("Settings").await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let part = AdminPart::WorkspaceSettings(workspace_id);
    let period = |model: &ServerModel| {
        let settings = model.admin.workspace_settings(workspace_id).loaded()?;
        Some(settings.archive_period)
    };

    owner.live().open_admin(part).await.unwrap();
    // A new workspace deletes an archived channel after 30 days.
    model_when(owner, "the archive period of a new workspace", |model| {
        period(model) == Some(ArchivePeriod { days: Some(30) })
    })
    .await;

    let longer = ArchivePeriod { days: Some(60) };
    let set = owner
        .api()
        .set_archive_period(workspace_id, longer)
        .await
        .unwrap();
    assert_eq!(set, longer);
    // The event names the workspace and not the new value: the client reads the settings
    // again.
    let model = model_when(owner, "the new archive period", |model| {
        period(model) == Some(longer)
    })
    .await;
    let settings = model
        .admin
        .workspace_settings(workspace_id)
        .loaded()
        .unwrap();
    assert_eq!(
        settings.file_retention,
        owner.api().file_retention(workspace_id).await.unwrap()
    );
    assert_eq!(
        settings.recording_retention,
        owner.api().recording_retention(workspace_id).await.unwrap()
    );

    // Only an administrator reads the settings of a workspace.
    let failed = refused(member, part).await;
    assert_eq!(
        *failed.admin.workspace_settings(workspace_id),
        Collection::Failed(LoadFailure::NotAllowed)
    );
}
