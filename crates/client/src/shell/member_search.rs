use super::Shell;
use crate::{
    assets::Icon,
    state::view::{MemberRow, ShellView},
    text,
    ui::{Button, ButtonKind, Field, FieldEvent, FieldKind},
};
use gpui::{Context, Entity, IntoElement, Subscription, Window};
use gpui_base::input::InputState;
use junto_client_core::protocol::ids::{AccountId, ServerId, WorkspaceId};

pub(super) struct MemberSearch {
    input: Entity<InputState>,
    words: Vec<String>,
    reset_scroll: bool,
    scope: Option<(ServerId, WorkspaceId, AccountId)>,
}
impl MemberSearch {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            input: Field::state(text::FIND_A_MEMBER, window, cx),
            words: Vec::new(),
            reset_scroll: true,
            scope: None,
        }
    }
    pub(super) fn observe(&self, window: &mut Window, cx: &mut Context<Shell>) -> Subscription {
        Field::observe(
            &self.input,
            window,
            cx,
            |shell: &mut Shell, event, _, cx| {
                if event == FieldEvent::Changed {
                    shell.member_search_changed(cx);
                }
            },
        )
    }
    pub(super) fn matches(&self, member: &MemberRow) -> bool {
        if self.words.is_empty() {
            return true;
        }
        let name = member.name.to_lowercase();
        self.words.iter().all(|word| name.contains(word))
    }
    pub(super) fn take_scroll_reset(&mut self) -> bool {
        std::mem::take(&mut self.reset_scroll)
    }
    pub(super) fn active(&self) -> bool {
        !self.words.is_empty()
    }
}
impl Shell {
    pub fn type_member_search(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.member_search.input.update(cx, |input, cx| {
            input.set_value(query.to_owned(), window, cx)
        });
        self.member_search_changed(cx);
    }
    fn member_search_changed(&mut self, cx: &mut Context<Self>) {
        self.member_search.words = self
            .member_search
            .input
            .read(cx)
            .value()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.member_search.reset_scroll = true;
        cx.notify();
    }
    pub(super) fn member_search_scope(
        &mut self,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = view
            .selected
            .as_ref()
            .and_then(|server| Some((server.id, view.workspace()?.id, view.person.account_id?)));
        if self.member_search.scope != scope {
            self.member_search.scope = scope;
            self.member_search.words.clear();
            self.type_member_search("", window, cx);
            self.member_search.reset_scroll = true;
        }
    }
    pub(super) fn member_search_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Field::new(&self.member_search.input, text::FIND_A_MEMBER)
            .kind(FieldKind::Find)
            .selector("member-search")
            .trailing(
                Button::new("member-search-clear")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLEAR_MEMBER_SEARCH)
                    .selector("member-search-clear")
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.type_member_search("", window, cx);
                        shell
                            .member_search
                            .input
                            .update(cx, |input, cx| input.focus(window, cx));
                    })),
            )
            .into_any_element()
    }
}
