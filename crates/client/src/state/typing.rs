use super::AppState;
use gpui::{Context, EntityId, Task};
use junto_client_core::{
    Owner,
    model::PlaceId,
    protocol::{
        ids::{AccountId, ServerId},
        typing::{TYPING_LIFETIME_MS, Typing},
    },
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

type Source = (EntityId, bool);
struct ComposerActivity {
    sources: HashMap<Source, Instant>,
    sent: Option<Instant>,
}
struct Person {
    owner: Owner,
    name: String,
    until: Instant,
}
#[derive(Default)]
pub(super) struct Activity {
    local: HashMap<(Owner, PlaceId), ComposerActivity>,
    people: HashMap<(ServerId, PlaceId, AccountId), Person>,
    timer: Option<Task<()>>,
}
type Owners<'a> = HashMap<ServerId, (Owner, Option<&'a junto_client_core::model::ServerModel>)>;

fn allowed(
    model: Option<&junto_client_core::model::ServerModel>,
    place: PlaceId,
    account: Option<AccountId>,
) -> bool {
    let Some(model) = model else {
        return false;
    };
    if model.place(place).is_none() {
        return false;
    }
    let PlaceId::Direct(id) = place else {
        return true;
    };
    if let Some(pair) = model.conversations.iter().find(|pair| pair.id == id) {
        let member = |account| account == pair.low_account_id || account == pair.high_account_id;
        return member(model.account.id) && account.is_none_or(member);
    }
    model
        .group_conversations
        .iter()
        .find(|group| group.id == id)
        .is_some_and(|group| {
            group.account_ids.contains(&model.account.id)
                && account.is_none_or(|account| group.account_ids.contains(&account))
        })
}

impl Activity {
    fn keep(&mut self, owners: &Owners<'_>, now: Instant) {
        self.local.retain(|(owner, place), local| {
            local.sources.retain(|_, until| *until > now);
            owners
                .get(&owner.entry.server_id)
                .is_some_and(|(held, model)| held == owner && allowed(*model, *place, None))
                && !local.sources.is_empty()
        });
        self.people.retain(|(server, place, account), person| {
            owners.get(server).is_some_and(|(owner, model)| {
                *owner == person.owner && allowed(*model, *place, Some(*account))
            }) && person.until > now
        });
    }
}

impl AppState {
    pub(super) fn keep_typing_owners(&mut self, snapshots: &[super::view::ServerSnapshot]) {
        let models: HashMap<_, _> = snapshots
            .iter()
            .filter_map(|snapshot| {
                super::view::shown_model(snapshot).map(|model| (snapshot.id, model))
            })
            .collect();
        let owners: Owners<'_> = self
            .client
            .servers()
            .iter()
            .filter_map(|server| {
                self.owner(server.id())
                    .map(|owner| (server.id(), (owner, models.get(&server.id()).copied())))
            })
            .collect();
        self.typing.keep(&owners, Instant::now());
    }

    fn typing_allowed(&self, server: ServerId, place: PlaceId, account: Option<AccountId>) -> bool {
        #[cfg(feature = "test-support")]
        if let Some(entry) = self
            .fixture
            .as_ref()
            .and_then(|entries| entries.iter().find(|entry| entry.id == server))
        {
            return allowed(super::view::shown_model(entry), place, account);
        }
        self.client
            .server(server)
            .is_some_and(|server| allowed(server.live().view().model.as_deref(), place, account))
    }

