use gpui::{AnyElement, App, IntoElement, SharedString};
use junto_client_core::protocol::ids::AccountId;

use super::{
    Shell,
    account_menus::{CallSurface, OpenAccountMenu, account_target},
};
use crate::state::view::{CallPanel, ShellView};

impl OpenAccountMenu {
    pub(super) fn additional_call_present(
        &self,
        surface: CallSurface,
        panel: &CallPanel,
        account: AccountId,
    ) -> bool {
        match surface {
            CallSurface::Hidden => panel
                .people
                .iter()
                .any(|person| person.account_id == account),
            CallSurface::InviteChoice => panel.channel().is_some_and(|channel| {
                channel
                    .outside
                    .iter()
                    .any(|person| person.account_id == account)
            }),
            CallSurface::Presenter => panel
                .room()
                .and_then(|room| room.meeting.as_ref())
                .is_some_and(|meeting| meeting.presenter_id == account),
            CallSurface::RaisedHand => panel
                .room()
                .and_then(|room| room.meeting.as_ref())
                .is_some_and(|meeting| {
                    meeting.by_self && meeting.hands.first().is_some_and(|(id, _)| *id == account)
                }),
            CallSurface::MeetingWaiting => panel
                .room()
                .and_then(|room| room.meeting.as_ref())
                .is_some_and(|meeting| {
                    meeting.by_self
                        && meeting.hands.is_empty()
                        && meeting
                            .waiting
                            .first()
                            .is_some_and(|(id, _)| *id == account)
                }),
            _ => false,
        }
    }
}

impl Shell {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_identity_target(
        &self,
        id: SharedString,
        account: AccountId,
        name: String,
        surface: CallSurface,
        child: impl IntoElement,
        cx: &App,
    ) -> AnyElement {
        let Some(open) = self.call_account_menu(account, name, surface, cx) else {
            return child.into_any_element();
        };
        account_target(
            id,
            Some(open.owner),
            open.people.into_iter().next(),
            open.source,
            child,
            cx,
        )
    }

    pub(super) fn additional_call_visible(
        &self,
        surface: CallSurface,
        account: AccountId,
        view: &ShellView,
        cx: &App,
    ) -> Option<bool> {
        let shown = self.detached || !self.state.read(cx).is_call_detached();
        Some(match surface {
            CallSurface::Hidden => {
                shown
                    && view
                        .lists()
                        .and_then(|lists| lists.call.as_ref())
                        .is_some_and(|panel| {
                            let video = self.people_with_video(&panel.people, cx);
                            panel.hide_without_video
                                && panel
                                    .people
                                    .iter()
                                    .filter(|person| !video.contains(&person.account_id))
                                    .take(3)
                                    .any(|person| person.account_id == account)
                        })
            }
            CallSurface::InviteChoice => {
                let wanted = self.invite_find.read(cx).value().trim().to_lowercase();
                self.invite_open
                    && self.call_tab == super::call::CallTab::Members
                    && self.call_panel_shown(self.call_panel_overlay, cx)
                    && view.lists().is_some_and(|lists| {
                        lists.notices.is_none()
                            && lists
                                .call
                                .as_ref()
                                .and_then(|panel| panel.channel())
                                .is_some_and(|channel| {
                                    channel.room.ready
                                        && channel.outside.iter().any(|person| {
                                            person.account_id == account
                                                && (wanted.is_empty()
                                                    || person.name.to_lowercase().contains(&wanted))
                                        })
                                })
                    })
            }
            CallSurface::Presenter | CallSurface::RaisedHand | CallSurface::MeetingWaiting => shown,
            _ => return None,
        })
    }
}
