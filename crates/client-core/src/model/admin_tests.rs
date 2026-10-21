use junto_protocol::access_token::{AccessTokenInfo, TokenAction};
use junto_protocol::account::{AccountProfile, AccountStatus, AccountSummary};
use junto_protocol::discovery::Retention;
use junto_protocol::events::Event;
use junto_protocol::export::{Export, ExportRemoval, ExportState};
use junto_protocol::ids::{
    AccessTokenId, AccountId, CallInvitationId, CategoryId, ChannelId, ExportId, InviteId,
    JoinRequestId, ServerRoleId, WorkspaceId,
};
use junto_protocol::invite::{Invite, InviteRevocation, JoinRequest, JoinRequestState};
use junto_protocol::roles::ServerRole;
use junto_protocol::structure::{
    ArchivePeriod, ArchivedChannel, Category, ChannelOverride, Workspace,
};
use junto_protocol::{ServerPermission, WorkspacePermission};

use super::message_tests::{general, opened, state};
use super::tests::{Scene, member, scene, time};
use super::*;

fn archived(scene: &Scene, channel_id: ChannelId) -> ArchivedChannel {
    let model = scene.model();
    ArchivedChannel {
        channel: model.channel(channel_id).unwrap().channel.clone(),
        archived_at: time(),
        deletes_at: Some(time() + time::Duration::days(30)),
    }
}

fn account(name: &str) -> AccountSummary {
    AccountSummary {
        id: AccountId::generate(),
        name: name.to_owned(),
        status: AccountStatus::Active,
        is_owner: false,
        server_role_ids: Vec::new(),
        created_at: time(),
    }
}

fn invite(created_by: AccountId) -> Invite {
    Invite {
        created_by_name: None,
        id: InviteId::generate(),
        created_by,
        created_at: time(),
        expires_at: time() + time::Duration::days(7),
        max_uses: 5,
        uses: 0,
        workspace_ids: Vec::new(),
        revocation: None,
    }
}

fn token(created_by: AccountId) -> AccessTokenInfo {
    AccessTokenInfo {
        id: AccessTokenId::generate(),
        name: "setup".to_owned(),
        created_by,
        actions: vec![TokenAction::ReadStructure],
        created_at: time(),
        expires_at: time() + time::Duration::days(30),
    }
}

fn export(scene: &Scene, export_state: ExportState) -> Export {
    Export {
        id: ExportId::generate(),
        place: scene.text_place(),
        state: export_state,
        failure: None,
        requested_at: time(),
        finished_at: None,
        expires_at: None,
        bytes: None,
    }
}

fn invitation(scene: &Scene) -> junto_protocol::call::CallInvitation {
    junto_protocol::call::CallInvitation {
        id: CallInvitationId::generate(),
        call_id: junto_protocol::ids::CallId::generate(),
        place: scene.place(scene.daily),
        account_id: scene.account_id,
        invited_by: AccountId::generate(),
        created_at: time(),
        expires_at: time() + time::Duration::minutes(1),
    }
}

fn loaded(model: &mut ServerModel, part: AdminPart, payload: AdminPayload) {
    model.set_admin(part, AdminSet::Loading);
    model.set_admin(part, AdminSet::Loaded(payload));
}

#[test]
fn a_deleted_workspace_leaves_nothing_of_itself() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=2);
    model.set_message_state(state(scene.text_place(), 0, 2, 2));
    let other = Workspace {
        id: WorkspaceId::generate(),
        name: "Second".to_owned(),
        is_default: false,
    };
    model.apply(&Event::WorkspaceCreated {
        workspace: other.clone(),
    });
    loaded(
        &mut model,
        AdminPart::Overrides(scene.general),
        AdminPayload::Overrides(Vec::new()),
    );
    loaded(
        &mut model,
        AdminPart::Archived(scene.workspace_id),
        AdminPayload::Archived(Vec::new()),
    );
    model.invitations.push(invitation(&scene));

    let follow = model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });

    assert!(!follow.reload);
    assert!(model.workspace(scene.workspace_id).is_none());
    assert_eq!(model.workspaces.len(), 1);
    assert_eq!(model.workspaces[0].workspace, other);
    assert!(model.account.workspace_ids.is_empty());
    assert!(model.timeline(place).is_none());
    assert!(model.message_state(place).is_none());
    assert!(model.invitations.is_empty());
    assert_eq!(
        model.admin.state(AdminPart::Overrides(scene.general)),
        CollectionState::NotLoaded
    );
    assert_eq!(
        model.admin.state(AdminPart::Archived(scene.workspace_id)),
        CollectionState::NotLoaded
    );
    // The event of a workspace that the state does not hold changes nothing.
    let before = model.clone();
    model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    assert_eq!(model, before);
}

