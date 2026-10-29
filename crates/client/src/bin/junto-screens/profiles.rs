use super::{Screen, Studio, acme, acme_with_luca, harbor, id, in_call, person, show_daily};
use junto_client::shell::Open;
use junto_client::state::profile::{ProfileRead, ProfileView};
use junto_client_core::{
    EntryKey, Owner,
    protocol::{
        account::{AccountCard, AccountProfile, ProfileRole, WorkspaceProfileRoles},
        presence::Presence,
    },
};
use std::{collections::HashMap, sync::Arc};

fn owner() -> Owner {
    Owner {
        entry: EntryKey {
            server_id: id(1),
            generation: 1,
        },
        account_id: person("Luca"),
    }
}
fn presence(
    shell: &mut junto_client::shell::Shell,
    cx: &mut gpui::Context<junto_client::shell::Shell>,
) {
    let values = [
        Presence::Online,
        Presence::Idle,
        Presence::DoNotDisturb,
        Presence::Offline,
        Presence::Invisible,
        Presence::Online,
        Presence::Idle,
    ];
    let values: HashMap<_, _> = super::PEOPLE
        .iter()
        .zip(values)
        .map(|(name, value)| (person(name), value))
        .collect();
    shell.state().clone().update(cx, |state, cx| {
        state.show_presence_fixture(owner(), values, cx)
    });
}
fn card(many: bool) -> ProfileRead {
    let roles = if many {
        (0..140)
            .map(|index| ProfileRole {
                color: None,
                id: id::<uuid::Uuid>(500 + index),
                name: format!("Project group {}", index + 1),
                position: index as i32,
            })
            .collect()
    } else {
        vec![ProfileRole {
            color: None,
            id: id(500),
            name: "Editors".to_owned(),
            position: 1,
        }]
    };
    ProfileRead::Shown(Arc::new(AccountCard {
        profile: AccountProfile {
            appearance: Default::default(),
            id: person("Ana"),
            name: "Ana".to_owned(),
            avatar: None,
            departure: None,
        },
        server_roles: vec![ProfileRole {
            color: None,
            id: id(499),
            name: "Moderators".to_owned(),
            position: 1,
        }],
        workspaces: vec![WorkspaceProfileRoles {
            workspace_id: id(10),
            name: "Studio".to_owned(),
            roles,
        }],
    }))
}
pub fn draw(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("profile-before-opening", vec![acme(), harbor()]),
        |shell, window, cx| {
            presence(shell, cx);
            shell.open(Open::Members, window, cx);
        },
    )?;
    for (name, read) in [
        ("profile-card", card(false)),
        ("profile-many-roles", card(true)),
        ("profile-reading", ProfileRead::Reading),
        ("profile-unavailable", ProfileRead::Unavailable),
        (
            "profile-failed",
            ProfileRead::Failed("The server could not be reached.".to_owned()),
        ),
    ] {
        studio.draw(
            Screen::new(name, vec![acme(), harbor()]),
            move |shell, window, cx| {
                presence(shell, cx);
                shell.show_profile_fixture(
                    ProfileView {
                        owner: owner(),
                        account: person("Ana"),
                        read,
                    },
                    window,
                    cx,
                );
            },
        )?;
    }
    studio.draw(
        Screen::call(
            "presence-call",
            in_call(junto_media::calls::Phase::Connected),
        )
        .servers(vec![acme_with_luca(|_| {}), harbor()]),
        |shell, window, cx| {
            presence(shell, cx);
            show_daily(shell, window, cx);
        },
    )
}

pub fn draw_share(studio: &mut Studio) -> Result<(), String> {
    let shared = junto_media::engine::ShareId::of(person("Carla"), 0);
    let media = super::with_call(in_call(junto_media::calls::Phase::Connected), move |call| {
        call.shares = vec![super::seen(
            "Carla",
            junto_media::share::ShareKind::Screen,
            (1920, 1200),
        )];
        call.watched = Some(shared);
    });
    studio.draw(
        Screen::call("presence-share", media).servers(vec![
            super::acme_with_shares(
                &[("Carla", junto_client_core::protocol::call::ShareAudio::None)],
                true,
                |_| {},
            ),
            harbor(),
        ]),
        move |shell, window, cx| {
            presence(shell, cx);
            super::show_pictures((shared, super::desktop_picture(1568, 980)), Vec::new())(
                shell, window, cx,
            );
        },
    )
}
