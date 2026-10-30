use junto_protocol::message::Message;

use crate::error::{ApiError, ApiResult};

pub const PAGE_BYTES: usize = 3 * 1024 * 1024;

pub fn bound(messages: &mut Vec<Message>, ascending: bool) -> ApiResult<bool> {
    let mut bytes = 2;
    let mut kept = 0;
    for at in 0..messages.len() {
        let index = if ascending {
            at
        } else {
            messages.len() - 1 - at
        };
        let length = serde_json::to_vec(&messages[index])
            .map_err(|error| ApiError::internal(&error))?
            .len()
            + 1;
        if bytes + length > PAGE_BYTES {
            break;
        }
        kept += 1;
        bytes += length;
    }
    if kept == 0 && !messages.is_empty() {
        return Err(ApiError::internal(
            &"a message exceeds the history page budget",
        ));
    }
    let more = kept < messages.len();
    if ascending {
        messages.truncate(kept);
    } else {
        messages.drain(..messages.len() - kept);
    }
    Ok(more)
}
