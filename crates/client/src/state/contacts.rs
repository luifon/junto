//! The chooser of a new direct conversation (ADR 0307): the accounts that the person may
//! write to, found on the server by a part of their name, also those that are in no member
//! list that the person can open.

use gpui::Context;
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::ids::{AccountId, ServerId};
use junto_client_core::protocol::message::{ContactAccount, ContactPage};

use super::AppState;
use super::account::shown_name;
use super::view::shown_model;
use crate::text;

/// The accounts of one read.
pub const PAGE: u32 = 50;
/// The longest part of a name that is sent: the length of a name (ADR 0119).
pub const LONGEST_PART: usize = 64;
/// The most accounts that the chooser holds. A person who has not found an account by
/// then types more of its name.
pub const MOST_ROWS: usize = 500;

/// The accounts that the server listed for one part of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    part: String,
    rows: Vec<ContactAccount>,
    next_after: Option<String>,
}

/// A read on its way: the part of the name, and the place after which it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Read {
    part: String,
    after: Option<String>,
}

/// What the application holds for the chooser.
#[derive(Debug, Default)]
pub struct ContactSearch {
    /// The server and the account that the chooser was opened for.
    open: Option<(ServerId, AccountId)>,
    /// The part of the name that the person typed last.
    wanted: String,
    listed: Option<Listed>,
    /// One read runs at a time. A part that the person types during a read is read after
    /// it.
    reading: Option<Read>,
    /// The person asked for the accounts after the listed ones.
    more_wanted: bool,
    /// Why the last read failed. No read starts by itself after a failure.
    failed: Option<String>,
    /// The number of times the chooser was opened or closed. An answer to a read of an
    /// earlier opening is dropped.
    session: u64,
}

impl ContactSearch {
    fn open(&mut self, server_id: ServerId, account_id: AccountId) {
        *self = Self {
            open: Some((server_id, account_id)),
            session: self.session + 1,
            ..Self::default()
        };
    }

    fn close(&mut self) {
        *self = Self {
            session: self.session + 1,
            ..Self::default()
        };
    }

    fn want(&mut self, part: &str) {
        let part: String = part.trim().chars().take(LONGEST_PART).collect();
        if part != self.wanted {
            self.wanted = part;
            self.more_wanted = false;
            self.failed = None;
        }
    }

    /// The read that is to start now, which counts as on its way from then on.
    fn next_read(&mut self) -> Option<Read> {
        if self.open.is_none() || self.reading.is_some() || self.failed.is_some() {
            return None;
        }
        let read = match &self.listed {
            Some(listed) if listed.part == self.wanted => {
                let after = listed
                    .next_after
                    .clone()
                    .filter(|_| self.more_wanted && listed.rows.len() < MOST_ROWS)?;
                Read {
                    part: listed.part.clone(),
                    after: Some(after),
                }
            }
            _ => Read {
                part: self.wanted.clone(),
                after: None,
            },
        };
        self.reading = Some(read.clone());
        Some(read)
    }

    fn answered(&mut self, session: u64, read: Read, answer: Result<ContactPage, String>) {
        if session != self.session {
            return;
        }
        self.reading = None;
        let page = match answer {
            Ok(page) => page,
            Err(reason) => {
                // A failure of a part that the person no longer wants is not told.
                if read.part == self.wanted {
                    self.failed = Some(reason);
                }
                return;
            }
        };
        match (&mut self.listed, read.after) {
            (Some(listed), Some(_)) if listed.part == read.part => {
                listed.rows.extend(page.accounts);
                listed.rows.truncate(MOST_ROWS);
                listed.next_after = page.next_after;
                self.more_wanted = false;
            }
            (_, Some(_)) => {}
            (_, None) => {
                self.listed = Some(Listed {
                    part: read.part,
                    rows: page.accounts,
                    next_after: page.next_after,
                });
            }
        }
    }
}

/// One account that the chooser offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactLine {
    pub account_id: AccountId,
    /// The name without control characters, cut at the length of a name.
    pub name: String,
}

/// The chooser as the window draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactsView {
    pub server_id: ServerId,
    /// The accounts for the part of the name that the rows belong to. Absent before the
    /// first answer.
    pub rows: Option<Vec<ContactLine>>,
    /// The listed rows belong to another part than the typed one, or a read is on its way.
    pub reading: bool,
    /// The server lists further accounts after the rows.
    pub more: bool,
    pub failed: Option<String>,
}

impl AppState {
    /// Opens the chooser for the account that the server shows now, and reads the first
    /// accounts.
    pub fn open_contacts(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let account = self
            .snapshots()
            .iter()
            .find(|server| server.id == server_id)
            .and_then(shown_model)
            .map(|model| model.account.id);
        let Some(account_id) = account else {
            return;
        };
        self.contacts.open(server_id, account_id);
        self.read_contacts(cx);
        cx.notify();
    }

    pub fn close_contacts(&mut self, cx: &mut Context<Self>) {
        self.contacts.close();
        cx.notify();
    }

    /// The person typed a part of a name: the accounts of that part are read, after a read
    /// that is on its way.
    pub fn type_contacts(&mut self, part: &str, cx: &mut Context<Self>) {
        self.contacts.want(part);
        self.read_contacts(cx);
        cx.notify();
    }

