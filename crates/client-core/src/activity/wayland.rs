use super::Shared;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, Proxy as _, QueueHandle,
    backend::WaylandError,
    delegate_noop,
    protocol::{wl_callback, wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

struct State {
    shared: Arc<Shared>,
    manager: Option<(u32, ext_idle_notifier_v1::ExtIdleNotifierV1)>,
    seats: HashMap<u32, wl_seat::WlSeat>,
    notifications: HashMap<u32, ext_idle_notification_v1::ExtIdleNotificationV1>,
    idle: HashSet<u32>,
    globals_done: bool,
}
impl State {
    fn add_notifications(&mut self, queue: &QueueHandle<Self>) {
        let Some((_, manager)) = &self.manager else {
            return;
        };
        for (id, seat) in &self.seats {
            self.notifications.entry(*id).or_insert_with(|| {
                let timeout = super::IDLE_AFTER.as_millis() as u32;
                if manager.version() >= 2 {
                    manager.get_input_idle_notification(timeout, seat, queue, *id)
                } else {
                    manager.get_idle_notification(timeout, seat, queue, *id)
                }
            });
        }
        self.publish();
    }
    fn publish(&self) {
        self.shared.set(
            (!self.notifications.is_empty())
                .then(|| self.notifications.keys().all(|id| self.idle.contains(id))),
        );
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_seat" => {
                state
                    .seats
                    .insert(name, registry.bind(name, version.min(1), queue, ()));
                state.add_notifications(queue);
            }
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "ext_idle_notifier_v1" => {
                state.manager = Some((name, registry.bind(name, version.min(2), queue, ())));
                state.add_notifications(queue);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(notification) = state.notifications.remove(&name) {
                    notification.destroy();
                }
                state.seats.remove(&name);
                state.idle.remove(&name);
                if state.manager.as_ref().is_some_and(|(id, _)| *id == name) {
                    for (_, notification) in state.notifications.drain() {
                        notification.destroy();
                    }
                    state.manager.take();
                    state.idle.clear();
                }
                state.publish();
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.globals_done = true;
    }
}
impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        seat: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                state.idle.insert(*seat);
            }
            ext_idle_notification_v1::Event::Resumed => {
                state.idle.remove(seat);
            }
            _ => {}
        }
        state.publish();
    }
}
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);

pub(super) fn run(shared: &Arc<Shared>) -> bool {
    let Ok(connection) = Connection::connect_to_env() else {
        return false;
    };
    let mut queue = connection.new_event_queue::<State>();
    let handle = queue.handle();
    let _registry = connection.display().get_registry(&handle, ());
    let _sync = connection.display().sync(&handle, ());
    let mut state = State {
        shared: shared.clone(),
        manager: None,
        seats: HashMap::new(),
        notifications: HashMap::new(),
        idle: HashSet::new(),
        globals_done: false,
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !shared.stopped() {
        if queue.dispatch_pending(&mut state).is_err() {
            return false;
        }
        if state.globals_done {
            let idle = shared.idle_duration();
            for reader in shared.take_readers() {
                let _ = reader.send(idle);
            }
        }
        if state.globals_done && state.notifications.is_empty() {
            return false;
        }
        if !state.globals_done && Instant::now() >= deadline {
            return false;
        }
        let writing = match connection.flush() {
            Ok(()) => false,
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => true,
            Err(_) => return false,
        };
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut descriptors = [PollFd::new(
            &connection,
            if writing {
                PollFlags::IN | PollFlags::OUT
            } else {
                PollFlags::IN
            },
        )];
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 200_000_000,
        };
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return false,
            Ok(_) => {}
        }
        let flags = descriptors[0].revents();
        if flags.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
            return false;
        }
        if flags.contains(PollFlags::IN) {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
        }
    }
    for reader in shared.take_readers() {
        let _ = reader.send(None);
    }
    true
}
