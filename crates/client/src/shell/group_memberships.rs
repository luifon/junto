use super::{Layer, Shell};
use crate::{
    text,
    text::conversations as words,
    theme::u,
    ui::{Button, Dialog, Notice, dialog_footer},
};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::{Owner, api::ErrorCategory, protocol::message::GroupMembership};

#[derive(Default)]
pub(super) struct Memberships {
    owner: Option<Owner>,
    generation: u64,
    navigation: u64,
    membership_epoch: u64,
    entries: Vec<GroupMembership>,
    target: Option<GroupMembership>,
    busy: bool,
    failed: Option<String>,
}

impl Memberships {
    pub(super) fn reset(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}

impl Shell {
    pub(super) fn group_memberships_current(&self, cx: &gpui::App) -> bool {
        let Some(owner) = self.memberships.owner else {
            return false;
        };
        let state = self.state.read(cx);
        matches!(self.layer, Layer::GroupMemberships)
            && state.navigation_generation() == self.memberships.navigation
            && state.group_membership_generation(owner.entry.server_id)
                == self.memberships.membership_epoch
            && state
                .server(owner.entry.server_id)
                .is_some_and(|server| server.owner() == Some(owner))
    }

    pub(crate) fn open_group_memberships(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self
            .state
            .read(cx)
            .contacts_view()
            .and_then(|view| self.state.read(cx).server(view.server_id))
            .and_then(|server| server.owner())
        else {
            return;
        };
        self.memberships.reset();
        self.memberships.owner = Some(owner);
        self.memberships.navigation = self.state.read(cx).navigation_generation();
        self.memberships.membership_epoch = self
            .state
            .read(cx)
            .group_membership_generation(owner.entry.server_id);
        self.open_layer(Layer::GroupMemberships, window, cx);
        self.load_group_memberships(window, cx);
    }