#[test]
fn a_deleted_category_leaves_its_channels_without_a_category() {
    let scene = scene();
    let mut model = scene.model();
    let category = Category {
        id: CategoryId::generate(),
        workspace_id: scene.workspace_id,
        name: "Projects".to_owned(),
        position: 0,
    };
    let kept = Category {
        id: CategoryId::generate(),
        workspace_id: scene.workspace_id,
        name: "Rooms".to_owned(),
        position: 1,
    };
    model.apply(&Event::CategoryCreated {
        category: category.clone(),
    });
    model.apply(&Event::CategoryCreated {
        category: kept.clone(),
    });
    for (channel_id, category_id) in [(scene.general, category.id), (scene.daily, kept.id)] {
        let mut changed = model.channel(channel_id).unwrap().channel.clone();
        changed.category_id = Some(category_id);
        model.apply(&Event::ChannelUpdated { channel: changed });
    }

    model.apply(&Event::CategoryDeleted {
        workspace_id: scene.workspace_id,
        category_id: category.id,
    });

    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert_eq!(workspace.categories, std::slice::from_ref(&kept));
    assert_eq!(
        workspace
            .channel(scene.general)
            .unwrap()
            .channel
            .category_id,
        None
    );
    assert_eq!(
        workspace.channel(scene.daily).unwrap().channel.category_id,
        Some(kept.id)
    );
    assert_eq!(workspace.channels.len(), 3);
}

#[test]
fn a_deleted_workspace_role_leaves_the_roles_the_members_and_the_overrides() {
    let scene = scene();
    let mut model = scene.model();
    let mut holder = member("Ana");
    holder.role_ids = vec![scene.helpers];
    model.set_members(scene.workspace_id, Loadable::Loaded(vec![holder.clone()]));
    loaded(
        &mut model,
        AdminPart::Overrides(scene.general),
        AdminPayload::Overrides(vec![
            ChannelOverride {
                role_id: scene.helpers,
                allow: vec![WorkspacePermission::ManageMessages],
                deny: Vec::new(),
            },
            ChannelOverride {
                role_id: scene.everyone,
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ]),
    );
    let part = AdminPart::Overrides(scene.general);
    let event = Event::WorkspaceRoleDeleted {
        workspace_id: scene.workspace_id,
        role_id: scene.helpers,
    };
    assert!(part.concerns(&event));

    model.apply(&event);

    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert!(workspace.roles.iter().all(|role| role.id != scene.helpers));
    assert_eq!(workspace.roles.len(), 2);
    assert!(workspace.own_role_ids.is_empty());
    assert!(workspace.members.loaded().unwrap()[0].role_ids.is_empty());
    let overrides = model.admin.overrides(scene.general).loaded().unwrap();
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].role_id, scene.everyone);
}

#[test]
fn an_archived_channel_leaves_the_list_and_stays_readable_for_its_reader() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=2);
    model.set_message_state(state(scene.text_place(), 0, 2, 2));
    let gone = archived(&scene, scene.general);

    model.apply(&Event::ChannelArchived {
        channel: gone.clone(),
    });

    assert!(model.channel(scene.general).is_none());
    assert_eq!(
        model.workspace(scene.workspace_id).unwrap().channels.len(),
        2
    );
    // The reader keeps the messages, and the state knows the channel as archived.
    assert!(model.timeline(place).is_some());
    assert_eq!(model.admin.archived_channel(scene.general), Some(&gone));
    assert_eq!(model.place(place), Some(scene.text_place()));
    assert!(model.message_state(place).is_some());
    // A change of a message of the archived channel is still followed.
    let system = super::message_tests::message(scene.text_place(), 3, scene.account_id, "line");
    let follow = model.apply(&super::message_tests::changed(&system, false));
    assert!(follow.load.contains(&Part::Message(place, system.id)));

    // The notice of the deletion replaces the archived channel.
    let mut due = gone.clone();
    due.deletes_at = Some(time() + time::Duration::days(7));
    model.apply(&Event::ChannelDeletionDue {
        channel: due.clone(),
    });
    assert_eq!(model.admin.archived_channel(scene.general), Some(&due));

    // A restored channel is a channel of the list again, with the messages of its reader.
    model.apply(&Event::ChannelRestored {
        channel: gone.channel.clone(),
    });
    assert_eq!(
        model.channel(scene.general).map(|held| &held.channel),
        Some(&gone.channel)
    );
    assert!(model.admin.archived_channel(scene.general).is_none());
    assert!(model.timeline(place).is_some());

    // An archived channel that is deleted leaves nothing.
    model.apply(&Event::ChannelArchived {
        channel: gone.clone(),
    });
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(model.admin.archived_channel(scene.general).is_none());
    assert!(model.timeline(place).is_none());
    assert!(model.place(place).is_none());
}

