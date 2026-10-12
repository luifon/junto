//! The storage guard (ADR 0006, ADR 0605): a job that writes large files reserves disk space
//! when it starts, and the server keeps a reserve of free space.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use junto_protocol::error::ErrorCode;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

/// The disk that the files are written to.
pub trait Disk: Send + Sync + 'static {
    /// The bytes that a program without special rights can still write.
    fn free_bytes(&self) -> std::io::Result<u64>;
}

/// The file system of one directory.
pub struct Directory {
    path: PathBuf,
}

impl Directory {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Disk for Directory {
    #[cfg(unix)]
    fn free_bytes(&self) -> std::io::Result<u64> {
        let stats = rustix::fs::statvfs(&self.path)?;
        Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
    }

    #[cfg(not(unix))]
    fn free_bytes(&self) -> std::io::Result<u64> {
        let _ = &self.path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the free space of a directory is read on Unix systems only",
        ))
    }
}

/// A disk whose free space a test sets.
pub struct FakeDisk {
    free: AtomicU64,
}

impl FakeDisk {
    /// The value that makes [`Disk::free_bytes`] fail.
    const UNREADABLE: u64 = u64::MAX;

    pub fn new(free: u64) -> Self {
        Self {
            free: AtomicU64::new(free),
        }
    }

    pub fn set_free(&self, free: u64) {
        self.free.store(free, Ordering::SeqCst);
    }

    pub fn set_unreadable(&self) {
        self.free.store(Self::UNREADABLE, Ordering::SeqCst);
    }
}

impl Disk for FakeDisk {
    fn free_bytes(&self) -> std::io::Result<u64> {
        match self.free.load(Ordering::SeqCst) {
            Self::UNREADABLE => Err(std::io::Error::other("the fake disk cannot be read")),
            free => Ok(free),
        }
    }
}

/// The disk of one directory of the server, whose free space the storage guard reads.
pub struct Space {
    disk: std::sync::Mutex<Option<std::sync::Arc<dyn Disk>>>,
    low: std::sync::atomic::AtomicBool,
}

impl Space {
    /// The file system of `directory`. Without a directory, no space is read.
    pub fn of(directory: Option<PathBuf>) -> Self {
        let disk = directory.map(|directory| {
            let disk: std::sync::Arc<dyn Disk> = std::sync::Arc::new(Directory::new(directory));
            disk
        });
        Self {
            disk: std::sync::Mutex::new(disk),
            low: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Replaces the disk. Only tests do this, so that a test states the free space and
    /// changes no limit (ADR 0904).
    pub fn use_disk(&self, disk: std::sync::Arc<dyn Disk>) {
        *self
            .disk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(disk);
    }

    /// The free bytes. Absent without a directory and when the space cannot be read.
    pub fn free_bytes(&self) -> Option<u64> {
        let disk = self
            .disk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        match disk.free_bytes() {
            Ok(free) => Some(free),
            Err(error) => {
                tracing::warn!(%error, "the free disk space of a directory cannot be read");
                None
            }
        }
    }
}

impl Space {
    /// Whether the free space of the directory is at or below `reserve`. A change is logged,
    /// which is the warning for the administrators (ADR 0006); `what` names the directory in
    /// the log record. Absent without a directory and when the space cannot be read.
    pub fn at_reserve(&self, what: &'static str, reserve: u64) -> Option<bool> {
        let low = at_reserve(self.free_bytes()?, reserve);
        if self.low.swap(low, Ordering::SeqCst) != low {
            if low {
                tracing::warn!(
                    reserve,
                    directory = what,
                    "the free disk space reached the reserve: the server refuses new files \
                     in this directory"
                );
            } else {
                tracing::info!(
                    directory = what,
                    "the free disk space is above the reserve again"
                );
            }
        }
        Some(low)
    }
}

/// The state of the storage guard over several directories: low when one of them is at
/// its reserve, not low when none is and one could be read, and absent when none could be
/// read.
pub fn any_at_reserve(spaces: &[Option<bool>]) -> Option<bool> {
    spaces
        .iter()
        .flatten()
        .copied()
        .reduce(|low, other| low || other)
}

/// What reserves disk space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Recording,
    Attachment,
    Export,
}

impl Kind {
    const fn stored(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Attachment => "attachment",
            Self::Export => "export",
        }
    }
}

