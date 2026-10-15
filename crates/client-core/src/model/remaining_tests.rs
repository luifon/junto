//! The state after the remaining requests of the client were adopted: the logo of the
//! server (ADR 0101), an invite that lost a workspace (ADR 0111), the description of the
//! avatar of a member (ADR 0119), and the removal of a member from one workspace
//! (ADR 0106).

use junto_protocol::account::{AccountProfile, Avatar, ImageType};
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, InviteId, WorkspaceId};
use junto_protocol::invite::Invite;

use super::tests::{member, scene, time};
use super::*;

fn avatar(hash: &str) -> Avatar {
    Avatar {
        hash: hash.repeat(64),
        media_type: ImageType::Png,
        bytes: 1024,
        width: 64,
        height: 64,
    }
}

fn invite(created_by: AccountId, workspace_ids: Vec<WorkspaceId>) -> Invite {
    Invite {
        created_by_name: Some("Ana".to_owned()),
        id: InviteId::generate(),
        created_by,
        created_at: time(),
        expires_at: time() + time::Duration::days(7),
        max_uses: 5,
        uses: 0,
        workspace_ids,
        revocation: None,
    }
}

#[test]
fn a_snapshot_does_not_state_the_logo_and_an_event_states_it() {
    let scene = scene();
    let mut model = scene.model();
    assert_eq!(model.logo, LogoState::NotStated);

    let set = Event::ServerLogoChanged {
        logo: Some(avatar("a")),
    };
    assert_eq!(model.apply(&set), Follow::default());
    assert_eq!(model.logo, LogoState::Stated(Some(avatar("a"))));
    // A second delivery changes nothing.
    model.apply(&set);
    assert_eq!(model.logo, LogoState::Stated(Some(avatar("a"))));

    model.apply(&Event::ServerLogoChanged {
        logo: Some(avatar("b")),
    });
    assert_eq!(model.logo, LogoState::Stated(Some(avatar("b"))));

    // A removal is stated too: it differs from a logo that nothing stated.
    model.apply(&Event::ServerLogoChanged { logo: None });
    assert_eq!(model.logo, LogoState::Stated(None));

    // A state from a later snapshot starts without a statement again.
    assert_eq!(scene.model().logo, LogoState::NotStated);
}

#[test]
fn an_invite_that_lost_a_workspace_replaces_the_one_in_the_list() {
    let scene = scene();
    let mut model = scene.model();
    let other = WorkspaceId::generate();
    let creator = AccountId::generate();
    let held = invite(creator, vec![scene.workspace_id, other]);
    let untouched = invite(creator, vec![other]);
    model.set_admin(AdminPart::Invites, AdminSet::Loading);
    model.set_admin(
        AdminPart::Invites,
        AdminSet::Loaded(AdminPayload::Invites(vec![held.clone(), untouched.clone()])),
    );

    let mut narrowed = held.clone();
    narrowed.workspace_ids = vec![other];
    let changed = Event::InviteChanged {
        invite: narrowed.clone(),
    };
    assert!(AdminPart::Invites.concerns(&changed));
    assert!(!AdminPart::Accounts.concerns(&changed));
    assert_eq!(model.apply(&changed), Follow::default());

    let invites = model.admin.invites.loaded().unwrap();
    assert_eq!(invites.len(), 2);
    let now = invites.iter().find(|invite| invite.id == held.id).unwrap();
    assert_eq!(now, &narrowed);
    assert_eq!(now.created_by_name.as_deref(), Some("Ana"));
    assert!(invites.contains(&untouched));
    // The newest first, as the list of the server.
    assert!(invites.windows(2).all(|pair| pair[0].id > pair[1].id));

    // An event that arrives while the list is read is applied after the answer.
    let mut model = scene.model();
    model.set_admin(AdminPart::Invites, AdminSet::Loading);
    model.set_admin(
        AdminPart::Invites,
        AdminSet::Loaded(AdminPayload::Invites(vec![held.clone()])),
    );
    model.replay_admin(AdminPart::Invites, [&changed]);
    assert_eq!(model.admin.invites.loaded().unwrap(), &[narrowed]);

    // No reader has the list open: the event changes nothing.
    let mut model = scene.model();
    model.apply(&changed);
    assert!(model.admin.invites.is_not_loaded());
}

#[test]
fn the_description_of_the_avatar_of_a_member_follows_the_profile_of_the_account() {
    let scene = scene();
    let mut model = scene.model();
    let (ana, bea) = (member("Ana"), member("Bea"));
    assert_eq!(ana.avatar, None);
    model.set_members(
        scene.workspace_id,
        Loadable::Loaded(vec![ana.clone(), bea.clone()]),
    );
    let before = model.workspaces[0].clone();

    // The account set a picture: its row of the member list describes it.
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: ana.account_id,
            name: ana.name.clone(),
            avatar: Some(avatar("a")),
        },
    });
    let members = |model: &ServerModel| {
        model
            .workspace(scene.workspace_id)
            .unwrap()
            .members
            .loaded()
            .unwrap()
            .clone()
    };
    let of = |model: &ServerModel, account_id| {
        members(model)
            .into_iter()
            .find(|member| member.account_id == account_id)
            .unwrap()
    };
    assert_eq!(of(&model, ana.account_id).avatar, Some(avatar("a")));
    assert_eq!(of(&model, bea.account_id).avatar, None);
    assert!(!Arc::ptr_eq(&before, &model.workspaces[0]));

    // The same profile again: the workspace is not copied.
    let held = model.workspaces[0].clone();
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: ana.account_id,
            name: ana.name.clone(),
            avatar: Some(avatar("a")),
        },
    });
    assert!(Arc::ptr_eq(&held, &model.workspaces[0]));

    // The picture is removed, and the name changes with it.
    model.apply(&Event::AccountProfileUpdated {
        profile: AccountProfile {
            id: ana.account_id,
            name: "Ana Maria".to_owned(),
            avatar: None,
        },
    });
    let now = of(&model, ana.account_id);
    assert_eq!(now.avatar, None);
    assert_eq!(now.name, "Ana Maria");
}

#[test]
fn a_member_who_was_removed_from_a_workspace_leaves_its_member_list() {
    let scene = scene();
    let mut model = scene.model();
    let (ana, bea) = (member("Ana"), member("Bea"));
    model.set_members(
        scene.workspace_id,
        Loadable::Loaded(vec![ana.clone(), bea.clone()]),
    );
    let removed = Event::WorkspaceMemberRemoved {
        workspace_id: scene.workspace_id,
        account_id: ana.account_id,
    };
    assert!(Part::Members(scene.workspace_id).concerns(&removed, &model));
    assert_eq!(model.apply(&removed), Follow::default());
    let members = model.workspaces[0].members.loaded().unwrap();
    assert_eq!(members.as_slice(), &[bea]);

    // The removal of the own account is followed by `permissions_changed`, which
    // replaces the state (ADR 0106): the event itself takes no workspace away.
    let own = Event::WorkspaceMemberRemoved {
        workspace_id: scene.workspace_id,
        account_id: scene.account_id,
    };
    assert_eq!(model.apply(&own), Follow::default());
    assert!(model.workspace(scene.workspace_id).is_some());
    assert_eq!(
        model.apply(&Event::PermissionsChanged {
            workspace_id: Some(scene.workspace_id)
        }),
        Follow {
            reload: true,
            withdrawn: true,
            load: Vec::new()
        }
    );
}
