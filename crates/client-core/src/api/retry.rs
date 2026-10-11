//! The one place where a request is sent again (ADR 0009, ADR 0108, ADR 0205).
//!
//! Two rules:
//!
//! - After `busy` and `timeout` the server changed nothing. The client waits for the time
//!   of the `Retry-After` header and sends the same request again, a bounded number of
//!   times.
//! - A signed request is sent again unchanged: the same bytes, so the same challenge and
//!   the same signature. The challenge is unused until its operation commits, so the
//!   server accepts the repetition, and it is used once at most, so a repetition cannot
//!   run the operation twice. The client therefore also repeats a signed request whose
//!   answer was lost. When the repetition then gets `challenge_invalid`, the first copy
//!   may have committed, and the caller gets [`ApiError::OutcomeUnknown`].
//!
//! A request without a signature is never repeated after a lost answer.

use std::time::Duration;

use junto_protocol::error::ErrorCode;

use super::error::{ApiError, ErrorCategory, read_rejection};
use super::transport::{HttpRequest, HttpResponse, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How often one request is sent while the server answers `busy` or `timeout`.
    pub attempts: u32,
    /// The wait when the server names none.
    pub default_wait: Duration,
    /// The longest wait that the client accepts from a `Retry-After` header here.
    pub longest_wait: Duration,
    /// How often a signed request is sent again after a lost answer.
    pub lost_answer_repeats: u32,
    pub lost_answer_wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 5,
            default_wait: Duration::from_secs(1),
            longest_wait: Duration::from_secs(5),
            lost_answer_repeats: 2,
            lost_answer_wait: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repeat {
    /// Sent again after `busy` and `timeout` only.
    WhenUnchanged,
    /// A request that carries a signed challenge: also sent again after a lost answer.
    Signed,
    /// One part of a file. It is sent again after `busy` and `timeout` only, and it has
    /// the time limit of a slow connection.
    Part,
}

/// Sends `request` and returns the first response that is not an error. The request is
/// never changed between two attempts.
pub(crate) async fn send(
    transport: &dyn Transport,
    request: &HttpRequest,
    repeat: Repeat,
    policy: &RetryPolicy,
) -> Result<HttpResponse, ApiError> {
    let mut sent = 0u32;
    let mut lost_answers = 0u32;
    loop {
        sent += 1;
        let exchange = if repeat == Repeat::Part {
            transport.send_part(request.clone())
        } else {
            transport.send(request.clone())
        };
        let response = match exchange.await {
            Ok(response) => response,
            Err(error) => {
                let may_have_arrived = error.request_may_have_arrived();
                if repeat == Repeat::Signed && may_have_arrived {
                    if lost_answers < policy.lost_answer_repeats {
                        lost_answers += 1;
                        tokio::time::sleep(policy.lost_answer_wait).await;
                        continue;
                    }
                    return Err(ApiError::OutcomeUnknown);
                }
                return Err(if lost_answers > 0 {
                    ApiError::OutcomeUnknown
                } else {
                    ApiError::Unreachable(error)
                });
            }
        };
        if response.is_success() {
            return Ok(response);
        }
        if response.status < 400 {
            return Err(ApiError::InvalidResponse {
                status: response.status,
                reason: "unexpected status",
            });
        }
        let error = read_rejection(&response);
        if error.category() == Some(ErrorCategory::Unchanged) && sent < policy.attempts {
            let wait = error
                .retry_after()
                .unwrap_or(policy.default_wait)
                .min(policy.longest_wait);
            tokio::time::sleep(wait).await;
            continue;
        }
        if lost_answers > 0 && error.code() == Some(ErrorCode::ChallengeInvalid) {
            return Err(ApiError::OutcomeUnknown);
        }
        return Err(error);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::api::transport::{
        Method, RequestBody, TransportError, TransportErrorKind, TransportFuture,
    };

    type Step = Result<HttpResponse, TransportError>;

    #[derive(Default)]
    struct Script {
        steps: Mutex<VecDeque<Step>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Script {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                steps: Mutex::new(steps.into_iter().collect()),
                seen: Mutex::default(),
            }
        }

        fn bodies(&self) -> Vec<Vec<u8>> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|request| {
                    request
                        .body
                        .as_ref()
                        .map(|body| body.as_slice().to_vec())
                        .unwrap_or_default()
                })
                .collect()
        }
    }

    impl Transport for Script {
        fn send(&self, request: HttpRequest) -> TransportFuture {
            self.seen.lock().unwrap().push(request);
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("an unexpected request");
            Box::pin(async move { step })
        }
    }

    fn request() -> HttpRequest {
        HttpRequest {
            method: Method::Post,
            url: "https://chat.example.test/api/v1/join".parse().unwrap(),
            bearer: None,
            body: Some(RequestBody::json(
                br#"{"proof":{"challenge_id":"c","signature":"s"}}"#.to_vec().into(),
            )),
        }
    }

    fn ok() -> Step {
        Ok(HttpResponse {
            status: 201,
            retry_after: None,
            body: b"{}".to_vec().into(),
        })
    }

    fn refusal(code: &str, status: u16, retry_after: Option<u64>) -> Step {
        Ok(HttpResponse {
            status,
            retry_after: retry_after.map(Duration::from_secs),
            body: format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#)
                .into_bytes()
                .into(),
        })
    }

    fn lost(kind: TransportErrorKind) -> Step {
        Err(TransportError::new(kind, ""))
    }

    #[tokio::test(start_paused = true)]
    async fn busy_and_timeout_are_waited_out_and_the_same_bytes_are_sent_again() {
        let script = Script::new([
            refusal("busy", 503, Some(1)),
            refusal("timeout", 503, Some(2)),
            ok(),
        ]);
        let started = tokio::time::Instant::now();
        let response = send(&script, &request(), Repeat::Signed, &RetryPolicy::default())
            .await
            .unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(started.elapsed(), Duration::from_secs(3));
        let bodies = script.bodies();
        assert_eq!(bodies.len(), 3);
        assert!(bodies.iter().all(|body| *body == bodies[0]));
    }

    #[tokio::test(start_paused = true)]
    async fn the_wait_has_a_default_and_an_upper_bound() {
        let script = Script::new([
            refusal("busy", 503, None),
            refusal("busy", 503, Some(3600)),
            ok(),
        ]);
        let started = tokio::time::Instant::now();
        send(
            &script,
            &request(),
            Repeat::WhenUnchanged,
            &RetryPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(1 + 5));
    }

    #[tokio::test(start_paused = true)]
    async fn a_server_that_stays_busy_ends_the_attempts() {
        let policy = RetryPolicy::default();
        let script = Script::new((0..policy.attempts).map(|_| refusal("busy", 503, Some(1))));
        let error = send(&script, &request(), Repeat::WhenUnchanged, &policy)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(ErrorCode::Busy));
        assert_eq!(script.bodies().len(), policy.attempts as usize);
    }

    #[tokio::test(start_paused = true)]
    async fn other_errors_are_not_sent_again() {
        for (code, status) in [
            ("rate_limited", 429),
            ("internal", 500),
            ("invalid_signature", 401),
            ("challenge_invalid", 401),
            ("invite_expired", 410),
            ("made_up_later", 503),
        ] {
            let script = Script::new([refusal(code, status, Some(1))]);
            let error = send(&script, &request(), Repeat::Signed, &RetryPolicy::default())
                .await
                .unwrap_err();
            assert_eq!(script.bodies().len(), 1, "{code}");
            assert_eq!(error.rejection().unwrap().status, status);
            assert_eq!(
                error.code() == Some(ErrorCode::Unknown),
                code == "made_up_later"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_signed_request_is_repeated_after_a_lost_answer() {
        let script = Script::new([lost(TransportErrorKind::Timeout), ok()]);
        send(&script, &request(), Repeat::Signed, &RetryPolicy::default())
            .await
            .unwrap();
        let bodies = script.bodies();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_used_challenge_after_a_lost_answer_means_an_unknown_outcome() {
        let script = Script::new([
            lost(TransportErrorKind::Interrupted),
            refusal("challenge_invalid", 401, None),
        ]);
        let error = send(&script, &request(), Repeat::Signed, &RetryPolicy::default())
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::OutcomeUnknown), "{error}");

        let policy = RetryPolicy::default();
        let script = Script::new(
            (0..=policy.lost_answer_repeats).map(|_| lost(TransportErrorKind::Timeout)),
        );
        let error = send(&script, &request(), Repeat::Signed, &policy)
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::OutcomeUnknown), "{error}");
        assert_eq!(
            script.bodies().len(),
            policy.lost_answer_repeats as usize + 1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_connection_is_reported_and_an_unsigned_request_is_never_repeated() {
        let script = Script::new([lost(TransportErrorKind::Connect)]);
        let error = send(&script, &request(), Repeat::Signed, &RetryPolicy::default())
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unreachable(_)), "{error}");

        let script = Script::new([lost(TransportErrorKind::Timeout)]);
        let error = send(
            &script,
            &request(),
            Repeat::WhenUnchanged,
            &RetryPolicy::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ApiError::Unreachable(_)), "{error}");
        assert_eq!(script.bodies().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_redirect_is_not_followed_and_not_accepted() {
        let script = Script::new([Ok(HttpResponse {
            status: 307,
            retry_after: None,
            body: Vec::new().into(),
        })]);
        let error = send(
            &script,
            &request(),
            Repeat::WhenUnchanged,
            &RetryPolicy::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ApiError::InvalidResponse { status: 307, .. }
        ));
    }
}
