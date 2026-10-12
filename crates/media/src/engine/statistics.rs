//! Statistics queried after a live test fails, without sampling its measured interval.

use std::time::Duration;

use livekit::rtc_engine::SessionStats;
use tokio::sync::oneshot;

use super::{Actor, Command, Engine};

pub(crate) type Reply = oneshot::Sender<Result<SessionStats, String>>;

impl Engine {
    pub(crate) fn statistics(&self, reply: Reply) {
        let _ = self.commands.send(Command::Statistics(reply));
    }
}

impl Actor {
    pub(super) async fn statistics(&mut self, reply: Reply) {
        let Some(room) = self.room.as_ref() else {
            let _ = reply.send(Err("no media connection".to_owned()));
            return;
        };
        let stats = match tokio::time::timeout(Duration::from_secs(3), room.get_stats()).await {
            Ok(stats) => stats.map_err(|error| error.to_string()),
            Err(_) => Err("statistics request timed out".to_owned()),
        };
        let _ = reply.send(stats);
    }
}