#[test]
fn an_archived_channel_without_a_reader_leaves_nothing_outside_the_archived_list() {
    let scene = scene();
    let place = general(&scene);
    let mut model = scene.model();
    model.set_message_state(state(scene.text_place(), 0, 2, 2));
    let gone = archived(&scene, scene.general);
    model.invitations.push(invitation(&scene));

    model.apply(&Event::ChannelArchived {
        channel: gone.clone(),
    });
    assert!(model.channel(scene.general).is_none());
    assert!(model.admin.archived_channel(scene.general).is_none());
    assert!(model.message_state(place).is_none());
    assert!(model.place(place).is_none());
    // The notice of a deletion of a channel that the state does not know adds nothing.
    model.apply(&Event::ChannelDeletionDue {
        channel: gone.clone(),
    });
    assert!(model.admin.archived_channel(scene.general).is_none());

    // The call of an archived voice channel has ended: what waited for it is gone.
    let voice = archived(&scene, scene.daily);
    model.apply(&Event::ChannelArchived {
        channel: voice.clone(),
    });
    assert!(model.invitations.is_empty());

    // With the archived list open, the list holds the channel.
    let mut model = scene.model();
    loaded(
        &mut model,
        AdminPart::Archived(scene.workspace_id),
        AdminPayload::Archived(Vec::new()),
    );
    let part = AdminPart::Archived(scene.workspace_id);
    let event = Event::ChannelArchived {
        channel: gone.clone(),
    };
    assert!(part.concerns(&event));
    assert!(!AdminPart::Archived(WorkspaceId::generate()).concerns(&event));
    model.apply(&event);
    assert_eq!(
        model.admin.archived(scene.workspace_id).loaded().unwrap(),
        std::slice::from_ref(&gone)
    );
    assert!(model.admin.archived_known.is_empty());
    assert_eq!(model.place(place), Some(scene.text_place()));
    model.apply(&Event::ChannelRestored {
        channel: gone.channel.clone(),
    });
    assert!(
        model
            .admin
            .archived(scene.workspace_id)
            .loaded()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_released_archived_list_keeps_the_channel_of_a_reader_of_its_messages() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=2);
    let part = AdminPart::Archived(scene.workspace_id);
    loaded(&mut model, part, AdminPayload::Archived(Vec::new()));
    let gone = archived(&scene, scene.general);
    let other = archived(&scene, scene.daily);
    for channel in [&gone, &other] {
        model.apply(&Event::ChannelArchived {
            channel: channel.clone(),
        });
    }
    assert_eq!(
        model
            .admin
            .archived(scene.workspace_id)
            .loaded()
            .unwrap()
            .len(),
        2
    );

    model.set_admin(part, AdminSet::NotLoaded);

    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert_eq!(model.admin.archived_known, std::slice::from_ref(&gone));
    assert_eq!(model.place(place), Some(scene.text_place()));
    assert!(model.readable_channel(scene.daily).is_none());
}

#[test]
fn a_reload_carries_the_archived_channel_of_a_reader_until_the_list_answers() {
    let scene = scene();
    let place = general(&scene);
    let (mut before, _) = opened(&scene, 1..=2);
    let gone = archived(&scene, scene.general);
    before.apply(&Event::ChannelArchived {
        channel: gone.clone(),
    });
    let mut snapshot = scene.snapshot.clone();
    snapshot.workspaces[0]
        .channels
        .retain(|held| held.id != scene.general);

    // The snapshot does not name the archived channel.
    let mut model = ServerModel::from_snapshot(snapshot.clone());
    assert!(model.place(place).is_none());
    model.carry_archived(&before, &[place]);
    assert_eq!(model.admin.archived_channel(scene.general), Some(&gone));
    assert_eq!(model.place(place), Some(scene.text_place()));
    model.carry_messages(&before, &[place]);
    model.open_timeline(place);

    // The list of the server holds the channel: the reader keeps the messages.
    let part = AdminPart::Archived(scene.workspace_id);
    let mut held = model.clone();
    loaded(&mut held, part, AdminPayload::Archived(vec![gone.clone()]));
    held.close_lost_archived();
    assert!(held.admin.archived_known.is_empty());
    assert!(held.timeline(place).is_some());

    // The list does not hold it: the channel was deleted while the client was away.
    loaded(&mut model, part, AdminPayload::Archived(Vec::new()));
    model.close_lost_archived();
    assert!(model.admin.archived_channel(scene.general).is_none());
    assert!(model.timeline(place).is_none());

    // A channel that the snapshot holds again is not carried as archived.
    let mut restored = ServerModel::from_snapshot(scene.snapshot.clone());
    restored.carry_archived(&before, &[place]);
    assert!(restored.admin.archived_known.is_empty());
    // Without a reader nothing is carried.
    let mut unread = ServerModel::from_snapshot(snapshot);
    unread.carry_archived(&before, &[]);
    assert!(unread.admin.archived_known.is_empty());
}

