//! Bound registry objects and the explicit links owned by one sound capture.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use pipewire as pw;
use pw::proxy::ProxyT;
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;

use super::graph::{Client, Graph, Node, Port};
use super::{Life, error};
use crate::share::SoundChoice;

/// A bound object and its listener. Rust releases fields in the order of their declaration,
/// and the release of a listener writes into the object it listens to: the listener is first.
struct Bound<P, L> {
    _listener: L,
    _proxy: P,
}

impl<P, L> Bound<P, L> {
    fn new(proxy: P, listener: L) -> Self {
        Self {
            _listener: listener,
            _proxy: proxy,
        }
    }
}

#[derive(Default)]
struct Objects {
    clients: BTreeMap<u32, Bound<pw::client::Client, pw::client::ClientListener>>,
    nodes: BTreeMap<u32, Bound<pw::node::Node, pw::node::NodeListener>>,
    ports: BTreeMap<u32, Bound<pw::port::Port, pw::port::PortListener>>,
    links: BTreeMap<(u32, u32), Bound<pw::link::Link, pw::proxy::ProxyListener>>,
    /// The links whose making the daemon refused, by their ports.
    refused: Rc<RefCell<BTreeSet<(u32, u32)>>>,
}

pub(super) struct Registry {
    _listener: pw::registry::Listener,
    _registry: pw::registry::RegistryRc,
    pub graph: Rc<RefCell<Graph>>,
    objects: Rc<RefCell<Objects>>,
    core: pw::core::CoreRc,
    choice: SoundChoice,
    life: std::sync::Arc<Life>,
}

fn number(props: &DictRef, key: &str) -> Option<u32> {
    props.get(key)?.parse().ok()
}
fn text(props: &DictRef, key: &str) -> String {
    props.get(key).unwrap_or("").to_owned()
}
fn client(props: &DictRef) -> Client {
    Client {
        nonce: text(props, "junto.capture.token"),
        socket_pid: number(props, "pipewire.sec.pid"),
        portal_id: text(props, "pipewire.access.portal.app_id"),
        binary: text(props, "application.process.binary"),
        name: text(props, "application.name"),
    }
}
fn node(props: &DictRef, known: bool) -> Node {
    Node {
        client: number(props, "client.id"),
        playback: props.get("media.class") == Some("Stream/Output/Audio"),
        known,
        forwarding: props.get("node.link-group").is_some(),
    }
}
fn port(props: &DictRef) -> Option<Port> {
    Some(Port {
        node: number(props, "node.id")?,
        output: props.get("port.direction") == Some("out"),
        monitor: props.get("port.monitor") == Some("true"),
        channel: text(props, "audio.channel"),
    })
}

fn reconcile(
    graph: &Graph,
    objects: &mut Objects,
    core: &pw::core::CoreRc,
    choice: &SoundChoice,
    life: &Life,
) {
    if life.stopped() {
        objects.links.clear();
        return;
    }
    let wanted = match graph.links(choice) {
        Ok(wanted) => wanted,
        Err(reason) => {
            objects.links.clear();
            life.fail(error(reason));
            return;
        }
    };
    // A port can end between its announcement and the request for its link. The daemon
    // then refuses the link; the object of the refused link is released here, outside its
    // own event, and the link is asked for again while the graph still wants it.
    let refused = std::mem::take(&mut *objects.refused.borrow_mut());
    objects
        .links
        .retain(|ports, _| wanted.contains(ports) && !refused.contains(ports));
    for (output, input) in wanted {
        if objects.links.contains_key(&(output, input)) || refused.contains(&(output, input)) {
            continue;
        }
        let (Some(from), Some(to)) = (graph.ports.get(&output), graph.ports.get(&input)) else {
            continue;
        };
        let properties = pw::properties::properties! {
            "link.output.node" => from.node.to_string(), "link.output.port" => output.to_string(),
            "link.input.node" => to.node.to_string(), "link.input.port" => input.to_string(),
            "object.linger" => "false",
        };
        match core.create_object::<pw::link::Link>("link-factory", &properties) {
            Ok(link) => {
                let listener = link
                    .upcast_ref()
                    .add_listener_local()
                    .error({
                        let refused = objects.refused.clone();
                        move |_, _, message| {
                            super::connection::in_callback("PipeWire registry error", || {
                                tracing::debug!(output, input, message, "a sound link was refused");
                                refused.borrow_mut().insert((output, input));
                            })
                        }
                    })
                    .register();
                objects
                    .links
                    .insert((output, input), Bound::new(link, listener));
            }
            Err(failure) => {
                objects.links.clear();
                life.fail(error(failure));
                return;
            }
        }
    }
}

