use std::time::{Duration, Instant};

use gpui::{Context, Task};
use junto_client_core::protocol::call::{CallPlace, ReportCallIdle};
use junto_media::calls::{CallTarget, Phase, Seat};

use super::AppState;
use junto_client_core::Owner;
use junto_client_core::protocol::ids::{CallMoveId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::AfkSettings;

#[derive(Clone)]
pub struct AfkAdmin {
    owner: Owner,
    workspace_id: WorkspaceId,
    pub settings: Option<AfkSettings>,
    pub changing: bool,
    pub error: Option<String>,
    refresh: bool,
}

struct AfkMove {
    owner: Owner,
    from: Seat,
    to: CallTarget,
    id: CallMoveId,
    destination_identity: Option<String>,
}

#[derive(Default)]
pub(super) struct AfkState {
    seat: Option<Seat>,
    joined_at: Option<Instant>,
    last_speech: Option<Instant>,
    next_read: Option<Instant>,
    task: Option<Task<()>>,
    moving: Option<AfkMove>,
    notice: Option<(Owner, Seat, CallTarget)>,
    admin: Option<AfkAdmin>,
    admin_task: Option<Task<()>>,
    #[cfg(feature = "test-support")]
    test_idle: Option<(Duration, Duration)>,
}

impl AppState {
    #[cfg(feature = "test-support")]
    pub fn afk_test_pending(&self) -> bool {
        self.afk.task.is_some()
    }
    #[cfg(feature = "test-support")]
    pub fn afk_test_activity(
        &mut self,
        system_idle: Duration,
        microphone_silence: Duration,
        cx: &mut Context<Self>,
    ) {
        self.afk.test_idle = Some((system_idle, microphone_silence));
        self.afk.next_read = None;
        self.afk_tick(cx);
    }
    pub(super) fn afk_speech(&mut self, at: Option<Instant>) {
        self.afk.last_speech = self.afk.last_speech.max(at);
    }
    pub fn supports_afk(&self, server: ServerId) -> bool {
        self.client
            .server(server)
            .is_some_and(|server| server.entry().capabilities.afk_channel)
    }
    pub fn afk_admin(&self, server: ServerId, workspace: WorkspaceId) -> Option<AfkAdmin> {
        self.afk
            .admin
            .as_ref()
            .filter(|admin| {
                Some(admin.owner) == self.owner(server) && admin.workspace_id == workspace
            })
            .cloned()
    }
    pub fn read_afk_admin(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        self.ask_afk_admin(server_id, workspace_id, None, cx);
    }
    pub fn change_afk_admin(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        settings: AfkSettings,
        cx: &mut Context<Self>,
    ) {
        if self
            .afk_admin(server_id, workspace_id)
            .is_some_and(|admin| !admin.changing)
        {
            self.ask_afk_admin(server_id, workspace_id, Some(settings), cx);
        }
    }
    fn ask_afk_admin(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        settings: Option<AfkSettings>,
        cx: &mut Context<Self>,
    ) {
        if !self.supports_afk(server_id) {
            return;
        }
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(owner) = server.owner() else { return };
        let held = self
            .afk_admin(server_id, workspace_id)
            .and_then(|admin| admin.settings);
        let changing = settings.is_some() || held.is_none();
        self.afk.admin = Some(AfkAdmin {
            owner,
            workspace_id,
            settings: held,
            changing,
            error: None,
            refresh: false,
        });
        let api = server.api_for(owner.account_id);
        self.afk.admin_task =
            Some(cx.spawn(async move |this, cx| {
                let result = match settings {
                    Some(settings) => api.set_afk_settings(workspace_id, &settings).await,
                    None => api.afk_settings(workspace_id).await,
                };
                let _ =
                    this.update(cx, |this, cx| {
                        if this.owner(server_id) != Some(owner) {
                            return;
                        }
                        let mut refresh = false;
                        if let Some(admin) = this.afk.admin.as_mut().filter(|admin| {
                            admin.owner == owner && admin.workspace_id == workspace_id
                        }) {
                            admin.changing = false;
                            refresh = admin.refresh;
                            admin.refresh = false;
                            match result {
                                Ok(settings) => admin.settings = Some(settings),
                                Err(error) => {
                                    admin.error = Some(crate::text::flow_error(
                                        &junto_client_core::flow::FlowError::from(&error),
                                    ))
                                }
                            }
                            cx.notify();
                        }
                        this.afk.admin_task = None;
                        if refresh {
                            this.read_afk_admin(server_id, workspace_id, cx);
                        }
                    });
            }));
        cx.notify();
    }

    pub(super) fn afk_reloaded(&mut self, server: ServerId, cx: &mut Context<Self>) {
        let moves = self
            .client
            .server(server)
            .and_then(|server| server.live().view().model)
            .map(|model| {
                model
                    .moves
                    .iter()
                    .filter(|call_move| call_move.afk)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for call_move in moves {
            self.afk_event(
                server,
                &junto_client_core::protocol::events::Event::CallMoveRequested { call_move },
                cx,
            );
        }
        if self
            .calls
            .view()
            .call
            .as_ref()
            .is_some_and(|call| call.target.server_id == server)
        {
            self.afk.next_read = None;
        }
        let owner = self.owner(server);
        let refresh = self
            .afk
            .admin
            .as_mut()
            .filter(|admin| Some(admin.owner) == owner)
            .map(|admin| {
                admin.refresh = true;
                (admin.workspace_id, !admin.changing)
            });
        if let Some((workspace, true)) = refresh {
            self.read_afk_admin(server, workspace, cx);
        }
    }

    pub(super) fn afk_event(
        &mut self,
        server: ServerId,
        event: &junto_client_core::protocol::events::Event,
        cx: &mut Context<Self>,
    ) {
        use junto_client_core::protocol::{call::MoveOutcome, events::Event};
        match event {
            Event::WorkspaceAfkChanged { workspace_id } => {
                if self.calls.view().call.as_ref().is_some_and(|call| call.target.server_id == server && matches!(call.target.place, CallPlace::Channel { workspace_id: current, .. } if current == *workspace_id)) {
                    self.afk.next_read = None;
                }
                let refresh = if let Some(admin) = self.afk.admin.as_mut().filter(|admin| {
                    admin.owner.entry.server_id == server && admin.workspace_id == *workspace_id
                }) {
                    admin.refresh = true;
                    !admin.changing
                } else {
                    false
                };
                if refresh {
                    self.read_afk_admin(server, *workspace_id, cx);
                }
            }
            Event::CallMoveRequested { call_move } if call_move.afk => {
                if let Some(owner) = self
                    .owner(server)
                    .filter(|owner| owner.account_id == call_move.account_id)
                    && let Some(call) = self
                        .calls
                        .view()
                        .call
                        .as_ref()
                        .filter(|call| call.target.server_id == server)
                {
                    if call.target.place == call_move.from {
                        self.afk.moving = Some(AfkMove {
                            owner,
                            from: call.seat(),
                            to: CallTarget {
                                server_id: server,
                                place: call_move.to,
                            },
                            id: call_move.id,
                            destination_identity: call_move.destination_identity.clone(),
                        });
                    } else if call.target.place == call_move.to
                        && call_move.destination_identity.as_deref() == Some(call.identity.as_str())
                    {
                        self.afk.notice = Some((
                            owner,
                            call.seat(),
                            CallTarget {
                                server_id: server,
                                place: call_move.from,
                            },
                        ));
                        cx.notify();
                    }
                }
            }
            Event::CallMoveResolved { move_id, outcome }
                if *outcome != MoveOutcome::Completed
                    && self
                        .afk
                        .moving
                        .as_ref()
                        .is_some_and(|moving| moving.id == *move_id) =>
            {
                self.afk.moving = None;
            }
            _ => {}
        }
        self.afk_changed(cx);
    }
    pub fn afk_notice(&self) -> bool {
        self.afk.notice.as_ref().is_some_and(|(owner, seat, _)| {
            if self.owner(owner.entry.server_id) != Some(*owner) {
                return false;
            }
            self.calls
                .view()
                .call
                .as_ref()
                .is_some_and(|call| call.seat() == *seat)
        })
    }

    pub fn rejoin_before_afk(&mut self, cx: &mut Context<Self>) {
        let Some((owner, seat, target)) = self.afk.notice.clone() else {
            return;
        };
        if self.owner(owner.entry.server_id) != Some(owner)
            || self
                .calls
                .view()
                .call
                .as_ref()
                .is_none_or(|call| call.seat() != seat)
        {
            return;
        }
        if let CallPlace::Channel {
            workspace_id,
            channel_id,
        } = target.place
        {
            self.join_call(target.server_id, workspace_id, channel_id, cx);
        }
        cx.notify();
    }

    pub(super) fn afk_changed(&mut self, cx: &mut Context<Self>) {
        let seat = self.calls.view().call.as_ref().map(|call| call.seat());
        if self
            .afk
            .moving
            .as_ref()
            .is_some_and(|moving| self.owner(moving.owner.entry.server_id) != Some(moving.owner))
        {
            self.afk.moving = None;
        }
        if let Some(moving) = &self.afk.moving
            && let Some(seat) = seat.as_ref().filter(|seat| {
                seat.target == moving.to
                    && moving.destination_identity.as_deref() == Some(seat.identity.as_str())
            })
        {
            self.afk.notice = Some((moving.owner, seat.clone(), moving.from.target));
            self.afk.moving = None;
            cx.notify();
        }
        if self.afk.moving.as_ref().is_some_and(|moving| {
            seat.as_ref().is_some_and(|seat| {
                *seat != moving.from
                    && (seat.target != moving.to
                        || moving.destination_identity.as_deref() != Some(seat.identity.as_str()))
            })
        }) {
            self.afk.moving = None;
        }
        if self.afk.notice.as_ref().is_some_and(|(owner, held, _)| {
            self.owner(owner.entry.server_id) != Some(*owner) || Some(held) != seat.as_ref()
        }) {
            self.afk.notice = None;
        }
        if self.afk.seat != seat {
            self.afk.seat = seat;
            self.afk.joined_at = Some(Instant::now());
            self.afk.next_read = None;
        }
    }

    fn afk_report(
        &mut self,
        seat: &Seat,
        owner: Owner,
        settings: &AfkSettings,
    ) -> Option<ReportCallIdle> {
        if self.owner(owner.entry.server_id) != Some(owner)
            || !settings.enabled
            || ![5, 15, 30, 60].contains(&settings.idle_minutes)
            || seat.target.place.channel_id() == Some(settings.channel_id)
        {
            return None;
        }
        let view = self.calls.view();
        let call = view
            .call
            .as_ref()
            .filter(|call| call.phase == Phase::Connected && call.seat() == *seat)?;
        #[cfg(feature = "test-support")]
        let system_idle = self
            .afk
            .test_idle
            .map(|(idle, _)| idle)
            .or_else(|| self.system_idle_duration());
        #[cfg(not(feature = "test-support"))]
        let system_idle = self.system_idle_duration();
        let idle = system_idle?;
        let server = self.client.server(call.target.server_id)?;
        let live = server.live().view();
        let listed = live.model.as_ref()?.call_at(&call.target.place)?;
        if listed.meeting.is_some() {
            return None;
        }
        let joined = self.afk.joined_at?;
        let last_speech = self
            .calls
            .levels()
            .borrow()
            .last_microphone_speech
            .max(self.afk.last_speech)
            .unwrap_or(joined)
            .max(joined);
        let silence = last_speech.elapsed();
        #[cfg(feature = "test-support")]
        let silence = self.afk.test_idle.map_or(silence, |(_, silence)| silence);
        let threshold = Duration::from_secs(u64::from(settings.idle_minutes) * 60);
        if idle < threshold || silence < threshold {
            return None;
        }
        Some(ReportCallIdle {
            call_id: listed.id,
            identity: seat.identity.clone(),
            system_idle_seconds: u32::try_from(idle.as_secs()).unwrap_or(u32::MAX),
            microphone_silent_seconds: u32::try_from(silence.as_secs()).unwrap_or(u32::MAX),
        })
    }

    pub(super) fn afk_tick(&mut self, cx: &mut Context<Self>) {
        self.afk_changed(cx);
        if self.afk.task.is_some() || self.afk.next_read.is_some_and(|at| at > Instant::now()) {
            return;
        }
        let view = self.calls.view();
        let Some(call) = view
            .call
            .as_ref()
            .filter(|call| call.phase == Phase::Connected)
        else {
            return;
        };
        let CallPlace::Channel { workspace_id, .. } = call.target.place else {
            return;
        };
        let Some(server) = self.client.server(call.target.server_id) else {
            return;
        };
        if !server.entry().capabilities.afk_channel {
            return;
        }
        let Some(owner) = server.owner() else { return };
        let seat = call.seat();
        self.afk.next_read = Some(Instant::now() + Duration::from_secs(30));
        let api = server.api_for(owner.account_id);
        self.afk.task = Some(cx.spawn(async move |this, cx| {
            let result = async {
                let settings = api.afk_settings(workspace_id).await.ok()?;
                let body = this
                    .update(cx, |this, _| this.afk_report(&seat, owner, &settings))
                    .ok()??;
                api.report_call_idle(&body).await.ok()
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.afk.task = None;
                if this.owner(owner.entry.server_id) != Some(owner) {
                    return;
                }
                if let Some(moved) = result
                    && this
                        .calls
                        .view()
                        .call
                        .as_ref()
                        .is_some_and(|call| call.seat() == seat)
                {
                    this.afk.moving = Some(AfkMove {
                        owner,
                        from: seat,
                        to: CallTarget {
                            server_id: owner.entry.server_id,
                            place: moved.to,
                        },
                        id: moved.id,
                        destination_identity: moved.destination_identity,
                    });
                    this.afk_changed(cx);
                }
            });
        }));
    }
}