#[test]
fn the_collections_of_the_server_follow_their_events_once_they_are_loaded() {
    let scene = scene();
    let mut model = scene.model();
    let before = model.clone();
    let ana = account("Ana");
    let added = Event::AccountAdded {
        account: ana.clone(),
    };
    // A collection that no reader has open follows nothing.
    model.apply(&added);
    assert_eq!(model, before);

    // Accounts.
    loaded(
        &mut model,
        AdminPart::Accounts,
        AdminPayload::Accounts(Vec::new()),
    );
    let bruno = account("Bruno");
    let role_id = ServerRoleId::generate();
    for event in [
        added.clone(),
        added,
        Event::AccountAdded {
            account: bruno.clone(),
        },
        Event::AccountStatusChanged {
            account_id: ana.id,
            status: AccountStatus::Banned,
        },
        Event::AccountServerRolesChanged {
            account_id: bruno.id,
            server_role_ids: vec![role_id],
        },
        Event::OwnerChanged {
            account_id: bruno.id,
            previous_owner_id: None,
        },
        Event::AccountProfileUpdated {
            profile: AccountProfile {
                id: ana.id,
                name: "Ana Lima".to_owned(),
                avatar: None,
            },
        },
    ] {
        assert!(AdminPart::Accounts.concerns(&event), "{}", event.name());
        model.apply(&event);
    }
    let accounts = model.admin.accounts.loaded().unwrap();
    assert_eq!(accounts.len(), 2);
    let held = |id| accounts.iter().find(|held| held.id == id).unwrap();
    assert_eq!(held(ana.id).status, AccountStatus::Banned);
    assert_eq!(held(ana.id).name, "Ana Lima");
    assert!(held(bruno.id).is_owner && !held(ana.id).is_owner);
    assert_eq!(held(bruno.id).server_role_ids, [role_id]);
    assert!(accounts.is_sorted_by_key(|held| held.id));

    // Server roles: a deleted role leaves the accounts too.
    let role = ServerRole {
        position: 1,
        id: role_id,
        name: "inviters".to_owned(),
        permissions: vec![ServerPermission::CreateInvites],
    };
    loaded(
        &mut model,
        AdminPart::ServerRoles,
        AdminPayload::ServerRoles(Vec::new()),
    );
    model.apply(&Event::ServerRoleCreated { role: role.clone() });
    let mut renamed = role.clone();
    renamed.name = "hosts".to_owned();
    model.apply(&Event::ServerRoleUpdated {
        role: renamed.clone(),
    });
    assert_eq!(model.admin.server_roles.loaded().unwrap(), &[renamed]);
    model.account.server_role_ids = vec![role_id];
    let deleted = Event::ServerRoleDeleted { role_id };
    assert!(AdminPart::ServerRoles.concerns(&deleted) && AdminPart::Accounts.concerns(&deleted));
    model.apply(&deleted);
    assert!(model.admin.server_roles.loaded().unwrap().is_empty());
    assert!(model.account.server_role_ids.is_empty());
    let accounts = model.admin.accounts.loaded().unwrap();
    assert!(accounts.iter().all(|held| held.server_role_ids.is_empty()));

    // Invites.
    loaded(
        &mut model,
        AdminPart::Invites,
        AdminPayload::Invites(Vec::new()),
    );
    let (first, second, foreign) = (invite(ana.id), invite(ana.id), invite(bruno.id));
    for held in [&first, &second, &foreign] {
        model.apply(&Event::InviteCreated {
            invite: held.clone(),
        });
    }
    model.apply(&Event::InviteUsed {
        invite_id: first.id,
        uses: 3,
    });
    // A use that is told a second time, out of its order, does not lower the count.
    model.apply(&Event::InviteUsed {
        invite_id: first.id,
        uses: 2,
    });
    model.apply(&Event::InviteRevoked {
        invite_id: second.id,
        revocation: InviteRevocation::Revoked,
    });
    model.apply(&Event::InvitesOfCreatorRevoked {
        created_by: ana.id,
        revocation: InviteRevocation::CreatorBanned,
    });
    let invites = model.admin.invites.loaded().unwrap();
    let held = |id| invites.iter().find(|held| held.id == id).unwrap();
    assert_eq!(held(first.id).uses, 3);
    assert_eq!(
        held(first.id).revocation,
        Some(InviteRevocation::CreatorBanned)
    );
    // The invite that a person revoked keeps its reason.
    assert_eq!(held(second.id).revocation, Some(InviteRevocation::Revoked));
    assert_eq!(held(foreign.id).revocation, None);
    assert!(invites.is_sorted_by_key(|held| std::cmp::Reverse(held.id)));

    // Join requests: a decided request leaves the list of the ones that wait.
    loaded(
        &mut model,
        AdminPart::JoinRequests,
        AdminPayload::JoinRequests(Vec::new()),
    );
    let request = JoinRequest {
        id: JoinRequestId::generate(),
        name: "Carla".to_owned(),
        created_at: time(),
        expires_at: time() + time::Duration::days(7),
    };
    model.apply(&Event::JoinRequestCreated {
        join_request: request.clone(),
    });
    assert_eq!(
        model.admin.join_requests.loaded().unwrap(),
        std::slice::from_ref(&request)
    );
    model.apply(&Event::JoinRequestDecided {
        join_request_id: request.id,
        state: JoinRequestState::Denied,
        account_id: None,
    });
    assert!(model.admin.join_requests.loaded().unwrap().is_empty());

    // Access tokens.
    loaded(
        &mut model,
        AdminPart::AccessTokens,
        AdminPayload::AccessTokens {
            tokens: Vec::new(),
            more: false,
        },
    );
    let (own, of_ana, second_of_ana) = (token(scene.account_id), token(ana.id), token(ana.id));
    for held in [&own, &of_ana, &second_of_ana] {
        model.apply(&Event::AccessTokenCreated {
            access_token: held.clone(),
        });
    }
    assert_eq!(model.admin.access_tokens.loaded().unwrap().len(), 3);
    model.apply(&Event::AccessTokenRevoked {
        access_token_id: own.id,
    });
    assert_eq!(model.admin.access_tokens.loaded().unwrap().len(), 2);
    model.apply(&Event::AccessTokensOfCreatorRevoked { created_by: ana.id });
    assert!(model.admin.access_tokens.loaded().unwrap().is_empty());

    // Exports.
    loaded(
        &mut model,
        AdminPart::Exports,
        AdminPayload::Exports(Vec::new()),
    );
    let mut asked = export(&scene, ExportState::Waiting);
    model.apply(&Event::ExportUpdated {
        export: asked.clone(),
    });
    asked.state = ExportState::Ready;
    asked.bytes = Some(2048);
    model.apply(&Event::ExportUpdated {
        export: asked.clone(),
    });
    assert_eq!(model.admin.exports.loaded().unwrap(), &[asked.clone()]);
    model.apply(&Event::ExportRemoved {
        export_id: asked.id,
        reason: ExportRemoval::Expired,
    });
    assert!(model.admin.exports.loaded().unwrap().is_empty());
}

