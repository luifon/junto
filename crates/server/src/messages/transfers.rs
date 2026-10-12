//! Independent upload and download capacity, with a bounded share per account.
use junto_protocol::ids::AccountId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// The transfers of one kind that the server serves at one time: `limit` together, and
/// `share` of them for one account.
pub struct Transfers {
    limit: usize,
    share: usize,
    held: Mutex<HashMap<AccountId, usize>>,
    changed: tokio::sync::Notify,
}

pub struct Permit {
    transfers: Arc<Transfers>,
    account: AccountId,
}

impl Transfers {
    pub fn new(limit: usize, share: usize) -> Self {
        Self {
            limit,
            share,
            held: Mutex::new(HashMap::new()),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// The downloads of attachments: 8 at one time, 2 of one account (ADR 0408).
    pub fn attachment_downloads() -> Self {
        Self::new(8, 2)
    }

    /// The uploads of attachments: 4 at one time, 2 of one account (ADR 0408).
    pub fn attachment_uploads() -> Self {
        Self::new(4, 2)
    }

    pub async fn wait_idle(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.held() == 0 {
                return;
            }
            changed.await;
        }
    }
    pub fn held(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .sum()
    }
    pub fn acquire(self: &Arc<Self>, account: AccountId) -> Option<Permit> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.values().sum::<usize>() >= self.limit
            || held.get(&account).copied().unwrap_or(0) >= self.share
        {
            return None;
        }
        *held.entry(account).or_default() += 1;
        Some(Permit {
            transfers: self.clone(),
            account,
        })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut held = self
            .transfers
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.account) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.account);
            }
        }
        drop(held);
        self.transfers.changed.notify_waiters();
    }
}

/// One verification job per upload; dropping an HTTP request does not cancel its job.
#[derive(Default)]
pub struct Checks(Mutex<std::collections::HashSet<junto_protocol::ids::AttachmentId>>);
pub struct Check {
    checks: Arc<Checks>,
    id: junto_protocol::ids::AttachmentId,
}
impl Checks {
    pub fn contains(&self, id: junto_protocol::ids::AttachmentId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
    }
    pub fn begin(self: &Arc<Self>, id: junto_protocol::ids::AttachmentId) -> Option<Check> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id)
            .then(|| Check {
                checks: self.clone(),
                id,
            })
    }
}
impl Drop for Check {
    fn drop(&mut self) {
        self.checks
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn download_places_are_bounded_for_the_server_and_each_account_and_return_on_drop() {
        let downloads = Arc::new(Transfers::attachment_downloads());
        let accounts = [
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        ];
        let mut permits = Vec::new();
        for account in accounts {
            permits.push(downloads.acquire(account).unwrap());
            permits.push(downloads.acquire(account).unwrap());
            assert!(downloads.acquire(account).is_none());
        }
        assert_eq!(downloads.held(), 8);
        assert!(downloads.acquire(AccountId::generate()).is_none());
        permits.pop();
        assert!(downloads.acquire(AccountId::generate()).is_some());
        drop(permits);
        downloads.wait_idle().await;
        assert_eq!(downloads.held(), 0);
    }

    #[test]
    fn the_share_of_an_account_and_the_places_of_the_server_are_the_numbers_of_the_kind() {
        let downloads = Arc::new(Transfers::new(5, 3));
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let of_first: Vec<_> = (0..3).map(|_| downloads.acquire(first).unwrap()).collect();
        assert!(downloads.acquire(first).is_none());
        let of_second: Vec<_> = (0..2).map(|_| downloads.acquire(second).unwrap()).collect();
        assert!(downloads.acquire(second).is_none());
        assert!(downloads.acquire(AccountId::generate()).is_none());
        drop(of_first);
        assert!(downloads.acquire(second).is_some());
        assert!(Arc::new(Transfers::new(0, 1)).acquire(first).is_none());
        drop(of_second);
    }
}
