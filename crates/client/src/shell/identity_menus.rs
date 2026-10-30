use std::rc::Rc;

use gpui::{App, Context, Div, Stateful, Window};
use junto_client_core::{Owner, protocol::ids::AccountId};

use super::{
    Layer, Shell,
    account_menus::{AccountSource, AccountTarget, OpenAccountMenu},
};
use crate::{
    state::{AppState, view::ShellView},
    text,
    ui::{RowAction, bind_lazy_action_menu},
};

impl OpenAccountMenu {
    pub(super) fn identity_source_present(
        &self,
        view: &ShellView,
        state: &AppState,
    ) -> Option<bool> {
        use crate::state::{
            admin::view::{Listed, Scope, ServerPage},
            view::Members,
        };
        let server_id = self.owner.entry.server_id;
        let present = match self.source.as_ref()? {
            AccountSource::Profile | AccountSource::MentionCandidate { .. } | AccountSource::Transcript { .. } | AccountSource::Invitation(_) => self.additional_identity_present(view),
            AccountSource::Contacts => state.contacts_view().is_some_and(|view| view.server_id == server_id && self.people.iter().all(|person| view.rows.iter().flatten().any(|row| row.account_id == person.account_id))),
            AccountSource::Members(workspace) => view.selected.as_ref().is_some_and(|server| server.id == server_id) && view.workspace().is_some_and(|view| view.id == *workspace && matches!(&view.members, Members::Loaded(rows) if self.people.iter().all(|person| rows.iter().any(|row| row.account_id == person.account_id)))),
            AccountSource::AdminPeople | AccountSource::AdminMembers { .. } => state.admin_view().is_some_and(|view| {
                if view.server_id != server_id { return false; }
                let Scope::Server { page, .. } = &view.scope else { return false; };
                match (&self.source, page) {
                    (Some(AccountSource::AdminPeople), ServerPage::People(page)) => matches!(&page.accounts, Listed::Loaded(rows) if self.people.iter().all(|person| rows.iter().any(|row| row.id == person.account_id && person.departed == (row.status != junto_client_core::protocol::account::AccountStatus::Active)))),
                    (Some(AccountSource::AdminMembers { workspace, addable }), ServerPage::Members(page)) if *workspace == page.workspace_id => {
                        if *addable { matches!(&page.addable, Some(Listed::Loaded(rows)) if self.people.iter().all(|person| rows.iter().any(|row| row.account_id == person.account_id))) }
                        else { matches!(&page.members, crate::state::admin::view::people::MemberList::Loaded(rows) if self.people.iter().all(|person| rows.iter().any(|row| row.account_id == person.account_id))) }
                    }
                    _ => false,
                }
            }),
            _ => return None,
        };
        Some(present)
    }
}

impl Shell {
    pub(super) fn identity_source_visible(
        &self,
        open: &OpenAccountMenu,
        view: &ShellView,
        cx: &App,
    ) -> bool {
        match open.source.as_ref() {
            Some(AccountSource::Contacts) => matches!(self.layer, Layer::NewDirect),
            Some(AccountSource::AdminPeople) => matches!(self.layer, Layer::Admin) && (self.admin.people.filter == super::admin::people::Filter::All || self.state.read(cx).admin_view().is_some_and(|view| {
                matches!(&view.scope, crate::state::admin::view::Scope::Server { page: crate::state::admin::view::ServerPage::People(page), .. } if page.accounts.loaded().is_some_and(|rows| open.people.iter().all(|person| rows.iter().any(|row| row.id == person.account_id && row.status == junto_client_core::protocol::account::AccountStatus::Banned))))
            })),
            Some(AccountSource::AdminMembers { .. }) => matches!(self.layer, Layer::Admin),
            Some(AccountSource::Members(_)) => matches!(self.layer, Layer::None) && self.members_open && view.lists().is_some_and(|lists| lists.call.is_none() && lists.notices.is_none()) && view.workspace().is_some_and(|workspace| workspace.thread.is_none() && workspace.pins.is_none() && workspace.transcript.is_none() && workspace.search.is_none()),
            _ => self.additional_identity_visible(open, view),
        }
    }

    pub(super) fn identity_context(
        &self,
        owner: Owner,
        person: AccountTarget,
        source: AccountSource,
    ) -> OpenAccountMenu {
        OpenAccountMenu {
            owner,
            people: vec![person],
            source: Some(source),
            position: Default::default(),
        }
    }