    fn load_group_memberships(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.memberships.busy
            || self.memberships.target.is_some()
            || !self.group_memberships_current(cx)
        {
            return;
        }
        let Some(owner) = self.memberships.owner else {
            return;
        };
        let Some(server) = self.state.read(cx).server(owner.entry.server_id) else {
            return;
        };
        let api = server.api_for(owner.account_id);
        self.layer_focus.focus(window, cx);
        self.memberships.generation = self.memberships.generation.wrapping_add(1);
        let generation = self.memberships.generation;
        self.memberships.busy = true;
        self.memberships.failed = None;
        self.memberships.target = None;
        cx.spawn_in(window, async move |this, cx| {
            let answer = api.group_memberships().await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.memberships.generation != generation || !this.group_memberships_current(cx)
                {
                    return;
                }
                this.memberships.busy = false;
                match answer {
                    Ok(mut entries) => {
                        let mut ids = std::collections::HashSet::new();
                        entries.retain(|entry| ids.insert(entry.id));
                        this.memberships.entries = entries;
                    }
                    Err(error) => {
                        this.memberships.failed = Some(text::flow_error(
                            &junto_client_core::flow::FlowError::from(&error),
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn leave_group_membership(
        &mut self,
        generation: u64,
        target: GroupMembership,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.memberships.busy
            || self.memberships.target.as_ref().map(|held| held.id) != Some(target.id)
            || membership_entry(self, generation, target.id).is_none()
            || !self.group_memberships_current(cx)
        {
            return;
        }
        let Some(owner) = self.memberships.owner else {
            return;
        };
        let Some(server) = self.state.read(cx).server(owner.entry.server_id) else {
            return;
        };
        let api = server.api_for(owner.account_id);
        self.memberships.busy = true;
        self.memberships.failed = None;
        cx.spawn_in(window, async move |this, cx| {
            let answer = api.leave_group(target.id).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.memberships.generation != generation || !this.group_memberships_current(cx)
                {
                    return;
                }
                this.memberships.busy = false;
                match answer {
                    Ok(()) => {
                        this.close_layer(window, cx);
                        this.focus.focus(window, cx);
                    }
                    Err(error) if error.category() == Some(ErrorCategory::NotFound) => {
                        this.close_layer(window, cx);
                        this.focus.focus(window, cx);
                    }
                    Err(error) => {
                        this.memberships.failed = Some(text::flow_error(
                            &junto_client_core::flow::FlowError::from(&error),
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_group_memberships(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let generation = self.memberships.generation;
        let busy = self.memberships.busy;
        let mut dialog = Dialog::new(words::MEMBERSHIPS_TITLE, self.layer_focus())
            .selector("group-memberships")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }));
        if let Some(target) = self.memberships.target.clone() {
            dialog = dialog
                .child(
                    div()
                        .id("membership-question")
                        .h(u(120.0))
                        .overflow_y_scroll()
                        .child(text::label(&target.name))
                        .child(Notice::new(words::LEAVE_QUESTION)),
                )
                .child(dialog_footer(
                    [
                        Button::new(SharedString::from(format!(
                            "membership-back:{generation}:{}",
                            target.id
                        )))
                        .label(text::CANCEL)
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.memberships.busy
                                    || this.memberships.generation != generation
                                    || !this.group_memberships_current(cx)
                                {
                                    return;
                                }
                                this.layer_focus.focus(window, cx);
                                this.memberships.target = None;
                                this.memberships.failed = None;
                                cx.notify();
                            },
                        )),
                        Button::new(SharedString::from(format!(
                            "membership-confirm:{generation}:{}",
                            target.id
                        )))
                        .label(words::LEAVE)
                        .selector("membership-leave-confirm")
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.leave_group_membership(generation, target.clone(), window, cx)
                            },
                        )),
                    ]
                    .map(|button| button.into_any_element()),
                ));
        } else {
            let mut list = div()
                .id("membership-list")
                .debug_selector(|| "group-memberships-list".to_owned())
                .h(u(200.0))
                .flex()
                .flex_col()
                .gap(u(6.0))
                .overflow_y_scroll();
            if self.memberships.entries.is_empty() && !busy && self.memberships.failed.is_none() {
                list = list.child(Notice::new(words::MEMBERSHIPS_EMPTY));
            }
            for entry in self.memberships.entries.clone() {
                let id = entry.id;
                list = list.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(text::label(&entry.name)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .debug_selector(move || format!("membership-leave:{id}"))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "membership-leave:{generation}:{id}"
                                    )))
                                    .label(words::LEAVE)
                                    .disabled(busy || self.memberships.failed.is_some())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.memberships.busy
                                            || this.memberships.target.is_some()
                                            || membership_entry(this, generation, id).is_none()
                                            || !this.group_memberships_current(cx)
                                        {
                                            return;
                                        }
                                        this.layer_focus.focus(window, cx);
                                        this.memberships.target = Some(entry.clone());
                                        this.memberships.failed = None;
                                        cx.notify();
                                    })),
                                ),
                        ),
                );
            }
            dialog = dialog.child(list).child(dialog_footer(
                [
                    Button::new(SharedString::from(format!("membership-close:{generation}")))
                        .label(text::CANCEL)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.memberships.generation == generation
                                && this.group_memberships_current(cx)
                            {
                                this.close_layer(window, cx);
                            }
                        })),
                    Button::new(SharedString::from(format!(
                        "membership-refresh:{generation}"
                    )))
                    .label(words::MEMBERSHIPS_REFRESH)
                    .selector("membership-refresh")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.memberships.generation == generation {
                            this.load_group_memberships(window, cx);
                        }
                    })),
                ]
                .map(|button| button.into_any_element()),
            ));
        }
        let status = if let Some(failed) = &self.memberships.failed {
            Notice::error(failed.clone())
        } else {
            Notice::new(if busy {
                words::MEMBERSHIPS_LOADING
            } else {
                words::MEMBERSHIPS_HELP
            })
        };
        dialog
            .child(
                div()
                    .id("membership-status")
                    .h(u(60.0))
                    .overflow_y_scroll()
                    .child(status),
            )
            .into_any_element()
    }
}

fn membership_entry(
    shell: &Shell,
    generation: u64,
    id: junto_client_core::protocol::ids::ConversationId,
) -> Option<&GroupMembership> {
    (shell.memberships.generation == generation).then_some(())?;
    shell
        .memberships
        .entries
        .iter()
        .find(|entry| entry.id == id)
}
