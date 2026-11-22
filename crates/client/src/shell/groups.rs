use super::{Layer, Shell};
use crate::text::conversations as words;
use crate::{
    text,
    theme::u,
    ui::{Button, Field, Notice},
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
    /// The first person chosen in the dialog of a new conversation starts a choice of
    /// several people.
    pub(crate) fn start_group_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_group_create(window, cx);
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
    /// Opens the side panel "People" of a group conversation, or closes it (ADR 0741).
    pub(crate) fn toggle_group_people(
        &mut self,
        server: junto_client_core::protocol::ids::ServerId,
        id: ConversationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.members_open {
            self.members_open = false;
            cx.notify();
            return;
        }
        let Some(owner) = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.owner())
        else {
            return;
        };
        self.open_group_details(owner, id, window, cx);
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
        if !matches!(self.layer, Layer::None) {
            self.close_layer(window, cx);
        }
        // The side panel shows one thing.
        self.members_open = true;
        self.state.update(cx, |state, cx| {
            state.close_thread(cx);
            state.close_pins(cx);
            state.close_notices(cx);
            state.hide_search(cx);
            state.close_transcript(cx);
            state.close_followed_threads(cx);
        });
        cx.notify();
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
                Mode::Details(..) => self.members_open,
                _ => matches!(self.layer, Layer::NewDirect),
            }
        {
            return;
        }
        let name = self.groups.name.read(cx).value().trim().to_owned();
        // The name is optional (ADR 0302).
        if !leaving
            && !matches!(mode, Mode::Add(..))
            && (name.chars().count() > 80 || name.chars().any(char::is_control))
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
                        match mode {
                            // The panel stays after a new name; leaving closes it.
                            Mode::Details(..) if leaving => this.members_open = false,
                            Mode::Details(..) => {}
                            _ => this.close_layer(window, cx),
                        }
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
    /// The side panel "People" of a group conversation: its name, its people, "Add people"
    /// and "Leave conversation" (ADR 0741).
    pub(crate) fn render_group_people(
        &mut self,
        owner: Owner,
        id: ConversationId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = crate::theme::theme(cx);
        let Some(group) = self.shown_group(owner, id, cx) else {
            return div().into_any_element();
        };
        let may_add = group.account_ids.len() < 10;
        let mut people = div().flex().flex_col().gap(u(6.0)).px(u(14.0)).pt(u(8.0));
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
        let details = Mode::Details(owner, id);
        let header = crate::ui::SideHeader::new(words::PEOPLE)
            .title_bar()
            .icon(crate::assets::Icon::People)
            .trailing(
                Button::new("close-group-people")
                    .kind(crate::ui::ButtonKind::Ghost)
                    .icon(crate::assets::Icon::X)
                    .tooltip(words::CLOSE_PEOPLE)
                    .selector("close-group-people")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.members_open = false;
                        cx.notify();
                    })),
            );
        let mut body = div()
            .id("group-details")
            .debug_selector(|| "group-details".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(u(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(6.0))
                    .px(u(14.0))
                    .pt(u(12.0))
                    .child(
                        Field::new(&self.groups.name, words::NAME_OPTIONAL).selector("group-name"),
                    )
                    .child(
                        div().flex().child(
                            Button::new("group-rename")
                                .label(words::SAVE_NAME)
                                .selector("group-rename")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.groups.mode = Some(details);
                                    this.save_group(false, window, cx)
                                })),
                        ),
                    ),
            )
            .child(people)
            .child(
                div().flex().px(u(14.0)).child(
                    Button::new("group-add")
                        .label(words::ADD)
                        .icon(crate::assets::Icon::Invite)
                        .selector("group-add")
                        .disabled(busy || !may_add)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !this.group_current(details, cx) {
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
                ),
            );
        body = if self.groups.leaving {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(6.0))
                    .px(u(14.0))
                    .child(Notice::new(words::LEAVE_QUESTION))
                    .child(
                        div().flex().child(
                            Button::new("group-leave-confirm")
                                .label(words::LEAVE)
                                .selector("group-leave-confirm")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.groups.mode = Some(details);
                                    this.save_group(true, window, cx)
                                })),
                        ),
                    ),
            )
        } else {
            body.child(
                div().flex().px(u(14.0)).child(
                    Button::new("group-leave")
                        .label(words::LEAVE)
                        .icon(crate::assets::Icon::Leave)
                        .selector("group-leave")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.groups.leaving = true;
                            cx.notify();
                        })),
                ),
            )
        };
        if let Some(failed) = &self.groups.failed {
            body = body.child(div().px(u(14.0)).child(Notice::error(failed.clone())));
        }
        super::notices::side_panel(
            &theme,
            words::PEOPLE,
            "group-people-panel",
            self.panel_width(
                junto_client_core::settings::Panel::Members,
                theme.metrics.side_panel_width,
                cx,
            ),
        )
        .child(header)
        .child(body)
        .into_any_element()
    }
}
