use crate::common::monotonic_ns;
use pipewire as pw;
use pw::proxy::ProxyT;
use std::collections::HashMap;

pub const PLAYBACK_STREAM_CLASS: &str = "Stream/Output/Audio";

pub enum Selector {
    ProcessId(u32),
    Binary(String),
    ApplicationName(String),
    PortalApplicationId(String),
    System { include_self: bool },
}

#[derive(Clone, Debug, Default)]
pub struct ClientIdentity {
    pub reported_process_id: Option<u32>,
    pub socket_process_id: Option<u32>,
    pub binary: Option<String>,
    pub application_name: Option<String>,
    pub api: Option<String>,
    pub portal_application_id: Option<String>,
}

pub struct PlaybackNode {
    pub serial: Option<u32>,
    pub client_id: Option<u32>,
    pub name: String,
    pub application_name: Option<String>,
}

pub struct Port {
    pub node_id: u32,
    pub is_output: bool,
    pub is_monitor: bool,
    pub channel: String,
}

pub struct Graph {
    selector: Selector,
    skip_link_groups: bool,
    started_ns: u64,
    clients: HashMap<u32, ClientIdentity>,
    client_proxies: HashMap<u32, (pw::client::Client, pw::client::ClientListener)>,
    nodes: HashMap<u32, PlaybackNode>,
    node_clients: HashMap<u32, u32>,
    node_link_groups: HashMap<u32, Option<String>>,
    node_proxies: HashMap<u32, (pw::node::Node, pw::node::NodeListener)>,
    ports: HashMap<u32, Port>,
    links: HashMap<(u32, u32), (pw::link::Link, pw::proxy::ProxyListener)>,
    decisions: HashMap<u32, bool>,
    capture_node: Option<u32>,
    stopped: bool,
}

impl Graph {
    pub fn new(selector: Selector, skip_link_groups: bool, started_ns: u64) -> Self {
        Graph {
            selector,
            skip_link_groups,
            started_ns,
            clients: HashMap::new(),
            client_proxies: HashMap::new(),
            nodes: HashMap::new(),
            node_clients: HashMap::new(),
            node_link_groups: HashMap::new(),
            node_proxies: HashMap::new(),
            ports: HashMap::new(),
            links: HashMap::new(),
            decisions: HashMap::new(),
            capture_node: None,
            stopped: false,
        }
    }

    pub fn log(&self, text: &str) {
        let elapsed = (monotonic_ns() - self.started_ns) as f64 / 1e9;
        println!("EVENT t={elapsed:.3} {text}");
    }

    pub fn keep_client_proxy(&mut self, id: u32, client: pw::client::Client, listener: pw::client::ClientListener) {
        self.client_proxies.insert(id, (client, listener));
    }

    pub fn set_client_identity(&mut self, id: u32, identity: ClientIdentity) {
        self.clients.insert(id, identity);
    }

    pub fn add_node(&mut self, id: u32, node: PlaybackNode) {
        self.log(&format!(
            "playback_node_added node={id} object.serial={:?} name={} client={:?}",
            node.serial, node.name, node.client_id
        ));
        self.nodes.insert(id, node);
    }

    pub fn note_node_client(&mut self, node_id: u32, client_id: u32) {
        self.node_clients.insert(node_id, client_id);
    }

    pub fn skips_link_groups(&self) -> bool {
        self.skip_link_groups
    }

    pub fn keep_node_proxy(&mut self, id: u32, node: pw::node::Node, listener: pw::node::NodeListener) {
        self.node_proxies.insert(id, (node, listener));
    }

    pub fn set_node_link_group(&mut self, id: u32, link_group: Option<String>) {
        self.node_link_groups.insert(id, link_group);
    }

    pub fn add_port(&mut self, id: u32, port: Port) {
        self.ports.insert(id, port);
    }

    pub fn set_capture_node(&mut self, id: u32) {
        if self.capture_node != Some(id) {
            self.capture_node = Some(id);
            self.log(&format!("capture_node node={id}"));
        }
    }

    pub fn capture_node(&self) -> Option<u32> {
        self.capture_node
    }

    pub fn remove(&mut self, id: u32) {
        if self.nodes.remove(&id).is_some() {
            self.decisions.remove(&id);
            self.log(&format!("playback_node_removed node={id}"));
        }
        self.node_clients.remove(&id);
        self.node_link_groups.remove(&id);
        self.node_proxies.remove(&id);
        self.ports.remove(&id);
        self.clients.remove(&id);
        self.client_proxies.remove(&id);
        let ports = &self.ports;
        self.links.retain(|(output, input), _| ports.contains_key(output) && ports.contains_key(input));
    }