#[test]
fn the_overrides_of_a_channel_follow_their_events() {
    let scene = scene();
    let mut model = scene.model();
    let part = AdminPart::Overrides(scene.general);
    loaded(&mut model, part, AdminPayload::Overrides(Vec::new()));
    let set = |channel_id, allow: &[WorkspacePermission], deny: &[WorkspacePermission]| {
        Event::ChannelOverrideSet {
            workspace_id: scene.workspace_id,
            channel_id,
            role_id: scene.helpers,
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        }
    };
    let event = set(scene.general, &[WorkspacePermission::ManageMessages], &[]);
    assert!(part.concerns(&event));
    assert!(!AdminPart::Overrides(scene.daily).concerns(&event));
    model.apply(&event);
    // The override of another channel does not change this list.
    model.apply(&set(scene.daily, &[], &[WorkspacePermission::ViewChannel]));
    assert_eq!(
        model.admin.overrides(scene.general).loaded().unwrap(),
        &[ChannelOverride {
            role_id: scene.helpers,
            allow: vec![WorkspacePermission::ManageMessages],
            deny: Vec::new(),
        }]
    );
    // Two empty sets mean that the role has no override.
    model.apply(&set(scene.general, &[], &[]));
    assert!(
        model
            .admin
            .overrides(scene.general)
            .loaded()
            .unwrap()
            .is_empty()
    );
    // The overrides of a deleted channel leave the state.
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
}

