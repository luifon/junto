//! Selection of playback ports; registry identifiers are never application identities.

use std::collections::{BTreeMap, BTreeSet};

use crate::share::{ApplicationId, ApplicationSource, SoundChoice};

pub(super) const CHANNELS: [&str; 9] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR", "MONO"];

#[derive(Clone, Debug, Default)]
pub(super) struct Client {
    pub nonce: String,
    pub socket_pid: Option<u32>,
    pub portal_id: String,
    pub binary: String,
    pub name: String,
}

impl Client {
    fn identity(&self) -> Option<ApplicationId> {
        let bundle_id = if !self.portal_id.is_empty() {
            format!("portal:{}", self.portal_id)
        } else if !self.binary.is_empty() && !self.name.is_empty() {
            // The separators make (ab, c) different from (a, bc).
            format!("native:{}\0{}", self.binary, self.name)
        } else {
            return None;
        };
        Some(ApplicationId {
            process_id: 0,
            bundle_id,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Node {
    pub client: Option<u32>,
    pub playback: bool,
    pub known: bool,
    pub forwarding: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Port {
    pub node: u32,
    pub output: bool,
    pub monitor: bool,
    pub channel: String,
}

#[derive(Default)]
pub(super) struct Graph {
    pub clients: BTreeMap<u32, Client>,
    pub nodes: BTreeMap<u32, Node>,
    pub ports: BTreeMap<u32, Port>,
    pub nonce: String,
    pub capture: Option<u32>,
}

impl Graph {
    fn own(&self) -> Option<(u32, &Client)> {
        self.clients
            .iter()
            .find(|(_, client)| !self.nonce.is_empty() && client.nonce == self.nonce)
            .map(|(id, client)| (*id, client))
    }

    fn application(&self, node: &Node) -> Option<(&Client, ApplicationId)> {
        if !node.known || !node.playback || node.forwarding {
            return None;
        }
        let (own_id, own) = self.own()?;
        let own_pid = own.socket_pid?;
        let client_id = node.client?;
        let client = self.clients.get(&client_id)?;
        client.socket_pid?;
        if client_id == own_id
            || client.socket_pid == Some(own_pid)
            || (!own.portal_id.is_empty() && own.portal_id == client.portal_id)
            || (!own.binary.is_empty() && own.binary == client.binary)
        {
            return None;
        }
        Some((client, client.identity()?))
    }

    fn unsupported_applications(&self) -> BTreeSet<String> {
        self.nodes
            .iter()
            .filter_map(|(node_id, node)| {
                let (_, application) = self.application(node)?;
                self.ports
                    .values()
                    .any(|port| {
                        port.node == *node_id
                            && port.output
                            && !port.monitor
                            && !CHANNELS.contains(&port.channel.as_str())
                    })
                    .then_some(application.bundle_id)
            })
            .collect()
    }

    pub fn system_available(&self) -> bool {
        self.ready() && self.unsupported_applications().is_empty()
    }

    pub fn applications(&self) -> Vec<ApplicationSource> {
        let mut applications = BTreeMap::new();
        let unsupported = self.unsupported_applications();
        for node in self.nodes.values() {
            if let Some((client, id)) = self.application(node) {
                if unsupported.contains(&id.bundle_id) {
                    continue;
                }
                let name = if client.name.is_empty() {
                    client.portal_id.clone()
                } else if client.portal_id.is_empty() && client.binary != client.name {
                    format!("{} ({})", client.name, client.binary)
                } else {
                    client.name.clone()
                };
                applications
                    .entry(id.bundle_id.clone())
                    .or_insert(ApplicationSource { id, name });
            }
        }
        applications.into_values().collect()
    }

    pub fn ready(&self) -> bool {
        self.own().is_some_and(|(_, own)| own.socket_pid.is_some())
    }

    /// The output and input port pairs that the capture alone should own.
    pub fn links(&self, choice: &SoundChoice) -> Result<BTreeSet<(u32, u32)>, String> {
        let Some(capture) = self.capture else {
            return Ok(BTreeSet::new());
        };
        let inputs: BTreeMap<&str, u32> = self
            .ports
            .iter()
            .filter(|(_, port)| port.node == capture && !port.output)
            .map(|(id, port)| (port.channel.as_str(), *id))
            .collect();
        // Do not send a partial layout while the stream's ports are being announced.
        if !CHANNELS.iter().all(|channel| inputs.contains_key(channel)) {
            return Ok(BTreeSet::new());
        }
        let mut links = BTreeSet::new();
        for (id, port) in &self.ports {
            if !port.output || port.monitor {
                continue;
            }
            let Some(node) = self.nodes.get(&port.node) else {
                continue;
            };
            let Some((_, application)) = self.application(node) else {
                continue;
            };
            let selected = match choice {
                SoundChoice::None => false,
                SoundChoice::System => true,
                SoundChoice::Application(chosen) => chosen == &application,
            };
            if !selected {
                continue;
            }
            let Some(input) = inputs.get(port.channel.as_str()) else {
                return Err(format!(
                    "the shared application uses an unsupported sound channel: {}",
                    port.channel
                ));
            };
            links.insert((*id, *input));
        }
        Ok(links)
    }

    pub fn remove(&mut self, id: u32) {
        self.clients.remove(&id);
        self.nodes.remove(&id);
        self.nodes.retain(|_, node| node.client != Some(id));
        self.ports.remove(&id);
        self.ports.retain(|_, port| {
            port.node != id
                && (Some(port.node) == self.capture || self.nodes.contains_key(&port.node))
        });
        if self.capture == Some(id) {
            self.capture = None;
        }
    }
}

/// Stereo downmix, retaining centre speech and surround channels.
pub(super) fn stereo(frame: [f32; 9]) -> [f32; 2] {
    let [
        left,
        right,
        centre,
        sub,
        rear_left,
        rear_right,
        side_left,
        side_right,
        mono,
    ] = frame;
    let centre = centre * std::f32::consts::FRAC_1_SQRT_2 + sub * 0.5 + mono;
    [
        left + centre + (rear_left + side_left) * std::f32::consts::FRAC_1_SQRT_2,
        right + centre + (rear_right + side_right) * std::f32::consts::FRAC_1_SQRT_2,
    ]
    .map(|value| {
        if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn scene() -> Graph {
        let mut graph = Graph {
            nonce: "private".into(),
            capture: Some(10),
            ..Graph::default()
        };
        graph.clients.insert(
            1,
            Client {
                nonce: "private".into(),
                socket_pid: Some(80),
                binary: "junto".into(),
                name: "Junto".into(),
                ..Client::default()
            },
        );
        graph.clients.insert(
            2,
            Client {
                socket_pid: Some(90),
                binary: "player".into(),
                name: "Music".into(),
                ..Client::default()
            },
        );
        graph.nodes.insert(
            20,
            Node {
                client: Some(2),
                playback: true,
                known: true,
                forwarding: false,
            },
        );
        for (index, channel) in CHANNELS.iter().enumerate() {
            graph.ports.insert(
                100 + index as u32,
                Port {
                    node: 10,
                    output: false,
                    monitor: false,
                    channel: (*channel).into(),
                },
            );
        }
        graph.ports.insert(
            200,
            Port {
                node: 20,
                output: true,
                monitor: false,
                channel: "FL".into(),
            },
        );
        graph
    }

    #[test]
    fn own_sound_and_pending_or_forwarded_nodes_never_get_links() {
        let mut graph = scene();
        assert_eq!(
            graph.links(&SoundChoice::System).unwrap(),
            BTreeSet::from([(200, 100)])
        );
        graph.nodes.get_mut(&20).unwrap().known = false;
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
        graph.nodes.get_mut(&20).unwrap().known = true;
        graph.nodes.get_mut(&20).unwrap().forwarding = true;
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
        graph.nodes.get_mut(&20).unwrap().forwarding = false;
        graph.clients.get_mut(&2).unwrap().socket_pid = Some(80);
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
        graph.clients.get_mut(&2).unwrap().socket_pid = Some(90);
        graph.clients.get_mut(&1).unwrap().socket_pid = None;
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
    }

    #[test]
    fn identities_survive_restarts_but_not_identifier_reuse_or_name_collisions() {
        let mut graph = scene();
        let choice = SoundChoice::Application(graph.applications()[0].id.clone());
        assert_eq!(graph.links(&choice).unwrap().len(), 1);
        graph.clients.get_mut(&2).unwrap().socket_pid = Some(91);
        assert_eq!(graph.links(&choice).unwrap().len(), 1);
        graph.clients.get_mut(&2).unwrap().binary = "other".into();
        assert!(graph.links(&choice).unwrap().is_empty());
        graph.clients.get_mut(&2).unwrap().binary = "player".into();
        graph.clients.get_mut(&2).unwrap().portal_id = "sandbox.one".into();
        let choice = SoundChoice::Application(graph.applications()[0].id.clone());
        graph.clients.get_mut(&2).unwrap().portal_id = "sandbox.two".into();
        assert!(graph.links(&choice).unwrap().is_empty());
        graph.remove(20);
        graph.nodes.insert(
            20,
            Node {
                client: Some(2),
                playback: true,
                ..Node::default()
            },
        );
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
    }

    #[test]
    fn a_changed_channel_or_selection_never_keeps_the_old_links() {
        let mut graph = scene();
        assert!(graph.links(&SoundChoice::None).unwrap().is_empty());
        graph.ports.get_mut(&200).unwrap().channel = "AUX0".into();
        assert!(!graph.system_available());
        assert!(graph.applications().is_empty());
        assert!(graph.links(&SoundChoice::System).is_err());
        graph.ports.get_mut(&200).unwrap().channel = "MONO".into();
        assert_eq!(
            graph.links(&SoundChoice::System).unwrap(),
            BTreeSet::from([(200, 108)])
        );
        graph.remove(200);
        assert!(graph.links(&SoundChoice::System).unwrap().is_empty());
    }

    #[test]
    fn mono_centre_and_surround_are_retained_without_changing_the_stereo_pair() {
        assert_eq!(
            stereo([0.1, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            [0.1, 0.2]
        );
        assert_eq!(
            stereo([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3]),
            [0.3, 0.3]
        );
        let speech = stereo([0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(speech[0] > 0.35 && speech[0] == speech[1]);
        let surround = stereo([0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        assert!(surround[0] > 0.35 && surround[1] == 0.0);
        assert_eq!(stereo([f32::NAN; 9]), [0.0, 0.0]);
    }
}