    fn typing_owner(&self, server: ServerId) -> Option<Owner> {
        #[cfg(feature = "test-support")]
        if let Some(entry) = self
            .fixture
            .as_ref()
            .and_then(|entries| entries.iter().find(|entry| entry.id == server))
        {
            return Some(Owner {
                entry: junto_client_core::book::EntryKey {
                    server_id: server,
                    generation: 0,
                },
                account_id: super::view::shown_model(entry)?.account.id,
            });
        }
        self.owner(server)
    }
    pub(crate) fn typing_activity(
        &mut self,
        source: Source,
        owner: Owner,
        place: PlaceId,
        active: bool,
    ) {
        if self.owner(owner.entry.server_id) != Some(owner)
            || !self.typing_allowed(owner.entry.server_id, place, None)
        {
            return;
        }
        let now = Instant::now();
        let key = (owner, place);
        self.typing
            .local
            .retain(|(held, _), _| *held == owner || held.entry.server_id != owner.entry.server_id);
        let previous = self.typing.local.contains_key(&key);
        let local = self
            .typing
            .local
            .entry(key)
            .or_insert_with(|| ComposerActivity {
                sources: HashMap::new(),
                sent: None,
            });
        local.sources.retain(|_, until| *until > now);
        if active {
            local.sources.insert(
                source,
                now + Duration::from_millis(u64::from(TYPING_LIFETIME_MS)),
            );
        } else {
            local.sources.remove(&source);
        }
        let sends = !active && previous
            || active
                && local
                    .sent
                    .is_none_or(|sent| now.duration_since(sent) >= Duration::from_secs(2));
        let lifetime = local.sources.values().max().map_or(0, |until| {
            until
                .saturating_duration_since(now)
                .as_millis()
                .min(u128::from(TYPING_LIFETIME_MS)) as u32
        });
        if sends {
            local.sent = Some(now);
            if let Some(server) = self.client.server(owner.entry.server_id) {
                server.live().typing(owner.account_id, place, lifetime);
            }
        }
        if local.sources.is_empty() {
            self.typing.local.remove(&key);
        }
    }
    pub(crate) fn typing_people(
        &self,
        server: ServerId,
        place: PlaceId,
    ) -> Vec<(AccountId, String)> {
        let Some(owner) = self.typing_owner(server) else {
            return Vec::new();
        };
        let now = Instant::now();
        let mut names: Vec<_> = self
            .typing
            .people
            .iter()
            .filter(|((saved, target, account), person)| {
                *saved == server
                    && *target == place
                    && person.owner == owner
                    && person.until > now
                    && self.typing_allowed(server, place, Some(*account))
            })
            .map(|((_, _, account), person)| (*account, person.name.clone()))
            .collect();
        names.sort_by(|a, b| {
            a.1.to_lowercase()
                .cmp(&b.1.to_lowercase())
                .then(a.0.as_uuid().cmp(&b.0.as_uuid()))
        });
        names
    }
    pub(crate) fn typing_profile_allowed(
        &self,
        owner: Owner,
        place: PlaceId,
        account: AccountId,
    ) -> bool {
        self.typing_owner(owner.entry.server_id) == Some(owner)
            && self.typing_allowed(owner.entry.server_id, place, Some(account))
            && self.typing_people(owner.entry.server_id, place).len() <= 3
            && self
                .typing
                .people
                .get(&(owner.entry.server_id, place, account))
                .is_some_and(|person| person.owner == owner && person.until > Instant::now())
    }

    pub(super) fn typing_profile_people(&self) -> Vec<(ServerId, AccountId)> {
        let places = self
            .shown
            .server
            .into_iter()
            .flat_map(|server| {
                self.shown
                    .place()
                    .into_iter()
                    .chain(self.shown.thread_place())
                    .map(move |place| (server, place))
            })
            .chain(self.call_window_place(&self.snapshots()));
        places
            .flat_map(|(server, place)| {
                let people = self.typing_people(server, place);
                if people.len() > 3 {
                    Vec::new()
                } else {
                    people
                        .into_iter()
                        .map(|(account, _)| (server, account))
                        .collect()
                }
            })
            .collect()
    }

    pub(super) fn heard_typing(
        &mut self,
        server: ServerId,
        activity: Option<&Typing>,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.typing_owner(server) else {
            self.typing
                .people
                .retain(|(saved, _, _), _| *saved != server);
            return;
        };
        let now = Instant::now();
        self.typing.people.retain(|(saved, _, _), person| {
            person.until > now && (*saved != server || person.owner == owner)
        });
        if let Some(activity) = activity {
            let place = PlaceId::from_typing(activity.place);
            let key = (server, place, activity.account_id);
            if !activity.active {
                self.typing.people.remove(&key);
            } else if activity.account_id != owner.account_id
                && self.typing_allowed(server, place, Some(activity.account_id))
                && (self.typing.people.len() < 256 || self.typing.people.contains_key(&key))
            {
                self.typing.people.insert(
                    key,
                    Person {
                        owner,
                        name: activity.name.clone(),
                        until: now
                            + Duration::from_millis(u64::from(
                                activity.expires_in_ms.min(TYPING_LIFETIME_MS),
                            )),
                    },
                );
            }
        } else {
            self.typing
                .people
                .retain(|(saved, _, _), _| *saved != server);
        }
        if self.typing.timer.is_none() && !self.typing.people.is_empty() {
            self.typing.timer = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let keep = this
                        .update(cx, |this, cx| {
                            let before = this.typing.people.len();
                            this.typing
                                .people
                                .retain(|_, person| person.until > Instant::now());
                            if before != this.typing.people.len() {
                                this.want_people(cx);
                                cx.notify();
                            }
                            if this.typing.people.is_empty() {
                                this.typing.timer = None;
                                false
                            } else {
                                true
                            }
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
        self.want_people(cx);
        cx.notify();
    }
    #[cfg(feature = "test-support")]
    pub fn use_typing_fixture(
        &mut self,
        server: ServerId,
        place: PlaceId,
        people: Vec<(AccountId, String)>,
        cx: &mut Context<Self>,
    ) {
        for (account_id, name) in people {
            self.heard_typing(
                server,
                Some(&Typing {
                    place: place.typing_place(),
                    account_id,
                    name,
                    active: true,
                    expires_in_ms: TYPING_LIFETIME_MS,
                }),
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests;