#[test]
fn the_events_that_arrived_during_a_read_are_applied_to_a_collection_after_its_answer() {
    let scene = scene();
    let mut model = scene.model();
    let part = AdminPart::Invites;
    model.set_admin(part, AdminSet::Loading);
    assert_eq!(model.admin.state(part), CollectionState::Loading);
    let (answered, later) = (invite(scene.account_id), invite(scene.account_id));
    let kept = [
        Event::InviteCreated {
            invite: later.clone(),
        },
        Event::InviteUsed {
            invite_id: answered.id,
            uses: 1,
        },
        // An event of another collection is not applied by the replay of this one.
        Event::WorkspaceDeleted {
            workspace_id: scene.workspace_id,
        },
    ];
    for event in &kept[..2] {
        model.apply(event);
    }
    assert_eq!(model.admin.state(part), CollectionState::Loading);

    model.set_admin(
        part,
        AdminSet::Loaded(AdminPayload::Invites(vec![answered.clone()])),
    );
    model.replay_admin(part, kept.iter());

    let invites = model.admin.invites.loaded().unwrap();
    assert_eq!(invites.len(), 2);
    assert_eq!(
        invites
            .iter()
            .find(|held| held.id == answered.id)
            .unwrap()
            .uses,
        1
    );
    assert!(model.workspace(scene.workspace_id).is_some());
}

#[test]
fn a_change_of_the_retention_asks_for_the_open_settings_again() {
    let scene = scene();
    let mut model = scene.model();
    let workspace = AdminPart::WorkspaceSettings(scene.workspace_id);
    let changed = Event::RetentionChanged {
        workspace_id: Some(scene.workspace_id),
    };
    let direct = Event::RetentionChanged { workspace_id: None };
    // No reader has the settings open: nothing is read.
    assert_eq!(model.apply(&changed), Follow::default());
    assert_eq!(model.apply(&direct), Follow::default());

    let settings = WorkspaceSettings {
        archive_period: ArchivePeriod { days: Some(30) },
        file_retention: Retention { days: None },
        recording_retention: Retention { days: Some(90) },
    };
    loaded(
        &mut model,
        workspace,
        AdminPayload::WorkspaceSettings(settings.clone()),
    );
    loaded(
        &mut model,
        AdminPart::DirectRetention,
        AdminPayload::DirectRetention(Retention { days: Some(10) }),
    );
    assert_eq!(model.apply(&changed).load, [Part::Admin(workspace)]);
    assert_eq!(
        model.apply(&direct).load,
        [Part::Admin(AdminPart::DirectRetention)]
    );
    assert!(workspace.is_stale_after(&changed) && !workspace.is_stale_after(&direct));
    assert!(AdminPart::DirectRetention.is_stale_after(&direct));
    let other = Event::RetentionChanged {
        workspace_id: Some(WorkspaceId::generate()),
    };
    assert_eq!(model.apply(&other), Follow::default());

    // The settings stay on screen while they are read again.
    model.set_admin(workspace, AdminSet::Loading);
    assert_eq!(
        model.admin.workspace_settings(scene.workspace_id).loaded(),
        Some(&settings)
    );
    // A failed read is in the state with its reason.
    model.set_admin(workspace, AdminSet::Failed(LoadFailure::NotAllowed));
    assert_eq!(
        model.admin.state(workspace),
        CollectionState::Failed(LoadFailure::NotAllowed)
    );
    model.set_admin(workspace, AdminSet::NotLoaded);
    assert!(model.admin.settings.is_empty());
}

#[test]
fn a_role_that_is_deleted_during_a_read_of_the_members_is_not_in_the_answer_afterwards() {
    let scene = scene();
    let mut model = scene.model();
    model.set_members(scene.workspace_id, Loadable::Loading);
    let deleted = Event::WorkspaceRoleDeleted {
        workspace_id: scene.workspace_id,
        role_id: scene.helpers,
    };
    model.apply(&deleted);
    // The answer was made before the deletion.
    let mut holder = member("Ana");
    holder.role_ids = vec![scene.helpers];
    model.set_members(scene.workspace_id, Loadable::Loaded(vec![holder]));
    let part = Part::Members(scene.workspace_id);
    assert!(part.concerns(&deleted, &model));
    model.replay(part, [&deleted]);
    let workspace = model.workspace(scene.workspace_id).unwrap();
    assert!(workspace.members.loaded().unwrap()[0].role_ids.is_empty());
    // The deletion of a role of another workspace does not concern these members.
    let other = Event::WorkspaceRoleDeleted {
        workspace_id: WorkspaceId::generate(),
        role_id: scene.helpers,
    };
    assert!(!part.concerns(&other, &model));
}