    pub fn teardown(&mut self) {
        self.stopped = true;
        self.links.clear();
        self.client_proxies.clear();
        self.node_proxies.clear();
    }

    fn decide(&self, node_id: u32, node: &PlaybackNode) -> Option<(bool, String)> {
        if self.skip_link_groups {
            if let Some(link_group) = self.node_link_groups.get(&node_id)? {
                return Some((false, format!("forwarding node of node.link-group={link_group}")));
            }
        }
        let own_client_id = *self.node_clients.get(&self.capture_node?)?;
        let own_socket_process_id = self.clients.get(&own_client_id)?.socket_process_id;
        let identity = match node.client_id {
            Some(client_id) => Some(self.clients.get(&client_id)?),
            None => None,
        };
        let same_connection = node.client_id == Some(own_client_id);
        let same_process = identity.is_some_and(|i| i.socket_process_id.is_some() && i.socket_process_id == own_socket_process_id);
        let is_own = same_connection || same_process;
        let describe = match identity {
            Some(i) => format!(
                "application.process.id={:?} pipewire.sec.pid={:?} application.process.binary={:?} application.name={:?} client.api={:?} pipewire.access.portal.app_id={:?}",
                i.reported_process_id, i.socket_process_id, i.binary, i.application_name, i.api, i.portal_application_id
            ),
            None => "no client".to_string(),
        };
        let wanted = match &self.selector {
            Selector::System { include_self } => *include_self || !is_own,
            Selector::ProcessId(process_id) => !is_own && identity.is_some_and(|i| i.reported_process_id == Some(*process_id)),
            Selector::Binary(binary) => !is_own && identity.is_some_and(|i| i.binary.as_deref() == Some(binary)),
            Selector::PortalApplicationId(id) => !is_own && identity.is_some_and(|i| i.portal_application_id.as_deref() == Some(id)),
            Selector::ApplicationName(name) => {
                !is_own
                    && (node.application_name.as_deref() == Some(name)
                        || identity.is_some_and(|i| i.application_name.as_deref() == Some(name)))
            }
        };
        Some((wanted, format!("own={is_own} (own client={own_client_id}, own pipewire.sec.pid={own_socket_process_id:?}) {describe}")))
    }

    pub fn reconcile(&mut self, core: &pw::core::CoreRc) {
        if self.stopped {
            return;
        }
        let Some(capture_node) = self.capture_node else { return };
        let inputs: Vec<(u32, String)> = self
            .ports
            .iter()
            .filter(|(_, port)| port.node_id == capture_node && !port.is_output)
            .map(|(id, port)| (*id, port.channel.clone()))
            .collect();
        if inputs.len() < crate::common::CHANNELS {
            return;
        }

        let undecided: Vec<u32> = self.nodes.keys().filter(|id| !self.decisions.contains_key(id)).copied().collect();
        for node_id in undecided {
            if let Some((wanted, reason)) = self.decide(node_id, &self.nodes[&node_id]) {
                let verdict = if wanted { "capture" } else { "ignore" };
                self.log(&format!("decision node={node_id} name={} {verdict} {reason}", self.nodes[&node_id].name));
                self.decisions.insert(node_id, wanted);
            }
        }

        let mut wanted_links = Vec::new();
        for (port_id, port) in &self.ports {
            if !port.is_output || port.is_monitor || self.decisions.get(&port.node_id) != Some(&true) {
                continue;
            }
            for (input_id, input_channel) in &inputs {
                if (port.channel == *input_channel || port.channel == "MONO") && !self.links.contains_key(&(*port_id, *input_id)) {
                    wanted_links.push((port.node_id, *port_id, port.channel.clone(), *input_id, input_channel.clone()));
                }
            }
        }

        for (output_node, output_port, output_channel, input_port, input_channel) in wanted_links {
            let properties = pw::properties::properties! {
                "link.output.node" => output_node.to_string(),
                "link.output.port" => output_port.to_string(),
                "link.input.node" => capture_node.to_string(),
                "link.input.port" => input_port.to_string(),
                "object.linger" => "false",
            };
            match core.create_object::<pw::link::Link>("link-factory", &properties) {
                Ok(link) => {
                    let listener = link
                        .upcast_ref()
                        .add_listener_local()
                        .error(move |seq, res, message| {
                            println!("EVENT link_error output_port={output_port} input_port={input_port} seq={seq} res={res} {message}");
                        })
                        .register();
                    self.log(&format!(
                        "link_requested node={output_node} port={output_port}({output_channel}) -> capture port={input_port}({input_channel})"
                    ));
                    self.links.insert((output_port, input_port), (link, listener));
                }
                Err(error) => self.log(&format!("link_failed node={output_node} port={output_port}: {error}")),
            }
        }
    }
}
