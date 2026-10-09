use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "test-participant", about = "Synthetic LiveKit participant for media tests")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Join a room and publish generated audio and video.
    Publish(PublishArgs),
    /// Join a room, subscribe to every track and report what arrives.
    Subscribe(SubscribeArgs),
}

#[derive(Args, Clone)]
pub struct ConnectArgs {
    #[arg(long, env = "LIVEKIT_URL", default_value = "ws://localhost:7880")]
    pub url: String,
    /// A ready join token. Without it, a token is created from the API key and secret.
    #[arg(long, env = "LIVEKIT_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, env = "LIVEKIT_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,
    #[arg(long, env = "LIVEKIT_API_SECRET", hide_env_values = true)]
    pub api_secret: Option<String>,
    /// Room name. Required when the tool creates its own token.
    #[arg(long)]
    pub room: Option<String>,
    /// Participant identity. Required when the tool creates its own token.
    #[arg(long)]
    pub identity: Option<String>,
}

#[derive(Args, Clone)]
pub struct PublishArgs {
    #[command(flatten)]
    pub connect: ConnectArgs,

    /// Frequency of the microphone tone in Hz.
    #[arg(long, default_value_t = 440.0)]
    pub mic_hz: f64,
    /// Do not publish a microphone track.
    #[arg(long)]
    pub no_mic: bool,
    /// Peak level of every tone, from 0 to 1.
    #[arg(long, default_value_t = 0.25)]
    pub level: f64,
    /// Time marker: at every wall-clock multiple of this period (Unix time), for
    /// --marker-gap-ms, the microphone tone is silent and the lower third of the
    /// video patterns is white. Share audio has no marker.
    #[arg(long)]
    pub marker_period_ms: Option<u64>,
    #[arg(long, default_value_t = 300)]
    pub marker_gap_ms: u64,

    /// Publish a camera track with a test pattern.
    #[arg(long)]
    pub camera: bool,
    #[arg(long, default_value = "640x360")]
    pub camera_size: String,
    #[arg(long, default_value_t = 15)]
    pub camera_fps: u32,

    /// Publish a screen-share video track with a test pattern.
    #[arg(long)]
    pub screen: bool,
    #[arg(long, default_value = "1280x720")]
    pub screen_size: String,
    #[arg(long, default_value_t = 5)]
    pub screen_fps: u32,
    /// Background colour of the screen pattern as RRGGBB.
    #[arg(long)]
    pub screen_color: Option<String>,
    /// Publish a screen-share audio track with a tone of this frequency in Hz.
    #[arg(long)]
    pub screen_audio_hz: Option<f64>,
    /// Start the screen share (video and audio) this many seconds after start.
    #[arg(long, default_value_t = 0.0)]
    pub screen_after: f64,
    /// Stop the screen share this many seconds after start.
    #[arg(long)]
    pub screen_stop_after: Option<f64>,

    /// Text drawn on the video patterns. Defaults to the identity.
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub simulcast: bool,

    /// Mute the microphone this many seconds after start.
    #[arg(long)]
    pub mute_after: Option<f64>,
    /// Unmute the microphone this many seconds after start.
    #[arg(long)]
    pub unmute_after: Option<f64>,
    /// Disconnect and connect again (new connection, new track identifiers)
    /// this many seconds after start.
    #[arg(long)]
    pub reconnect_after: Option<f64>,
    /// Seconds to stay disconnected during a reconnect.
    #[arg(long, default_value_t = 2.0)]
    pub reconnect_gap: f64,
    /// Leave the room and exit this many seconds after start.
    #[arg(long)]
    pub leave_after: Option<f64>,
}

#[derive(Args, Clone)]
pub struct SubscribeArgs {
    #[command(flatten)]
    pub connect: ConnectArgs,

    /// Stop and print the summary after this many seconds. Without it, run until Ctrl-C.
    #[arg(long)]
    pub duration: Option<f64>,
    /// Seconds between report lines.
    #[arg(long, default_value_t = 5.0)]
    pub report_every: f64,
    /// Expected dominant tone, as identity/source=hz (source: microphone or
    /// screen_share_audio). Repeatable. The exit status is 1 when one is not met.
    #[arg(long = "expect")]
    pub expectations: Vec<String>,
}
