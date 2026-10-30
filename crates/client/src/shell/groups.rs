use super::{Layer, Shell};
use crate::text::conversations as words;
use crate::{
    text,
    theme::u,
    ui::{Button, Dialog, Field, Notice, dialog_footer},
};
use gpui::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, Styled, Window,
    div,
};
use gpui_base::input::InputState;
use junto_client_core::{
    Owner,
    protocol::{
        ids::{AccountId, ConversationId},
        message::GroupConversation,
    },
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Create(Owner),
    Details(Owner, ConversationId),
    Add(Owner, ConversationId),
}
impl Mode {
    fn owner(self) -> Owner {
        match self {
            Self::Create(owner) | Self::Details(owner, _) | Self::Add(owner, _) => owner,
        }
    }
}
pub(crate) struct Groups {
    pub name: Entity<InputState>,
    pub mode: Option<Mode>,
    pub selected: BTreeMap<AccountId, String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub leaving: bool,
    generation: u64,
}
impl Groups {
    pub fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            name: Field::state(words::NAME, window, cx),
            mode: None,
            selected: BTreeMap::new(),
            busy: false,
            failed: None,
            leaving: false,
            generation: 0,
        }
    }
    pub fn reset(&mut self) {
        self.generation += 1;
        self.mode = None;
        self.selected.clear();
        self.busy = false;
        self.failed = None;
        self.leaving = false;
    }
}
impl Shell {
    pub(crate) fn group_current(&self, mode: Mode, cx: &gpui::App) -> bool {
        let owner = mode.owner();
        self.state
            .read(cx)
            .server(owner.entry.server_id)
            .is_some_and(|server| {
                server.owner() == Some(owner)
                    && match mode {
                        Mode::Create(_) => true,
                        Mode::Details(_, id) | Mode::Add(_, id) => {
                            server.live().view().model.is_some_and(|model| {
                                model.group_conversations.iter().any(|group| group.id == id)
                            })
                        }
                    }
            })
    }
    pub(crate) fn open_group_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self
            .state
            .read(cx)
            .contacts_view()
            .map(|view| view.server_id)
        else {
            return;
        };
        let Some(owner) = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.owner())
        else {
            return;
        };
        self.groups.reset();
        self.groups.mode = Some(Mode::Create(owner));
        self.groups
            .name
            .update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }
    pub(crate) fn open_group_details(
        &mut self,
        owner: Owner,
        id: ConversationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = Mode::Details(owner, id);
        if !self.group_current(mode, cx) {
            return;
        }
        let Some(group) = self.shown_group(owner, id, cx) else {
            return;
        };
        self.groups.reset();
        self.groups.mode = Some(mode);
        self.groups
            .name
            .update(cx, |field, cx| field.set_value(group.name, window, cx));
        self.open_layer(Layer::GroupConversation, window, cx);
    }
    fn shown_group(
        &self,
        owner: Owner,
        id: ConversationId,
        cx: &gpui::App,
    ) -> Option<GroupConversation> {
        let server = self.state.read(cx).server(owner.entry.server_id)?;
        if server.owner() != Some(owner) {
            return None;
        }
        server
            .live()
            .view()
            .model?
            .group_conversations
            .iter()
            .find(|group| group.id == id)
            .cloned()
    }
    pub(crate) fn group_candidate(&self, account: AccountId, cx: &gpui::App) -> bool {
        match self.groups.mode {
            Some(Mode::Add(owner, id)) => self
                .shown_group(owner, id, cx)
                .is_some_and(|group| !group.account_ids.contains(&account)),
            _ => true,
        }
    }
    pub(crate) fn toggle_group_person(
        &mut self,
        id: AccountId,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        if self.groups.busy {
            return;
        }
        let Some(mode) = self.groups.mode else {
            return;
        };
        if !self.group_current(mode, cx) {
            return;
        }
        if self.groups.selected.remove(&id).is_none() && self.groups.selected.len() < 9 {
            if matches!(mode, Mode::Add(..)) {
                self.groups.selected.clear();
            }
            self.groups.selected.insert(id, name.to_owned());
        }
        cx.notify();
    }
    pub(crate) fn save_group(
        &mut self,
        leaving: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mode) = self.groups.mode else {
            return;
        };
        if self.groups.busy
            || !self.group_current(mode, cx)
            || !match mode {
                Mode::Details(..) => matches!(self.layer, Layer::GroupConversation),
                _ => matches!(self.layer, Layer::NewDirect),
            }
        {
            return;
        }
        let name = self.groups.name.read(cx).value().trim().to_owned();
        if !leaving
            && !matches!(mode, Mode::Add(..))
            && (name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control))
        {
            self.groups.failed = Some(words::NAME_INVALID.to_owned());
            cx.notify();
            return;
        }
        let accounts: Vec<_> = self.groups.selected.keys().copied().collect();
        if matches!(mode, Mode::Create(_)) && accounts.len() < 2 {
            return;
        }
        let owner = mode.owner();
        let Some(server) = self.state.read(cx).server(owner.entry.server_id) else {
            return;
        };
        let api = server.api_for(owner.account_id);
        self.groups.busy = true;
        self.groups.failed = None;
        let generation = self.groups.generation;
        let navigation = self.state.read(cx).navigation_generation();
        cx.spawn_in(window, async move |this, cx| {
            let answer = match mode {
                Mode::Create(_) => api.create_group(name, accounts).await.map(Some),
                Mode::Details(_, id) if leaving => api.leave_group(id).await.map(|_| None),
                Mode::Details(_, id) => api.rename_group(id, name).await.map(Some),
                Mode::Add(_, id) => {
                    let mut result = Ok(None);
                    for account in accounts {
                        match api.add_group_member(id, account).await {
                            Ok(group) => result = Ok(Some(group)),
                            Err(error) => {
                                result = Err(error);
                                break;
                            }
                        }
                    }
                    result
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.groups.generation != generation
                    || this.groups.mode != Some(mode)
                    || !this.group_current(mode, cx)
                {
                    return;
                }
                this.groups.busy = false;
                match answer {
                    Ok(group) => {
                        this.close_layer(window, cx);
                        if matches!(mode, Mode::Create(_))
                            && let Some(group) = group
                        {
                            this.state.update(cx, |state, cx| {
                                state.select_created_group(owner, group.id, navigation, cx)
                            });
                        }
                    }
                    Err(error) => {
                        this.groups.failed = Some(text::flow_error(
                            &junto_client_core::flow::FlowError::from(&error),
                        ));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn render_group_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Mode::Details(owner, id)) = self.groups.mode else {
            return div().into_any_element();
        };
        let Some(group) = self.shown_group(owner, id, cx) else {
            return div().into_any_element();
        };
        let may_add = group.account_ids.len() < 10;
        let mut people = div().flex().flex_col().gap(u(6.0));
        for account in group.account_ids {
            let name = self
                .state
                .read(cx)
                .group_person_name(owner.entry.server_id, account);
            let departure = self
                .state
                .read(cx)
                .group_person_departure(owner.entry.server_id, account);
            let departed = departure.is_some();
            people = people.child(super::account_menus::account_target(
                format!("group-member-{account}").into(),
                Some(owner),
                Some(super::account_menus::AccountTarget {
                    account_id: account,
                    name: name.clone(),
                    departed,
                }),
                Some(super::account_menus::AccountSource::GroupMember {
                    conversation_id: id,
                    account_id: account,
                }),
                div()
                    .debug_selector(move || format!("group-member-{account}"))
                    .child(match departure {
                        Some(departure) => text::departed_name(&name, departure),
                        None => name,
                    }),
                cx,
            ));
        }
        let busy = self.groups.busy;
        let mut dialog = Dialog::new(words::TITLE, self.layer_focus())
            .selector("group-details")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(Field::new(&self.groups.name, words::NAME).selector("group-name"))
            .child(people)
            .child(
                Button::new("group-rename")
                    .label(words::SAVE_NAME)
                    .selector("group-rename")
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.save_group(false, window, cx)),
                    ),
            )
            .child(
                Button::new("group-add")
                    .label(words::ADD)
                    .selector("group-add")
                    .disabled(busy || !may_add)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.group_current(Mode::Details(owner, id), cx) {
                            return;
                        }
                        this.groups.reset();
                        this.groups.mode = Some(Mode::Add(owner, id));
                        this.contact_part
                            .update(cx, |field, cx| field.set_value("", window, cx));
                        this.state.update(cx, |state, cx| {
                            state.open_contacts(owner.entry.server_id, cx)
                        });
                        this.open_layer(Layer::NewDirect, window, cx);
                    })),
            );
        if self.groups.leaving {
            dialog = dialog.child(Notice::new(words::LEAVE_QUESTION)).child(
                Button::new("group-leave-confirm")
                    .label(words::LEAVE)
                    .selector("group-leave-confirm")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.save_group(true, window, cx))),
            );
        } else {
            dialog = dialog.child(
                Button::new("group-leave")
                    .label(words::LEAVE)
                    .selector("group-leave")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.groups.leaving = true;
                        cx.notify();
                    })),
            );
        }
        if let Some(failed) = &self.groups.failed {
            dialog = dialog.child(Notice::error(failed.clone()));
        }
        dialog
            .child(dialog_footer([Button::new("group-close")
                .label(text::CANCEL)
                .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                .into_any_element()]))
            .into_any_element()
    }
}