#[test]
fn a_reader_of_the_pins_keeps_its_archived_channel() {
    let scene = scene();
    let mut model = scene.model();
    model.open_pins(scene.general);
    let gone = archived(&scene, scene.general);
    model.apply(&Event::ChannelArchived {
        channel: gone.clone(),
    });
    assert_eq!(model.admin.archived_channel(scene.general), Some(&gone));
    assert!(model.pin_list(scene.general).is_some());
    assert!(model.readable_channel(scene.general).is_some());
}

#[test]
fn a_deleted_workspace_leaves_no_notification_setting_of_itself() {
    use junto_protocol::discovery::{NotificationSettings, ScopedNotificationSettings};

    let scene = scene();
    let mut model = scene.model();
    let other = uuid::Uuid::now_v7();
    let setting = |scope_id| ScopedNotificationSettings {
        scope_id,
        settings: NotificationSettings::default(),
    };
    model.notification_settings = vec![
        setting(uuid::Uuid::nil()),
        setting(scene.workspace_id.as_uuid()),
        setting(scene.general.as_uuid()),
        setting(other),
    ];
    model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    let scopes: Vec<uuid::Uuid> = model
        .notification_settings
        .iter()
        .map(|held| held.scope_id)
        .collect();
    assert_eq!(scopes, [uuid::Uuid::nil(), other]);
}

#[test]
fn the_flags_of_the_server_are_held_by_their_read_and_read_again_after_a_change_of_order() {
    use junto_protocol::account::ListedAccount;
    use junto_protocol::roles::{ListedServerRole, ListedWorkspaceRole, RoleActions};
    let scene = scene();
    let mut model = scene.model();
    let (ana, bruno) = (account("Ana"), account("Bruno"));
    let listed = |account: &AccountSummary, may_unban| ListedAccount {
        account: account.clone(),
        may_unban,
    };
    // Unread: no statement.
    assert_eq!(model.admin.may_unban(ana.id), None);
    loaded(
        &mut model,
        AdminPart::Accounts,
        AdminPayload::Accounts(vec![listed(&ana, true), listed(&bruno, false)]),
    );
    assert_eq!(model.admin.may_unban(ana.id), Some(true));
    assert_eq!(model.admin.may_unban(bruno.id), Some(false));

    let role = ServerRole {
        position: 1,
        id: ServerRoleId::generate(),
        name: "hosts".to_owned(),
        permissions: Vec::new(),
    };
    let may = RoleActions {
        may_assign: true,
        may_remove: true,
        may_delete: false,
    };
    loaded(
        &mut model,
        AdminPart::ServerRoles,
        AdminPayload::ServerRoles(vec![ListedServerRole {
            role: role.clone(),
            actions: may.clone(),
        }]),
    );
    assert_eq!(model.admin.server_role_actions(role.id), Some(&may));

    let workspace = AdminPart::RoleActions(scene.workspace_id);
    assert_eq!(
        model
            .admin
            .workspace_role_actions(scene.workspace_id, scene.helpers),
        None
    );
    let helpers = model
        .workspace(scene.workspace_id)
        .unwrap()
        .roles
        .iter()
        .find(|held| held.id == scene.helpers)
        .unwrap()
        .clone();
    loaded(
        &mut model,
        workspace,
        AdminPayload::RoleActions(vec![ListedWorkspaceRole {
            role: helpers.clone(),
            actions: may.clone(),
        }]),
    );
    assert_eq!(
        model
            .admin
            .workspace_role_actions(scene.workspace_id, scene.helpers),
        Some(&may)
    );

    // A ban changes what may be unbanned: the accounts are read again, and only they.
    let banned = Event::AccountStatusChanged {
        account_id: bruno.id,
        status: AccountStatus::Banned,
    };
    let follow = model.apply(&banned);
    assert_eq!(follow.load, [Part::Admin(AdminPart::Accounts)]);
    // Until that read answers, the flags of the accounts state nothing.
    assert_eq!(model.admin.may_unban(ana.id), None);
    loaded(
        &mut model,
        AdminPart::Accounts,
        AdminPayload::Accounts(vec![listed(&ana, true), listed(&bruno, true)]),
    );
    assert_eq!(model.admin.may_unban(bruno.id), Some(true));
    // A change of the order of the server roles reads every flag again.
    let shifted = Event::ServerRoleUpdated { role: role.clone() };
    let follow = model.apply(&shifted);
    for part in [AdminPart::Accounts, AdminPart::ServerRoles, workspace] {
        assert!(follow.load.contains(&Part::Admin(part)), "{part:?}");
    }
    assert_eq!(model.admin.server_role_actions(role.id), None);
    assert_eq!(
        model
            .admin
            .workspace_role_actions(scene.workspace_id, scene.helpers),
        None
    );
    // An override of a channel changes the grant ceiling of the roles of its workspace.
    let overridden = Event::ChannelOverrideSet {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
        role_id: scene.helpers,
        allow: vec![WorkspacePermission::ManageChannel],
        deny: Vec::new(),
    };
    let follow = model.apply(&overridden);
    assert_eq!(follow.load, [Part::Admin(workspace)]);
    // A deleted channel takes its overrides with it, which lifts their denies.
    let deleted = Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    };
    assert!(workspace.is_stale_after(&deleted));
    // The roles of another workspace change nothing here.
    let mut elsewhere = helpers;
    elsewhere.workspace_id = WorkspaceId::generate();
    let follow = model.apply(&Event::WorkspaceRoleUpdated { role: elsewhere });
    assert!(follow.load.is_empty());
    // An answer that names a role of another workspace is not taken.
    let mut foreign = model.workspace(scene.workspace_id).unwrap().roles[0].clone();
    foreign.workspace_id = WorkspaceId::generate();
    loaded(
        &mut model,
        workspace,
        AdminPayload::RoleActions(vec![ListedWorkspaceRole {
            role: foreign.clone(),
            actions: may,
        }]),
    );
    assert_eq!(
        model
            .admin
            .workspace_role_actions(scene.workspace_id, foreign.id),
        None
    );
}

