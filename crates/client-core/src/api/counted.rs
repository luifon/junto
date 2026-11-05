//! The requests of one client that are on their way, for the window tests: a test that
//! presses a control waits until the requests that the press started have an answer
//! (ADR 0902). Built only with the feature `test-support`.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::transport::{HttpRequest, OpenFuture, Transport, TransportFuture};

/// The numbers of the requests of a client that are on their way.
#[derive(Debug, Clone, Default)]
pub struct Requests(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    next: AtomicU64,
    open: Mutex<BTreeSet<u64>>,
}

impl Requests {
    /// The number that the next request gets.
    pub fn next(&self) -> u64 {
        self.0.next.load(Ordering::SeqCst)
    }

    /// How many requests with the number `from` or a later one are on their way.
    pub fn open_since(&self, from: u64) -> usize {
        self.0
            .open
            .lock()
            .map_or(0, |open| open.range(from..).count())
    }

    fn start(&self) -> Open {
        let number = self.0.next.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut open) = self.0.open.lock() {
            open.insert(number);
        }
        Open(self.clone(), number)
    }
}

/// A request on its way; it is answered when this is dropped.
struct Open(Requests, u64);

impl Drop for Open {
    fn drop(&mut self) {
        if let Ok(mut open) = (self.0).0.open.lock() {
            open.remove(&self.1);
        }
    }
}

/// A transport that counts the requests that it carries.
pub(crate) struct Counted {
    inner: Arc<dyn Transport>,
    requests: Requests,
}

impl Counted {
    pub(crate) fn wrap(inner: Arc<dyn Transport>) -> (Arc<dyn Transport>, Requests) {
        let requests = Requests::default();
        let counted = Arc::new(Self {
            inner,
            requests: requests.clone(),
        });
        (counted, requests)
    }
}

impl Transport for Counted {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let open = self.requests.start();
        let sent = self.inner.send(request);
        Box::pin(async move {
            let answer = sent.await;
            drop(open);
            answer
        })
    }

    fn send_part(&self, request: HttpRequest) -> TransportFuture {
        let open = self.requests.start();
        let sent = self.inner.send_part(request);
        Box::pin(async move {
            let answer = sent.await;
            drop(open);
            answer
        })
    }

    fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
        let open = self.requests.start();
        let opened = self.inner.open(request, from);
        Box::pin(async move {
            let answer = opened.await;
            drop(open);
            answer
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_open_until_its_answer() {
        let requests = Requests::default();
        let first = requests.start();
        let mark = requests.next();
        assert_eq!(requests.open_since(0), 1);
        assert_eq!(requests.open_since(mark), 0);
        let second = requests.start();
        assert_eq!(requests.open_since(mark), 1);
        drop(first);
        assert_eq!(requests.open_since(0), 1);
        drop(second);
        assert_eq!(requests.open_since(0), 0);
    }
}
