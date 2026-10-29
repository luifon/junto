use super::AppState;
use crate::text;
use gpui::{Context, EntityId, Task};
use junto_client_core::{
    Owner,
    api::ErrorCategory,
    flow::FlowError,
    protocol::{
        account::AccountCard,
        ids::{AccountId, ServerId},
    },
};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug)]
pub enum ProfileRead {
    Reading,
    Shown(Arc<AccountCard>),
    Unavailable,
    Failed(String),
}
#[derive(Clone, Debug)]
pub struct ProfileView {
    pub owner: Owner,
    pub account: AccountId,
    pub read: ProfileRead,
}
impl ProfileView {
    pub fn can_contact(&self) -> bool {
        self.account != self.owner.account_id && matches!(self.read, ProfileRead::Shown(_))
    }
}
struct Card {
    view: ProfileView,
    number: u64,
    task: Option<Task<()>>,
}
#[derive(Default)]
pub(super) struct Profiles {
    cards: HashMap<EntityId, Card>,
    sequence: u64,
    refresh: Option<Task<()>>,
    #[cfg(feature = "test-support")]
    fixtures: HashMap<EntityId, ProfileView>,
}
impl AppState {
    pub fn profile_view(&self, window: EntityId) -> Option<ProfileView> {
        #[cfg(feature = "test-support")]
        if self.fixture.is_some()
            && let Some(view) = self.profiles.fixtures.get(&window)
        {
            return Some(view.clone());
        }
        let card = self.profiles.cards.get(&window)?;
        (self.owner(card.view.owner.entry.server_id) == Some(card.view.owner))
            .then(|| card.view.clone())
    }
    #[cfg(feature = "test-support")]
    pub fn show_profile_fixture(
        &mut self,
        window: EntityId,
        view: ProfileView,
        cx: &mut Context<Self>,
    ) {
        self.profiles.fixtures.insert(window, view);
        cx.notify();
    }
    pub fn open_profile(
        &mut self,
        window: EntityId,
        server: ServerId,
        account: AccountId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self.owner(server) else {
            return false;
        };
        self.profiles.cards.insert(
            window,
            Card {
                view: ProfileView {
                    owner,
                    account,
                    read: ProfileRead::Reading,
                },
                number: 0,
                task: None,
            },
        );
        self.read_profile(window, false, cx);
        self.follow_profile_cards(cx);
        self.want_people(cx);
        cx.notify();
        true
    }
    pub fn close_profile(&mut self, window: EntityId, cx: &mut Context<Self>) {
        self.profiles.cards.remove(&window);
        if self.profiles.cards.is_empty() {
            self.profiles.refresh = None;
        }
        #[cfg(feature = "test-support")]
        self.profiles.fixtures.remove(&window);
        self.want_people(cx);
        cx.notify();
    }
    pub fn retry_profile(&mut self, window: EntityId, cx: &mut Context<Self>) {
        self.read_profile(window, false, cx);
        cx.notify();
    }
    fn follow_profile_cards(&mut self, cx: &mut Context<Self>) {
        if self.profiles.refresh.is_some() {
            return;
        }
        self.profiles.refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let again = this.update(cx, |this, cx| {
                    let windows: Vec<_> = this
                        .profiles
                        .cards
                        .iter()
                        .filter(|(_, card)| {
                            card.task.is_none() && matches!(card.view.read, ProfileRead::Shown(_))
                        })
                        .map(|(window, _)| *window)
                        .collect();
                    for window in windows {
                        this.read_profile(window, true, cx);
                    }
                    !this.profiles.cards.is_empty()
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
        }));
    }
    fn read_profile(&mut self, window: EntityId, keep_shown: bool, cx: &mut Context<Self>) {
        let Some(card) = self.profiles.cards.get_mut(&window) else {
            return;
        };
        let owner = card.view.owner;
        let account = card.view.account;
        let Some(server) = self
            .client
            .server(owner.entry.server_id)
            .filter(|server| server.owner() == Some(owner))
        else {
            return;
        };
        self.profiles.sequence += 1;
        let number = self.profiles.sequence;
        card.number = number;
        if !keep_shown {
            card.view.read = ProfileRead::Reading;
        }
        let api = server.api_for(owner.account_id);
        card.task = Some(cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    match api.profile_card(account).await {
                        Ok(mut card)
                            if card.profile.id == account && card.profile.departure.is_none() =>
                        {
                            card.profile.name = text::label(&card.profile.name);
                            for role in &mut card.server_roles {
                                role.name = text::label(&role.name);
                            }
                            for workspace in &mut card.workspaces {
                                workspace.name = text::label(&workspace.name);
                                for role in &mut workspace.roles {
                                    role.name = text::label(&role.name);
                                }
                            }
                            ProfileRead::Shown(Arc::new(card))
                        }
                        Ok(_) => ProfileRead::Unavailable,
                        Err(error)
                            if matches!(
                                error.category(),
                                Some(
                                    ErrorCategory::NotFound
                                        | ErrorCategory::NotAllowed
                                        | ErrorCategory::SessionEnded
                                        | ErrorCategory::Inactive
                                        | ErrorCategory::Banned
                                )
                            ) =>
                        {
                            ProfileRead::Unavailable
                        }
                        Err(error) => {
                            ProfileRead::Failed(text::flow_error(&FlowError::from(&error)))
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.owner(owner.entry.server_id) != Some(owner) {
                    this.keep_profiles();
                    cx.notify();
                    return;
                }
                if let Some(card) = this.profiles.cards.get_mut(&window).filter(|card| {
                    card.number == number
                        && card.view.owner == owner
                        && card.view.account == account
                }) {
                    card.view.read = read;
                    card.task = None;
                    this.want_people(cx);
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn keep_profiles(&mut self) {
        let owners: Vec<_> = self
            .client
            .servers()
            .iter()
            .filter_map(junto_client_core::ServerHandle::owner)
            .collect();
        self.profiles
            .cards
            .retain(|_, card| owners.contains(&card.view.owner));
        if self.profiles.cards.is_empty() {
            self.profiles.refresh = None;
        }
    }
    pub(super) fn invalidate_profiles(&mut self, server: ServerId, cx: &mut Context<Self>) {
        let windows: Vec<_> = self
            .profiles
            .cards
            .iter()
            .filter(|(_, card)| card.view.owner.entry.server_id == server)
            .map(|(window, _)| *window)
            .collect();
        for window in windows {
            self.read_profile(window, false, cx);
        }
    }
    pub(super) fn profile_people(&self) -> Vec<(ServerId, AccountId)> {
        self.profiles
            .cards
            .values()
            .filter(|card| matches!(card.view.read, ProfileRead::Reading | ProfileRead::Shown(_)))
            .map(|card| (card.view.owner.entry.server_id, card.view.account))
            .collect()
    }
    pub fn profile_message(
        &mut self,
        window: EntityId,
        call: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(view) = self.profile_view(window).filter(ProfileView::can_contact) else {
            return false;
        };
        self.open_conversation(view.owner.entry.server_id, view.account, cx);
        if call {
            self.start_direct_call(view.owner.entry.server_id, view.account, cx);
        }
        true
    }
}