impl Registry {
    pub fn new(
        core: pw::core::CoreRc,
        nonce: String,
        choice: SoundChoice,
        life: std::sync::Arc<Life>,
    ) -> Result<Self, crate::share::CaptureError> {
        let registry = core.get_registry_rc().map_err(error)?;
        let graph = Rc::new(RefCell::new(Graph {
            nonce,
            ..Graph::default()
        }));
        let objects = Rc::new(RefCell::new(Objects::default()));
        let changed: Rc<dyn Fn()> = {
            let graph = Rc::downgrade(&graph);
            let objects = Rc::downgrade(&objects);
            let core = core.clone();
            let choice = choice.clone();
            let life = life.clone();
            Rc::new(move || {
                if let (Some(graph), Some(objects)) = (graph.upgrade(), objects.upgrade()) {
                    reconcile(
                        &graph.borrow(),
                        &mut objects.borrow_mut(),
                        &core,
                        &choice,
                        &life,
                    );
                }
            })
        };
        let listener = registry
            .add_listener_local()
            .global({
                let registry = registry.downgrade();
                let graph = Rc::downgrade(&graph);
                let objects = Rc::downgrade(&objects);
                let changed = changed.clone();
                let life = life.clone();
                move |global| {
                    super::connection::in_callback("PipeWire registry global", || {
                        let (Some(registry), Some(graph), Some(objects)) =
                            (registry.upgrade(), graph.upgrade(), objects.upgrade())
                        else {
                            return;
                        };
                        let id = global.id;
                        let Some(props) = global.props else {
                            return;
                        };
                        match global.type_ {
                            ObjectType::Client => {
                                let bound = match registry.bind::<pw::client::Client, _>(global) {
                                    Ok(bound) => bound,
                                    Err(failure) => {
                                        life.fail(error(failure));
                                        return;
                                    }
                                };
                                let listener = bound
                                    .add_listener_local()
                                    .info({
                                        let graph = Rc::downgrade(&graph);
                                        let changed = changed.clone();
                                        move |info| {
                                            super::connection::in_callback(
                                                "PipeWire registry info",
                                                || {
                                                    if let (Some(graph), Some(props)) =
                                                        (graph.upgrade(), info.props())
                                                    {
                                                        graph
                                                            .borrow_mut()
                                                            .clients
                                                            .insert(id, client(props));
                                                        changed();
                                                    }
                                                },
                                            )
                                        }
                                    })
                                    .register();
                                objects
                                    .borrow_mut()
                                    .clients
                                    .insert(id, Bound::new(bound, listener));
                            }
                            ObjectType::Node => {
                                let initial = node(props, false);
                                graph.borrow_mut().nodes.insert(id, initial.clone());
                                let bound = match registry.bind::<pw::node::Node, _>(global) {
                                    Ok(bound) => bound,
                                    Err(failure) => {
                                        life.fail(error(failure));
                                        return;
                                    }
                                };
                                let listener = bound
                                    .add_listener_local()
                                    .info({
                                        let graph = Rc::downgrade(&graph);
                                        let changed = changed.clone();
                                        move |info| {
                                            super::connection::in_callback(
                                                "PipeWire registry info",
                                                || {
                                                    if let (Some(graph), Some(props)) =
                                                        (graph.upgrade(), info.props())
                                                    {
                                                        let mut current = node(props, true);
                                                        current.client =
                                                            current.client.or(initial.client);
                                                        if props.get("media.class").is_none() {
                                                            current.playback = initial.playback;
                                                        }
                                                        graph
                                                            .borrow_mut()
                                                            .nodes
                                                            .insert(id, current);
                                                        changed();
                                                    }
                                                },
                                            )
                                        }
                                    })
                                    .register();
                                objects
                                    .borrow_mut()
                                    .nodes
                                    .insert(id, Bound::new(bound, listener));
                            }
                            ObjectType::Port => {
                                let initial = port(props);
                                // A port's bound information can change without replacing its registry ID.
                                let bound = match registry.bind::<pw::port::Port, _>(global) {
                                    Ok(bound) => bound,
                                    Err(failure) => {
                                        life.fail(error(failure));
                                        return;
                                    }
                                };
                                let listener = bound
                                    .add_listener_local()
                                    .info({
                                        let graph = Rc::downgrade(&graph);
                                        let changed = changed.clone();
                                        move |info| {
                                            super::connection::in_callback(
                                                "PipeWire registry info",
                                                || {
                                                    if let (Some(graph), Some(props)) =
                                                        (graph.upgrade(), info.props())
                                                    {
                                                        let mut held = graph.borrow_mut();
                                                        held.ports.remove(&id);
                                                        let current = port(props).or_else(|| {
                                                            initial.as_ref().map(|base| Port {
                                                                node: base.node,
                                                                output: props
                                                                    .get("port.direction")
                                                                    .map_or(
                                                                        base.output,
                                                                        |direction| {
                                                                            direction == "out"
                                                                        },
                                                                    ),
                                                                monitor: props
                                                                    .get("port.monitor")
                                                                    .map_or(
                                                                        base.monitor,
                                                                        |monitor| monitor == "true",
                                                                    ),
                                                                channel: props
                                                                    .get("audio.channel")
                                                                    .unwrap_or(&base.channel)
                                                                    .to_owned(),
                                                            })
                                                        });
                                                        if let Some(port) = current {
                                                            held.ports.insert(id, port);
                                                        }
                                                        drop(held);
                                                        changed();
                                                    }
                                                },
                                            )
                                        }
                                    })
                                    .register();
                                objects
                                    .borrow_mut()
                                    .ports
                                    .insert(id, Bound::new(bound, listener));
                            }
                            _ => {}
                        }
                        changed();
                    })
                }
            })
            .global_remove({
                let graph = Rc::downgrade(&graph);
                let objects = Rc::downgrade(&objects);
                move |id| {
                    super::connection::in_callback("PipeWire registry global_remove", || {
                        if let (Some(graph), Some(objects)) = (graph.upgrade(), objects.upgrade()) {
                            graph.borrow_mut().remove(id);
                            let mut held = objects.borrow_mut();
                            held.clients.remove(&id);
                            held.nodes.remove(&id);
                            held.ports.remove(&id);
                            drop(held);
                            changed();
                        }
                    })
                }
            })
            .register();
        Ok(Self {
            _listener: listener,
            _registry: registry,
            graph,
            objects,
            core,
            choice,
            life,
        })
    }

    pub fn set_capture(&self, node: u32) {
        self.graph.borrow_mut().capture = Some(node);
        reconcile(
            &self.graph.borrow(),
            &mut self.objects.borrow_mut(),
            &self.core,
            &self.choice,
            &self.life,
        );
    }
}
