//! Exports of a channel or of a direct-message conversation (ADR 0411).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::ExportId;
use crate::message::MessagePlace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    /// The export waits for its turn.
    Waiting,
    /// The server writes the file.
    Running,
    /// The file can be downloaded until `expires_at`.
    Ready,
    Failed,
}

/// Why an export has no file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExportFailure {
    /// The server has too little free disk space for the file (ADR 0006).
    StorageLow,
    /// The account lost the permission for the export while the file was written.
    AccessEnded,
    /// The server stopped three times while it wrote the file.
    Interrupted,
    /// The server could not read a file of the export or could not write the file.
    Failed,
}

/// One export. It belongs to the account that asked for it; no other account reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Export {
    /// The export holds the items of its place whose identifier is not greater than this
    /// one: what existed when the export was asked for.
    pub id: ExportId,
    pub place: MessagePlace,
    pub state: ExportState,
    /// Present with the state `failed`.
    pub failure: Option<ExportFailure>,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// When the server removes the export. Present when the export has finished.
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// The size of the file. Present with the state `ready`.
    pub bytes: Option<u64>,
}

/// Why the server removed an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExportRemoval {
    /// The account that asked for the export removed it.
    Removed,
    /// Seven days have passed since the export finished.
    Expired,
    /// A message, a file or a recording that the export holds, or can hold, was deleted.
    ContentDeleted,
    /// The channel of the export was deleted.
    PlaceDeleted,
    /// The account was banned or removed.
    AccountEnded,
}

/// The file `export.json` of an export file: what the file is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportManifest {
    /// The version of the layout of the file: 1.
    pub format: u32,
    pub export_id: ExportId,
    pub place: MessagePlace,
    /// The name of the channel. Absent for a direct-message conversation.
    pub channel_name: Option<String>,
    pub requested_by: crate::ids::AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    pub messages: u64,
    pub attachments: u64,
    pub recordings: u64,
}

/// The layout version of an export file that this build writes.
pub const EXPORT_FORMAT: u32 = 1;

/// The file `recordings/<recording>/recording.json` of an export file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportedRecording {
    pub recording: crate::recording::RecordingDetail,
    /// The name of the file of each segment that the export holds, by segment.
    pub files: Vec<ExportedSegmentFile>,
    /// True when the export holds `transcript.txt` for the recording.
    pub transcript: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportedSegmentFile {
    pub segment_id: crate::ids::RecordingSegmentId,
    pub name: String,
}
