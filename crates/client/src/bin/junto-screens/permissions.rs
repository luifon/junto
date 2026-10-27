//! The screens of the permissions of using a channel (ADR 0790): the read-only line in
//! place of the composer, the refused attach and reactions, a channel without its history,
//! the refused join, and a call whose channel withholds sources.

use std::sync::Arc;

use junto_client::shell::call::CallTab;
use junto_client::state::view::ServerSnapshot;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::structure::ChannelPermissions;

use super::messages::{GENERAL, acme_with, general, none, prepare};
use super::{
    Phase, Screen, Studio, acme, acme_with_luca, harbor, id, in_call, select_channel,
    show_daily_tab, with_call,
};

const DAILY: u32 = 34;

/// The server with the list of one channel without these permissions.
fn without(
    mut server: ServerSnapshot,
    channel: u32,
    taken: &[WorkspacePermission],
) -> ServerSnapshot {
    if let Some(model) = &mut server.model {
        let model = Arc::make_mut(model);
        let studio = Arc::make_mut(&mut model.workspaces[0]);
        let permissions = studio
            .permissions
            .iter()
            .copied()
            .filter(|permission| !taken.contains(permission))
            .collect();
        studio
            .channel_permissions
            .retain(|held| held.channel_id != id(channel));
        studio.channel_permissions.push(ChannelPermissions {
            channel_id: id(channel),
            permissions,
        });
    }
    server
}

fn general_without(taken: &[WorkspacePermission]) -> Vec<ServerSnapshot> {
    let timeline = general().timeline();
    vec![
        without(
            acme_with(|model| model.timelines = vec![Arc::new(timeline)]),
            GENERAL,
            taken,
        ),
        harbor(),
    ]
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new(
            "permissions-read-only",
            general_without(&[WorkspacePermission::SendMessages]),
        ),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new(
            "permissions-attach-and-reactions",
            general_without(&[
                WorkspacePermission::AttachFiles,
                WorkspacePermission::AddReactions,
            ]),
        ),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new(
            "permissions-no-history",
            general_without(&[WorkspacePermission::ReadMessageHistory]),
        ),
        |shell, _, cx| prepare(shell, none(), cx),
    )?;
    studio.draw(
        Screen::new(
            "permissions-join-refused",
            vec![
                without(acme(), DAILY, &[WorkspacePermission::Connect]),
                harbor(),
            ],
        ),
        |shell, _, cx| select_channel(shell, "Daily", cx),
    )?;
    let sources = [
        WorkspacePermission::Speak,
        WorkspacePermission::Video,
        WorkspacePermission::ShareScreen,
    ];
    let withheld = with_call(in_call(Phase::Connected), |call| {
        call.withheld = junto_media::calls::Withheld {
            microphone: true,
            camera: true,
            share: true,
            voice_activity: false,
        };
    });
    studio.draw(
        Screen::call("permissions-call-sources", withheld).servers(vec![
            without(acme_with_luca(|_| {}), DAILY, &sources),
            harbor(),
        ]),
        show_daily_tab(CallTab::Members),
    )?;
    let push = with_call(in_call(Phase::Connected), |call| {
        call.withheld.voice_activity = true;
    });
    studio.draw(
        Screen::call("permissions-call-push-to-talk", push).servers(vec![
            without(
                acme_with_luca(|_| {}),
                DAILY,
                &[WorkspacePermission::UseVoiceActivity],
            ),
            harbor(),
        ]),
        show_daily_tab(CallTab::Settings),
    )?;
    Ok(())
}
