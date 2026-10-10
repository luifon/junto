//! The bytes of request bodies that the server holds in memory (ADR 0013).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Why the server does not hold more bytes of a request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The address of the client holds as many bytes as one address may.
    Address,
    /// The server holds as many bytes as it may for all requests together.
    Server,
}

#[derive(Default)]
struct Held {
    total: usize,
    by_address: HashMap<IpAddr, usize>,
}

/// The bound on the bytes of request bodies that are held at one time: for the whole server
/// and for one client address. A body is counted while its request is read and while its
/// route runs.
pub struct BodyBudget {
    total: usize,
    per_address: usize,
    held: Mutex<Held>,
}

impl BodyBudget {
    pub fn new(total: usize, per_address: usize) -> Self {
        Self {
            total,
            per_address,
            held: Mutex::new(Held::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The bytes that are held now.
    pub fn held(&self) -> usize {
        self.state().total
    }

    /// The addresses that hold bytes now.
    pub fn addresses(&self) -> usize {
        self.state().by_address.len()
    }
}

/// The bytes of one request body. Dropping it gives them back.
pub struct Reservation {
    budget: Arc<BodyBudget>,
    address: IpAddr,
    bytes: usize,
}

impl Reservation {
    pub fn new(budget: Arc<BodyBudget>, address: IpAddr) -> Self {
        Self {
            budget,
            address,
            bytes: 0,
        }
    }

    /// Counts `bytes` more for this request, or states which bound they would pass.
    pub fn grow(&mut self, bytes: usize) -> Result<(), Refused> {
        let budget = self.budget.clone();
        let mut held = budget.state();
        let of_address = held.by_address.get(&self.address).copied().unwrap_or(0);
        if of_address.saturating_add(bytes) > budget.per_address {
            return Err(Refused::Address);
        }
        if held.total.saturating_add(bytes) > budget.total {
            return Err(Refused::Server);
        }
        held.total += bytes;
        held.by_address.insert(self.address, of_address + bytes);
        self.bytes += bytes;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut held = self.budget.state();
        held.total = held.total.saturating_sub(self.bytes);
        let left = held
            .by_address
            .get(&self.address)
            .copied()
            .unwrap_or(0)
            .saturating_sub(self.bytes);
        if left == 0 {
            held.by_address.remove(&self.address);
        } else {
            held.by_address.insert(self.address, left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(last: u8) -> IpAddr {
        IpAddr::from([192, 0, 2, last])
    }

    #[test]
    fn the_bytes_of_one_address_and_of_the_server_are_bounded_and_given_back() {
        let budget = Arc::new(BodyBudget::new(100, 40));
        let mut first = Reservation::new(budget.clone(), address(1));
        assert_eq!(first.grow(30), Ok(()));
        assert_eq!(first.grow(11), Err(Refused::Address));
        assert_eq!(first.grow(10), Ok(()));
        let mut same_address = Reservation::new(budget.clone(), address(1));
        assert_eq!(same_address.grow(1), Err(Refused::Address));

        let mut second = Reservation::new(budget.clone(), address(2));
        let mut third = Reservation::new(budget.clone(), address(3));
        assert_eq!(second.grow(40), Ok(()));
        assert_eq!(third.grow(21), Err(Refused::Server));
        assert_eq!(third.grow(20), Ok(()));
        assert_eq!((budget.held(), budget.addresses()), (100, 3));

        drop(first);
        assert_eq!((budget.held(), budget.addresses()), (60, 2));
        assert_eq!(same_address.grow(40), Ok(()));
        drop((second, third, same_address));
        assert_eq!((budget.held(), budget.addresses()), (0, 0));
    }
}
