use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use livekit::prelude::*;
use livekit_api::access_token::{AccessToken, VideoGrants};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::cli::ConnectArgs;

pub fn join_token(args: &ConnectArgs) -> Result<String> {
    if let Some(token) = &args.token {
        return Ok(token.clone());
    }
    let key = args.api_key.as_deref().ok_or_else(|| anyhow!("no --token and no LIVEKIT_API_KEY"))?;
    let secret =
        args.api_secret.as_deref().ok_or_else(|| anyhow!("no --token and no LIVEKIT_API_SECRET"))?;
    let room = args.room.as_deref().ok_or_else(|| anyhow!("--room is required without --token"))?;
    let identity =
        args.identity.as_deref().ok_or_else(|| anyhow!("--identity is required without --token"))?;
    AccessToken::with_api_key(key, secret)
        .with_identity(identity)
        .with_name(identity)
        .with_ttl(Duration::from_secs(3600))
        .with_grants(VideoGrants { room_join: true, room: room.to_string(), ..Default::default() })
        .to_jwt()
        .context("creating the join token")
}

pub async fn connect(
    args: &ConnectArgs,
    auto_subscribe: bool,
) -> Result<(Room, UnboundedReceiver<RoomEvent>)> {
    let token = join_token(args)?;
    let mut options = RoomOptions::default();
    options.auto_subscribe = auto_subscribe;
    Room::connect(&args.url, &token, options).await.context("connecting to the room")
}

pub fn source_name(source: TrackSource) -> &'static str {
    match source {
        TrackSource::Camera => "camera",
        TrackSource::Microphone => "microphone",
        TrackSource::Screenshare => "screen_share",
        TrackSource::ScreenshareAudio => "screen_share_audio",
        TrackSource::Unknown => "unknown",
    }
}
