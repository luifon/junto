//! Sending the profile preset to an account that this device created (ADR 0105).
//!
//! The server has no operation yet that sets the name and the avatar of an account. The
//! client side is complete up to that call: it records per server that the preset waits,
//! builds what is sent, counts the tries that failed, and records the success. The call
//! itself is behind [`ProfileUploader`]; the uploader of the product answers that the
//! server does not offer the operation, and the preset keeps waiting.

use std::future::Future;
use std::pin::Pin;

use crate::api::{ApiError, ServerApi};
use crate::profile::AvatarFormat;

/// What a new account takes from the preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileUpdate {
    pub name: String,
    pub avatar: Option<(AvatarFormat, Vec<u8>)>,
}

pub type UploadFuture = Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'static>>;

/// Sends a profile to the account of a session.
pub trait ProfileUploader: Send + Sync + 'static {
    fn upload(&self, api: ServerApi, update: ProfileUpdate) -> UploadFuture;
}

/// The uploader until the server has the operation: it sends nothing and reports that the
/// operation is not offered.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaitsForServerOperation;

pub const PROFILE_OPERATION: &str = "set the name and the avatar of an account";

impl ProfileUploader for WaitsForServerOperation {
    fn upload(&self, _api: ServerApi, _update: ProfileUpdate) -> UploadFuture {
        Box::pin(async {
            Err(ApiError::NotOffered {
                operation: PROFILE_OPERATION,
            })
        })
    }
}