    /// Reads the accounts after the listed ones.
    pub fn more_contacts(&mut self, cx: &mut Context<Self>) {
        self.contacts.more_wanted = true;
        self.read_contacts(cx);
        cx.notify();
    }

    /// Reads again after a read that failed.
    pub fn retry_contacts(&mut self, cx: &mut Context<Self>) {
        self.contacts.failed = None;
        self.read_contacts(cx);
        cx.notify();
    }

    /// Opens the conversation with a chosen account, and closes the chooser.
    pub fn choose_contact(&mut self, account_id: AccountId, cx: &mut Context<Self>) {
        let Some((server_id, _)) = self.contacts.open else {
            return;
        };
        self.contacts.close();
        self.open_conversation(server_id, account_id, cx);
    }

    fn read_contacts(&mut self, cx: &mut Context<Self>) {
        let Some((server_id, account_id)) = self.contacts.open else {
            return;
        };
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(read) = self.contacts.next_read() else {
            return;
        };
        let session = self.contacts.session;
        let api = server.api_for(account_id);
        cx.spawn(async move |this, cx| {
            let answer = api
                .contacts(&read.part, read.after.as_deref(), Some(PAGE))
                .await
                .map_err(|error| text::flow_error(&FlowError::from(&error)));
            let _ = this.update(cx, |this, cx| {
                this.contacts.answered(session, read, answer);
                // A part that was typed during the read is read now.
                this.read_contacts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The chooser as the window shows it. Absent while it is closed, and when its server
    /// is gone or shows another account than the one that it was opened for.
    pub fn contacts_view(&self) -> Option<ContactsView> {
        let (server_id, account_id) = self.contacts.open?;
        let shows = self
            .snapshots()
            .iter()
            .find(|server| server.id == server_id)
            .and_then(shown_model)
            .is_some_and(|model| model.account.id == account_id);
        if !shows {
            return None;
        }
        let search = &self.contacts;
        let listed = search.listed.as_ref();
        Some(ContactsView {
            server_id,
            rows: listed.map(|listed| {
                listed
                    .rows
                    .iter()
                    .map(|row| ContactLine {
                        account_id: row.id,
                        name: shown_name(&row.name),
                    })
                    .collect()
            }),
            reading: search.reading.is_some()
                || listed.is_none_or(|listed| listed.part != search.wanted),
            more: listed.is_some_and(|listed| {
                listed.part == search.wanted
                    && listed.next_after.is_some()
                    && listed.rows.len() < MOST_ROWS
            }),
            failed: search.failed.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(names: &[&str], next: Option<&str>) -> ContactPage {
        ContactPage {
            accounts: names
                .iter()
                .map(|name| ContactAccount {
                    id: AccountId::generate(),
                    name: (*name).to_owned(),
                })
                .collect(),
            next_after: next.map(str::to_owned),
        }
    }

    fn names(search: &ContactSearch) -> Vec<String> {
        search
            .listed
            .as_ref()
            .map(|listed| listed.rows.iter().map(|row| row.name.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn one_read_runs_at_a_time_and_the_newest_part_is_read_after_it() {
        let mut search = ContactSearch::default();
        search.open(ServerId::generate(), AccountId::generate());
        let session = search.session;
        let first = search.next_read().unwrap();
        assert_eq!(first.part, "");
        search.want("  Ad ");
        search.want("Ada");
        assert_eq!(search.next_read(), None);
        search.answered(session, first, Ok(page(&["Ada", "Bruno"], None)));
        assert_eq!(names(&search), ["Ada", "Bruno"]);
        let second = search.next_read().unwrap();
        assert_eq!(
            (second.part.as_str(), second.after.as_deref()),
            ("Ada", None)
        );
        search.answered(session, second, Ok(page(&["Ada"], Some("next"))));
        assert_eq!(names(&search), ["Ada"]);
        // Nothing more is read until the person asks.
        assert_eq!(search.next_read(), None);
        search.more_wanted = true;
        let third = search.next_read().unwrap();
        assert_eq!(third.after.as_deref(), Some("next"));
        search.answered(session, third, Ok(page(&["Adam"], None)));
        assert_eq!(names(&search), ["Ada", "Adam"]);
        assert_eq!(search.next_read(), None);
    }

    #[test]
    fn an_answer_of_an_earlier_opening_or_part_changes_nothing() {
        let mut search = ContactSearch::default();
        search.open(ServerId::generate(), AccountId::generate());
        let earlier = search.session;
        let read = search.next_read().unwrap();
        search.open(ServerId::generate(), AccountId::generate());
        search.answered(earlier, read, Ok(page(&["Ada"], None)));
        assert!(search.listed.is_none());

        // A failure of a part that the person no longer wants is not told, and a failure
        // of the wanted part stops the reads until the person tries again.
        let session = search.session;
        let read = search.next_read().unwrap();
        search.want("Bru");
        search.answered(session, read, Err("refused".to_owned()));
        assert_eq!(search.failed, None);
        let read = search.next_read().unwrap();
        search.answered(session, read, Err("refused".to_owned()));
        assert_eq!(search.failed.as_deref(), Some("refused"));
        assert_eq!(search.next_read(), None);
        // A part is cut at the length of a name.
        search.want(&"a".repeat(80));
        assert_eq!(search.wanted.chars().count(), LONGEST_PART);
        assert!(search.next_read().is_some());
    }
}
