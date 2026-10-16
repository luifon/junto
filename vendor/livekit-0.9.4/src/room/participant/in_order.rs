// Added to the vendored copy of livekit 0.9.4; this file is not in the published crate and
// contains no code of it.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Work that runs on its own task, one piece after the other, in the order in which it was
//! given.
//!
//! A local track tells the media server each mute and each unmute with a request. The
//! request waits while the engine restores its connection, so a task of its own sends it.
//! Separate tasks that wait together start in any order when the connection is back: an
//! unmute could reach the server before the mute that was made before it, and the server
//! would hold the track as muted while the device sends. Each piece of work given here
//! starts only when the piece given before it has ended.
//!
//! The file uses the standard library and Tokio only, so that a test outside the crate can
//! compile it.

use std::future::Future;
use std::sync::{Mutex, PoisonError};

use tokio::sync::oneshot;

/// Runs work on tasks of the current Tokio runtime, in the order of [`InOrder::run`].
#[derive(Default)]
pub(crate) struct InOrder {
    /// Ends when the piece of work that was given last has ended.
    last: Mutex<Option<oneshot::Receiver<()>>>,
}

impl InOrder {
    /// Starts `work` on a new task after every piece of work that was given before it. A
    /// piece whose task ended without finishing, by a panic or the end of the runtime,
    /// does not hold back the pieces after it.
    pub(crate) fn run(&self, work: impl Future<Output = ()> + Send + 'static) {
        let (done, ended) = oneshot::channel();
        let before = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(ended);
        tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            work.await;
            let _ = done.send(());
        });
    }
}