    pub(super) fn identity_actions(
        &self,
        open: &OpenAccountMenu,
        person: &AccountTarget,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, RowAction)> {
        let mut actions = Vec::new();
        for (id, label, value) in [
            ("account-copy-name", text::COPY_NAME, person.name.clone()),
            (
                "account-copy-id",
                text::COPY_ACCOUNT_ID,
                person.account_id.to_string(),
            ),
        ] {
            let open = open.clone();
            actions.push((
                id,
                RowAction::new(
                    label,
                    cx.listener(move |this, _, window, cx| {
                        if !this.account_context_current(&open, cx) {
                            return;
                        }
                        if matches!(&this.layer, Layer::AccountMenu(current) if **current == open) {
                            this.close_layer(window, cx);
                        }
                        this.copy_text(value.clone(), cx);
                    }),
                ),
            ));
        }
        if person.account_id != open.owner.account_id
            && !person.departed
            && self
                .state
                .read(cx)
                .account_departure(open.owner.entry.server_id, person.account_id)
                .is_none()
        {
            for (id, label, call) in [
                ("account-message", text::message_person(&person.name), false),
                ("account-call", text::DIRECT_CALL.to_owned(), true),
            ] {
                let open = open.clone();
                let account = person.account_id;
                actions.push((
                    id,
                    RowAction::new(
                        label,
                        cx.listener(move |this, _, window, cx| {
                            if !this.account_context_current(&open, cx)
                                || this
                                    .state
                                    .read(cx)
                                    .account_departure(open.owner.entry.server_id, account)
                                    .is_some()
                            {
                                return;
                            }
                            this.close_layer(window, cx);
                            this.state.update(cx, |state, cx| {
                                if call {
                                    state.start_direct_call(
                                        open.owner.entry.server_id,
                                        account,
                                        cx,
                                    );
                                } else {
                                    state.open_conversation(
                                        open.owner.entry.server_id,
                                        account,
                                        cx,
                                    );
                                }
                            });
                        }),
                    ),
                ));
            }
        }
        actions
    }

    pub(super) fn guard_identity_actions(
        &self,
        open: OpenAccountMenu,
        actions: &mut [RowAction],
        cx: &Context<Self>,
    ) {
        for action in actions {
            if let Some(handler) = action.handler.take() {
                let shell = cx.entity().downgrade();
                let open = open.clone();
                action.handler = Some(Rc::new(move |event, window, cx| {
                    if shell
                        .upgrade()
                        .is_some_and(|shell| shell.read(cx).account_context_current(&open, cx))
                    {
                        handler(event, window, cx);
                    }
                }));
            }
        }
    }

    pub(super) fn bind_identity_context(
        &mut self,
        frame: Stateful<Div>,
        surface: &'static str,
        open: OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = gpui::SharedString::from(format!("identity:{surface}:{open:?}"));
        bind_lazy_action_menu(frame, id, window, cx, |_, cx| {
            if !self.account_context_current(&open, cx) {
                return Vec::new();
            }
            open.people
                .iter()
                .flat_map(|person| {
                    self.identity_actions(&open, person, cx)
                        .into_iter()
                        .map(|(_, action)| action)
                })
                .collect()
        })
    }

    pub(super) fn admin_identity_context(
        &self,
        account: AccountId,
        cx: &App,
    ) -> Option<OpenAccountMenu> {
        use crate::state::admin::view::{Scope, ServerPage};
        let state = self.state.read(cx);
        let view = state.admin_view()?;
        let owner = state.server(view.server_id)?.owner()?;
        let Scope::Server { page, .. } = view.scope else {
            return None;
        };
        let (name, departed, source) = match page {
            ServerPage::People(page) => {
                let row = page
                    .accounts
                    .loaded()?
                    .iter()
                    .find(|row| row.id == account)?;
                (
                    row.name.clone(),
                    row.status != junto_client_core::protocol::account::AccountStatus::Active,
                    AccountSource::AdminPeople,
                )
            }
            ServerPage::Members(page) => {
                if let crate::state::admin::view::people::MemberList::Loaded(rows) = &page.members
                    && let Some(row) = rows.iter().find(|row| row.account_id == account)
                {
                    (
                        row.name.clone(),
                        false,
                        AccountSource::AdminMembers {
                            workspace: page.workspace_id,
                            addable: false,
                        },
                    )
                } else {
                    let row = page
                        .addable
                        .as_ref()?
                        .loaded()?
                        .iter()
                        .find(|row| row.account_id == account)?;
                    (
                        row.name.clone(),
                        false,
                        AccountSource::AdminMembers {
                            workspace: page.workspace_id,
                            addable: true,
                        },
                    )
                }
            }
            _ => return None,
        };
        Some(self.identity_context(
            owner,
            AccountTarget {
                account_id: account,
                name,
                departed,
            },
            source,
        ))
    }

    pub(super) fn register_identity_actions(&self, open: &OpenAccountMenu, cx: &mut Context<Self>) {
        for person in &open.people {
            for (_, action) in self.identity_actions(open, person, cx) {
                action.register();
            }
        }
    }
}