#[test]
fn a_glossary_is_read_again_after_its_event_withdrawn_with_its_workspace_and_gone_with_it() {
    let scene = scene();
    let mut model = scene.model();
    let part = AdminPart::Glossary(scene.workspace_id);
    let replaced = Event::GlossaryReplaced {
        workspace_id: scene.workspace_id,
    };
    // No reader has the glossary open: nothing is read.
    assert_eq!(model.apply(&replaced), Follow::default());

    let terms = vec!["Quorvex".to_owned()];
    loaded(&mut model, part, AdminPayload::Glossary(terms.clone()));
    assert_eq!(model.apply(&replaced).load, [Part::Admin(part)]);
    assert!(part.concerns(&replaced) && part.is_stale_after(&replaced));
    let other = Event::GlossaryReplaced {
        workspace_id: WorkspaceId::generate(),
    };
    assert!(!part.is_stale_after(&other));
    assert_eq!(model.apply(&other), Follow::default());
    assert_eq!(part.workspace(), Some(scene.workspace_id));

    // A change of what the account may see in the workspace shows the glossary as being
    // read until the next state reads it.
    model.withdraw(Some(scene.workspace_id));
    assert_eq!(model.admin.state(part), CollectionState::Loading);
    loaded(&mut model, part, AdminPayload::Glossary(terms));
    model.withdraw(Some(WorkspaceId::generate()));
    assert_eq!(model.admin.state(part), CollectionState::Loaded);

    model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
}

#[test]
fn a_collection_states_when_the_read_that_it_shows_started() {
    let scene = scene();
    let mut model = scene.model();
    let part = AdminPart::Glossary(scene.workspace_id);
    assert_eq!(model.admin.read_times(part), ReadTimes::default());
    model.set_admin(part, AdminSet::Loading);
    let first = model.admin.read_times(part);
    assert!(first.started.is_some() && first.shown.is_none());
    model.set_admin(part, AdminSet::Loaded(AdminPayload::Glossary(Vec::new())));
    assert_eq!(model.admin.read_times(part).shown, first.started);
    // A second read is on its way: the collection still shows the first one.
    model.set_admin(part, AdminSet::Loading);
    let second = model.admin.read_times(part);
    assert!(second.started >= first.started);
    assert_eq!(second.shown, first.started);
    model.set_admin(part, AdminSet::Loaded(AdminPayload::Glossary(Vec::new())));
    assert_eq!(model.admin.read_times(part).shown, second.started);
}
