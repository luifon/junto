//! The requests of one client that are on their way, for the window tests: a test that
//! presses a control waits until the requests that the press started have an answer
//! (ADR 0902). Built only with the feature `test-support`.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::transport::{HttpRequest, OpenFuture, Transport, TransportFuture};

/// The numbers of the requests of every client of the process: two clients of one test
/// never give one number, so a proxy that holds a number names one request.
static NEXT: AtomicU64 = AtomicU64::new(0);

tokio::task_local! {
    /// The number of the request that the work of the runtime makes: the request was
    /// counted where it was asked for, on the thread of its caller.
    pub(crate) static NUMBER: u64;
}

/// The numbers of the requests of a client that are on their way.
#[derive(Debug, Clone, Default)]
pub struct Requests(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    open: Mutex<BTreeSet<u64>>,
}

impl Requests {
    /// The number that the next request of any client gets.
    pub fn next(&self) -> u64 {
        NEXT.load(Ordering::SeqCst)
    }

    /// The numbers of the requests of this client with the number `from` or a later one
    /// that are on their way.
    pub fn open_since(&self, from: u64) -> Vec<u64> {
        self.0
            .open
            .lock()
            .map(|open| open.range(from..).copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn start(&self) -> Open {
        let number = NEXT.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut open) = self.0.open.lock() {
            open.insert(number);
        }
        Open(self.clone(), number)
    }
}

impl Open {
    pub(crate) fn number(&self) -> u64 {
        self.1
    }
}

/// Runs `work` with the number of `ticket`: its requests carry it, and the request stays
/// on its way until `work` and every other holder of the ticket ended.
pub(crate) async fn within<F: std::future::Future>(
    ticket: Option<Arc<Open>>,
    work: F,
) -> F::Output {
    match ticket {
        Some(open) => {
            NUMBER
                .scope(open.number(), async move {
                    let _open = open;
                    work.await
                })
                .await
        }
        None => work.await,
    }
}

/// A request on its way; it is answered when this is dropped.
pub(crate) struct Open(Requests, u64);

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

impl Counted {
    /// A request that the work of a counted request makes carries its number; any other
    /// request, such as a read of the event stream, is counted here.
    fn counted(&self, request: &mut HttpRequest) -> Option<Open> {
        match NUMBER.try_with(|number| *number) {
            Ok(number) => {
                request.test_number = Some(number);
                None
            }
            Err(_) => {
                let open = self.requests.start();
                request.test_number = Some(open.number());
                Some(open)
            }
        }
    }
}

impl Transport for Counted {
    fn send(&self, mut request: HttpRequest) -> TransportFuture {
        let open = self.counted(&mut request);
        let sent = self.inner.send(request);
        Box::pin(async move {
            let answer = sent.await;
            drop(open);
            answer
        })
    }

    fn send_part(&self, mut request: HttpRequest) -> TransportFuture {
        let open = self.counted(&mut request);
        let sent = self.inner.send_part(request);
        Box::pin(async move {
            let answer = sent.await;
            drop(open);
            answer
        })
    }

    fn open(&self, mut request: HttpRequest, from: u64) -> OpenFuture {
        let open = self.counted(&mut request);
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
        assert_eq!(requests.open_since(0), [first.number()]);
        assert!(requests.open_since(mark).is_empty());
        let second = requests.start();
        assert_eq!(requests.open_since(mark), [second.number()]);
        let other = Requests::default().start();
        assert_ne!(
            other.number(),
            second.number(),
            "two clients share no number"
        );
        drop(first);
        assert_eq!(requests.open_since(0), [second.number()]);
        drop(second);
        assert!(requests.open_since(0).is_empty());
    }
}
