//! The client of the LiveKit server API (ADR 0508): four calls of the room service over
//! Twirp with JSON bodies, each authorized with a token that the server signs for it.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use junto_protocol::secret::CallToken;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::clock::Clock;
use crate::config::MediaSettings;
use crate::media::token;
use crate::media::{
    JoinGrant, JoinToken, MediaError, MediaResult, MediaServer, Notification, Participant,
    ParticipantKind, Publishing, Room, Track, TrackKind, TrackSource,
};

const SERVICE: &str = "twirp/livekit.RoomService";

pub struct LiveKit {
    http: reqwest::Client,
    settings: MediaSettings,
    clock: Arc<dyn Clock>,
}

/// A value of an enumeration, which the API writes as its name and reads as name or number.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum Named {
    Name(String),
    Number(i64),
    #[default]
    Absent,
}

impl Named {
    fn is(&self, name: &str, number: i64) -> bool {
        match self {
            Self::Name(text) => text == name,
            Self::Number(value) => *value == number,
            Self::Absent => number == 0,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RoomList {
    #[serde(default)]
    rooms: Vec<RoomInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct RoomInfo {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ParticipantList {
    #[serde(default)]
    participants: Vec<ParticipantInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct ParticipantInfo {
    #[serde(default)]
    identity: String,
    #[serde(default)]
    state: Named,
    #[serde(default)]
    kind: Named,
    #[serde(default)]
    permission: Option<PermissionInfo>,
    #[serde(default)]
    tracks: Vec<TrackInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct PermissionInfo {
    #[serde(default, alias = "canPublish")]
    can_publish: bool,
    #[serde(default, alias = "canPublishSources")]
    can_publish_sources: Vec<Named>,
}

#[derive(Debug, Default, Deserialize)]
struct TrackInfo {
    #[serde(default, rename = "type")]
    kind: Named,
    #[serde(default)]
    source: Named,
    #[serde(default)]
    muted: bool,
}

#[derive(Debug, Default, Deserialize)]
struct TwirpError {
    #[serde(default)]
    code: String,
}

const SOURCES: [(TrackSource, &str, i64); 5] = [
    (TrackSource::Unknown, "UNKNOWN", 0),
    (TrackSource::Camera, "CAMERA", 1),
    (TrackSource::Microphone, "MICROPHONE", 2),
    (TrackSource::ScreenShare, "SCREEN_SHARE", 3),
    (TrackSource::ScreenShareAudio, "SCREEN_SHARE_AUDIO", 4),
];

fn source(named: &Named) -> TrackSource {
    SOURCES
        .iter()
        .find(|(_, name, number)| named.is(name, *number))
        .map_or(TrackSource::Unknown, |(source, _, _)| *source)
}

fn participant(info: ParticipantInfo) -> Participant {
    let kind = if info.kind.is("STANDARD", 0) {
        ParticipantKind::Standard
    } else if info.kind.is("EGRESS", 2) {
        ParticipantKind::Recorder
    } else {
        ParticipantKind::Other
    };
    let publishing = info.permission.map(|permission| {
        let sources: BTreeSet<TrackSource> = if permission.can_publish {
            permission.can_publish_sources.iter().map(source).collect()
        } else {
            BTreeSet::new()
        };
        (permission.can_publish, sources)
    });
    let publishing = match publishing {
        Some((true, sources)) if sources.is_empty() => None,
        Some((_, sources)) => Some(Publishing { sources }),
        None => None,
    };
    Participant {
        identity: info.identity,
        kind,
        connected: !info.state.is("DISCONNECTED", 3),
        publishing,
        tracks: info
            .tracks
            .iter()
            .map(|track| Track {
                kind: if track.kind.is("AUDIO", 0) {
                    TrackKind::Audio
                } else if track.kind.is("VIDEO", 1) {
                    TrackKind::Video
                } else {
                    TrackKind::Data
                },
                source: source(&track.source),
                muted: track.muted,
            })
            .collect(),
    }
}

enum Answer {
    Body(Value),
    NotFound,
}

impl LiveKit {
    pub fn new(settings: MediaSettings, clock: Arc<dyn Clock>) -> MediaResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(settings.request_timeout.unsigned_abs())
            .connect_timeout(settings.request_timeout.unsigned_abs())
            .build()
            .map_err(|error| MediaError::Refused(error.to_string()))?;
        Ok(Self {
            http,
            settings,
            clock,
        })
    }

    fn secret(&self) -> &[u8] {
        self.settings.api_secret.expose().as_bytes()
    }

    async fn call(&self, method: &str, room: Option<&str>, body: Value) -> MediaResult<Answer> {
        let claims = token::api_claims(&self.settings.api_key, room, self.clock.now());
        let authorization = token::sign(self.secret(), &claims)
            .ok_or_else(|| MediaError::Refused("the API token could not be signed".to_owned()))?;
        let url = format!(
            "{}/{SERVICE}/{method}",
            self.settings.api_url.trim_end_matches('/')
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(authorization)
            .json(&body)
            .send()
            .await
            .map_err(|error| MediaError::Unreachable(error.without_url().to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| MediaError::Unreachable(error.without_url().to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map(Answer::Body)
                .map_err(|error| MediaError::Unreachable(format!("{method}: {error}")));
        }
        let code = serde_json::from_slice::<TwirpError>(&bytes)
            .map(|error| error.code)
            .unwrap_or_default();
        if code == "not_found" {
            Ok(Answer::NotFound)
        } else if status.is_server_error() || code == "unavailable" {
            Err(MediaError::Unreachable(format!(
                "{method}: {status} {code}"
            )))
        } else {
            Err(MediaError::Refused(format!("{method}: {status} {code}")))
        }
    }
}

fn read<T: serde::de::DeserializeOwned + Default>(method: &str, answer: Answer) -> MediaResult<T> {
    match answer {
        Answer::NotFound => Ok(T::default()),
        Answer::Body(body) => serde_json::from_value(body)
            .map_err(|error| MediaError::Unreachable(format!("{method}: {error}"))),
    }
}

impl MediaServer for LiveKit {
    fn client_url(&self) -> &str {
        &self.settings.client_url
    }

    fn join_token(&self, grant: &JoinGrant<'_>) -> MediaResult<JoinToken> {
        let claims = token::join_claims(&self.settings.api_key, grant);
        let token = token::sign(self.secret(), &claims)
            .ok_or_else(|| MediaError::Refused("the join token could not be signed".to_owned()))?;
        Ok(JoinToken {
            token: CallToken::new(token),
            expires_at: grant.now + grant.lifetime,
        })
    }

    fn list_rooms(&self) -> BoxFuture<'_, MediaResult<Vec<Room>>> {
        async move {
            let answer = self.call("ListRooms", None, json!({})).await?;
            let list: RoomList = read("ListRooms", answer)?;
            Ok(list
                .rooms
                .into_iter()
                .map(|room| Room { name: room.name })
                .collect())
        }
        .boxed()
    }

    fn list_participants<'a>(
        &'a self,
        room: &'a str,
    ) -> BoxFuture<'a, MediaResult<Vec<Participant>>> {
        async move {
            let answer = self
                .call("ListParticipants", Some(room), json!({ "room": room }))
                .await?;
            let list: ParticipantList = read("ListParticipants", answer)?;
            Ok(list.participants.into_iter().map(participant).collect())
        }
        .boxed()
    }

    fn remove_participant<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
    ) -> BoxFuture<'a, MediaResult<()>> {
        async move {
            let body = json!({ "room": room, "identity": identity });
            self.call("RemoveParticipant", Some(room), body).await?;
            Ok(())
        }
        .boxed()
    }

    fn update_publishing<'a>(
        &'a self,
        room: &'a str,
        identity: &'a str,
        publishing: &'a Publishing,
    ) -> BoxFuture<'a, MediaResult<()>> {
        async move {
            let sources: Vec<&str> = publishing
                .sources
                .iter()
                .map(|source| source.api_name())
                .collect();
            let body = json!({
                "room": room,
                "identity": identity,
                "permission": {
                    "can_subscribe": true,
                    "can_publish": !sources.is_empty(),
                    "can_publish_data": false,
                    "can_publish_sources": sources,
                    "can_update_metadata": false,
                },
            });
            self.call("UpdateParticipant", Some(room), body).await?;
            Ok(())
        }
        .boxed()
    }

    fn verify_notification(
        &self,
        authorization: &str,
        body: &[u8],
        now: OffsetDateTime,
    ) -> Option<Notification> {
        token::notification(
            &self.settings.api_key,
            self.secret(),
            authorization,
            body,
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_participant_is_read_from_the_names_and_the_numbers_of_the_api() {
        let listed: ParticipantList = serde_json::from_value(json!({
            "participants": [
                {
                    "sid": "PA_1", "identity": "account:seat", "state": "ACTIVE",
                    "joined_at": "1800000000", "kind": "STANDARD",
                    "permission": {
                        "can_subscribe": true, "can_publish": true,
                        "can_publish_sources": ["CAMERA", "SCREEN_SHARE"]
                    },
                    "tracks": [
                        { "sid": "TR_1", "type": "AUDIO", "source": "MICROPHONE", "muted": true },
                        { "sid": "TR_2", "type": "VIDEO", "source": "SCREEN_SHARE" },
                        { "sid": "TR_3", "type": 0, "source": 4 },
                        { "sid": "TR_4", "type": "AUDIO", "source": "LATER_SOURCE" }
                    ]
                },
                { "identity": "EG_1", "state": "ACTIVE", "kind": "EGRESS" },
                { "identity": "old", "state": 3, "kind": 0,
                  "permission": { "canPublish": true, "canPublishSources": [] } },
                { "identity": "agent", "kind": "AGENT", "permission": { "can_publish": false } }
            ]
        }))
        .unwrap();
        let participants: Vec<Participant> =
            listed.participants.into_iter().map(participant).collect();

        let person = &participants[0];
        assert_eq!(person.identity, "account:seat");
        assert_eq!(person.kind, ParticipantKind::Standard);
        assert!(person.connected);
        assert_eq!(person.publishing, Some(Publishing::pictures_only()));
        assert_eq!(
            person.tracks,
            [
                Track {
                    kind: TrackKind::Audio,
                    source: TrackSource::Microphone,
                    muted: true
                },
                Track {
                    kind: TrackKind::Video,
                    source: TrackSource::ScreenShare,
                    muted: false
                },
                Track {
                    kind: TrackKind::Audio,
                    source: TrackSource::ScreenShareAudio,
                    muted: false
                },
                Track {
                    kind: TrackKind::Audio,
                    source: TrackSource::Unknown,
                    muted: false
                },
            ]
        );
        assert!(person.publishes(TrackSource::ScreenShare));
        assert!(!person.publishes(TrackSource::Camera));

        assert_eq!(participants[1].kind, ParticipantKind::Recorder);
        assert_eq!(participants[1].publishing, None);
        assert!(!participants[2].connected);
        assert_eq!(participants[2].publishing, None);
        assert_eq!(participants[3].kind, ParticipantKind::Other);
        assert_eq!(
            participants[3].publishing,
            Some(Publishing {
                sources: BTreeSet::new()
            })
        );

        let empty: ParticipantList = serde_json::from_value(json!({})).unwrap();
        assert!(empty.participants.is_empty());
        let rooms: RoomList =
            serde_json::from_value(json!({ "rooms": [{ "name": "jr-a-b", "sid": "RM_1" }] }))
                .unwrap();
        assert_eq!(rooms.rooms[0].name, "jr-a-b");
    }
}