/// True when `free` is at or below the reserve: the server starts no job that writes large
/// files, and stops the recordings that run.
pub const fn at_reserve(free: u64, reserve: u64) -> bool {
    free <= reserve
}

fn low() -> ApiError {
    ApiError::new(
        ErrorCode::StorageLow,
        "the server has too little free disk space for a new file operation",
    )
}

/// Reserves `bytes` for `subject`, in the transaction that starts its job. The free space
/// must hold the reserve of the server, every reservation that exists and this one.
/// `free` is absent when the free space could not be read; nothing is reserved then.
pub async fn reserve(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    kind: Kind,
    subject: Uuid,
    bytes: u64,
    free: Option<u64>,
    reserve: u64,
) -> ApiResult<()> {
    let free = free.ok_or_else(low)?;
    let reserved: i64 =
        sqlx::query_scalar("select coalesce(sum(bytes), 0)::bigint from storage_reservations")
            .fetch_one(&mut *connection)
            .await?;
    let needed = u128::from(reserve)
        + u128::from(u64::try_from(reserved).unwrap_or(u64::MAX))
        + u128::from(bytes);
    if u128::from(free) < needed {
        return Err(low());
    }
    let stored = i64::try_from(bytes).map_err(|error| ApiError::internal(&error))?;
    sqlx::query(
        "insert into storage_reservations (subject_id, kind, bytes, reserved_at)
         values ($1, $2, $3, $4)",
    )
    .bind(subject)
    .bind(kind.stored())
    .bind(stored)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// Raises the reservation of `subject` to `total` bytes, of which its files do not yet
/// hold `pending`. The free space must hold the reserve of the server, every other
/// reservation and the pending bytes.
pub async fn reserve_more(
    connection: &mut PgConnection,
    subject: Uuid,
    total: u64,
    pending: u64,
    free: Option<u64>,
    reserve: u64,
) -> ApiResult<()> {
    let free = free.ok_or_else(low)?;
    let others: i64 = sqlx::query_scalar(
        "select coalesce(sum(bytes), 0)::bigint from storage_reservations
         where subject_id <> $1",
    )
    .bind(subject)
    .fetch_one(&mut *connection)
    .await?;
    let needed = u128::from(reserve)
        + u128::from(u64::try_from(others).unwrap_or(u64::MAX))
        + u128::from(pending);
    if u128::from(free) < needed {
        return Err(low());
    }
    let stored = i64::try_from(total).map_err(|error| ApiError::internal(&error))?;
    sqlx::query("update storage_reservations set bytes = $2 where subject_id = $1")
        .bind(subject)
        .bind(stored)
        .execute(connection)
        .await?;
    Ok(())
}

/// Ends the reservation of `subject`: its job writes nothing more.
pub async fn release(connection: &mut PgConnection, subject: Uuid) -> ApiResult<()> {
    sqlx::query("delete from storage_reservations where subject_id = $1")
        .bind(subject)
        .execute(connection)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_is_reached_at_its_own_value() {
        assert!(at_reserve(0, 0));
        assert!(at_reserve(10, 10));
        assert!(at_reserve(9, 10));
        assert!(!at_reserve(11, 10));
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_reports_the_free_space_of_its_file_system() {
        let directory = Directory::new(std::env::temp_dir());
        assert!(directory.free_bytes().unwrap() > 0);
        let absent = Directory::new(PathBuf::from("/nonexistent/junto/recordings"));
        assert!(absent.free_bytes().is_err());
    }

    #[test]
    fn a_fake_disk_reports_what_a_test_sets() {
        let disk = FakeDisk::new(7);
        assert_eq!(disk.free_bytes().unwrap(), 7);
        disk.set_free(9);
        assert_eq!(disk.free_bytes().unwrap(), 9);
        disk.set_unreadable();
        assert!(disk.free_bytes().is_err());
    }
}
