//! The collections of the administration as a reader opens them (ADR 0760).

use std::future::Future;

use tokio::sync::oneshot;

use super::{Command, LiveServer, LoadError};
use crate::model::AdminPart;

impl LiveServer {
    /// Asks for a collection of the administration. It is read from its route, appears
    /// in [`crate::model::ServerModel::admin`], and follows the events from then on;
    /// after a reload of the state the client reads it again by itself (ADR 0207). The
    /// future ends when the collection is in the state or the read failed; a failed
    /// read is also in the state, with its reason. Each call needs one
    /// [`LiveServer::release_admin`], also after a failure.
    pub fn open_admin(
        &self,
        part: AdminPart,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::OpenAdmin { part, reply });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// States that a reader needs a collection no longer. When no reader is left, the
    /// state drops it.
    pub fn release_admin(&self, part: AdminPart) {
        self.shared.send(Command::ReleaseAdmin(part));
    }

    /// Reads again a collection that a reader has open and whose last read failed.
    pub fn read_admin_again(&self, part: AdminPart) {
        self.shared.send(Command::ReadAdminAgain(part));
    }

    /// Reads again a collection that a reader has open, also when it is loaded: the reader
    /// knows of a change that can be newer than every read that started before this call.
    /// A loaded collection keeps its values until the read answers, and
    /// [`crate::model::ReadTimes`] states when the read that the state shows started.
    pub fn read_admin_now(&self, part: AdminPart) {
        self.shared.send(Command::ReadAdminNow(part));
    }
}
