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

/// What reserves disk space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Recording,
}

impl Kind {
    const fn stored(self) -> &'static str {
        match self {
            Self::Recording => "recording",
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
        "the server has too little free disk space for a new recording",
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
