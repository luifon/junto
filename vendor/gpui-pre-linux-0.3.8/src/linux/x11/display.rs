// Modified: expose RandR monitors and the current desktop's usable work area.
use anyhow::Context as _;
use uuid::Uuid;
use x11rb::{
    connection::Connection as _,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{AtomEnum, ConnectionExt as _},
    },
    xcb_ffi::XCBConnection,
};

use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Point, point, px, size};

#[derive(Debug)]
pub(crate) struct X11Display {
    id: DisplayId,
    bounds: Bounds<Pixels>,
    visible: Bounds<Pixels>,
    uuid: Uuid,
    primary: bool,
}

impl X11Display {
    pub(crate) fn new(xcb: &XCBConnection, scale: f32, screen: usize) -> anyhow::Result<Self> {
        let mut displays = Self::all(xcb, scale, screen)?;
        let primary = displays
            .iter()
            .position(|display| display.primary)
            .unwrap_or(0);
        Ok(displays.swap_remove(primary))
    }

    pub(crate) fn at(
        xcb: &XCBConnection,
        scale: f32,
        screen: usize,
        location: Point<Pixels>,
    ) -> anyhow::Result<Self> {
        let mut displays = Self::all(xcb, scale, screen)?;
        let selected = displays
            .iter()
            .position(|display| display.bounds.contains(&location))
            .or_else(|| displays.iter().position(|display| display.primary))
            .unwrap_or(0);
        Ok(displays.swap_remove(selected))
    }

    pub(crate) fn all(
        xcb: &XCBConnection,
        scale: f32,
        screen_index: usize,
    ) -> anyhow::Result<Vec<Self>> {
        let screen = xcb
            .setup()
            .roots
            .get(screen_index)
            .with_context(|| format!("No screen found with index {screen_index}"))?;
        let work = work_area(xcb, screen.root, scale);
        let monitors = xcb
            .randr_get_monitors(screen.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let mut displays = Vec::new();
        if let Some(monitors) = monitors {
            for monitor in monitors.monitors {
                if monitor.width == 0 || monitor.height == 0 {
                    continue;
                }
                let Ok(name) = xcb.get_atom_name(monitor.name)?.reply() else {
                    continue;
                };
                let bounds = Bounds::new(
                    point(px(monitor.x as f32 / scale), px(monitor.y as f32 / scale)),
                    size(
                        px(monitor.width as f32 / scale),
                        px(monitor.height as f32 / scale),
                    ),
                );
                let mut identity = screen_index.to_le_bytes().to_vec();
                identity.extend(name.name);
                displays.push(Self {
                    id: DisplayId::new(((screen_index as u64) << 32) | u64::from(monitor.name)),
                    bounds,
                    visible: usable(bounds, work),
                    uuid: Uuid::new_v5(&Uuid::NAMESPACE_OID, &identity),
                    primary: monitor.primary,
                });
            }
        }
        if displays.is_empty() {
            let bounds = Bounds::new(
                point(px(0.), px(0.)),
                size(
                    px(screen.width_in_pixels as f32 / scale),
                    px(screen.height_in_pixels as f32 / scale),
                ),
            );
            displays.push(Self {
                id: DisplayId::new((screen_index as u64) << 32),
                bounds,
                visible: usable(bounds, work),
                uuid: Uuid::new_v5(&Uuid::NAMESPACE_OID, &screen_index.to_le_bytes()),
                primary: true,
            });
        }
        Ok(displays)
    }
}

fn usable(bounds: Bounds<Pixels>, work: Option<Bounds<Pixels>>) -> Bounds<Pixels> {
    let visible = work.map(|work| bounds.intersect(&work)).unwrap_or(bounds);
    if visible.size.width > px(0.) && visible.size.height > px(0.) {
        visible
    } else {
        bounds
    }
}

fn work_area(xcb: &XCBConnection, root: u32, scale: f32) -> Option<Bounds<Pixels>> {
    let cardinal = |name: &[u8]| -> Option<Vec<u32>> {
        let atom = xcb.intern_atom(true, name).ok()?.reply().ok()?.atom;
        if atom == 0 {
            return None;
        }
        xcb.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1024)
            .ok()?
            .reply()
            .ok()?
            .value32()
            .map(Iterator::collect)
    };
    let desktop = cardinal(b"_NET_CURRENT_DESKTOP")
        .and_then(|values| values.first().copied())
        .unwrap_or(0) as usize;
    let values = cardinal(b"_NET_WORKAREA")?;
    let [x, y, width, height] =
        *values.get(desktop.checked_mul(4)?..desktop.checked_mul(4)?.checked_add(4)?)?
    else {
        return None;
    };
    Some(Bounds::new(
        point(px(x as i32 as f32 / scale), px(y as i32 as f32 / scale)),
        size(px(width as f32 / scale), px(height as f32 / scale)),
    ))
}

impl PlatformDisplay for X11Display {
    fn id(&self) -> DisplayId {
        self.id
    }
    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }
    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.visible
    }
}
