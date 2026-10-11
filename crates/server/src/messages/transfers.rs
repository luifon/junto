//! Independent upload and download capacity, with a bounded share per account.
use junto_protocol::ids::AccountId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Default)]
pub struct Transfers<const LIMIT: usize> {
    held: Mutex<HashMap<AccountId, usize>>,
    changed: tokio::sync::Notify,
}

pub struct Permit<const LIMIT: usize> {
    transfers: Arc<Transfers<LIMIT>>,
    account: AccountId,
}
pub type Downloads = Transfers<8>;
pub type Uploads = Transfers<4>;

impl<const LIMIT: usize> Transfers<LIMIT> {
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
    pub fn acquire(self: &Arc<Self>, account: AccountId) -> Option<Permit<LIMIT>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.values().sum::<usize>() >= LIMIT || held.get(&account).copied().unwrap_or(0) >= 2 {
            return None;
        }
        *held.entry(account).or_default() += 1;
        Some(Permit {
            transfers: self.clone(),
            account,
        })
    }
}
impl<const LIMIT: usize> Drop for Permit<LIMIT> {
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
        let downloads = Arc::new(Downloads::default());
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
}
