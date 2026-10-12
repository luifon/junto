//! Requests to the media server that must arrive in the order in which they were made
//! (ADR 0737).
//!
//! The call library sends some requests from a task that it starts for the one request,
//! and it gives no handle to wait for. On a runtime with several threads two such tasks
//! run in any order, so the media server can get an earlier request last and keep its
//! value. The requests are therefore made on one thread with a runtime that runs its
//! tasks in the order in which they were started.

use std::sync::OnceLock;

use tokio::sync::mpsc;

type Job = Box<dyn FnOnce() + Send>;

/// Starts the thread. Absent when the system gives no thread or no runtime.
fn start() -> Option<mpsc::UnboundedSender<Job>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .ok()?;
    let (jobs, mut made) = mpsc::unbounded_channel::<Job>();
    std::thread::Builder::new()
        .name("media-requests".to_owned())
        .spawn(move || {
            runtime.block_on(async move {
                while let Some(job) = made.recv().await {
                    // A job that panics must not end the thread: the jobs after it
                    // would run without their order.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                        tracing::warn!("a request to the media server panicked");
                    }
                }
            });
        })
        .ok()?;
    Some(jobs)
}

/// Runs a job after every job before it. A task that the job starts with `tokio::spawn`
/// runs after the tasks of the jobs before it, up to its first wait.
pub(super) fn in_order(job: impl FnOnce() + Send + 'static) {
    static JOBS: OnceLock<Option<mpsc::UnboundedSender<Job>>> = OnceLock::new();
    let jobs = JOBS.get_or_init(|| {
        let jobs = start();
        if jobs.is_none() {
            tracing::warn!("no thread for requests in order: their order is not kept");
        }
        jobs
    });
    let job: Job = Box::new(job);
    let unsent = match jobs {
        Some(jobs) => jobs.send(job).err().map(|unsent| unsent.0),
        None => Some(job),
    };
    // Without the thread the request is still made, from the runtime of the caller.
    if let Some(job) = unsent {
        job();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Mutex;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_tasks_that_jobs_start_run_in_the_order_of_the_jobs() {
        const JOBS: usize = 2_000;
        let ran: Arc<Mutex<Vec<usize>>> = Arc::default();
        let (done, mut finished) = mpsc::unbounded_channel();
        for number in 0..JOBS {
            let (ran, done) = (ran.clone(), done.clone());
            // Each job starts a task of its own, as the call library does for a request.
            in_order(move || {
                tokio::spawn(async move {
                    ran.lock().await.push(number);
                    let _ = done.send(());
                });
            });
        }
        for _ in 0..JOBS {
            tokio::time::timeout(Duration::from_secs(60), finished.recv())
                .await
                .unwrap();
        }
        assert_eq!(*ran.lock().await, (0..JOBS).collect::<Vec<_>>());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_job_that_panics_does_not_end_the_order_of_the_later_jobs() {
        in_order(|| panic!("a request that fails inside the call library"));
        let (done, mut finished) = mpsc::unbounded_channel();
        for _ in 0..3 {
            let done = done.clone();
            in_order(move || {
                let _ = done.send(std::thread::current().name().map(str::to_owned));
            });
        }
        for _ in 0..3 {
            let thread = tokio::time::timeout(Duration::from_secs(60), finished.recv())
                .await
                .unwrap()
                .unwrap();
            // The jobs still run on the one thread that keeps their order.
            assert_eq!(thread.as_deref(), Some("media-requests"));
        }
    }
}
