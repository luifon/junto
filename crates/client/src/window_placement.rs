use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Size, WindowBounds, point, px, size};
use junto_client_core::window_placement::WindowPlacement;
use std::rc::Rc;

pub(crate) struct Restored {
    pub bounds: Bounds<Pixels>,
    pub display: Option<DisplayId>,
    pub minimum: Size<Pixels>,
}

fn valid(bounds: Bounds<Pixels>) -> bool {
    [
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
    ]
    .into_iter()
    .all(|value| f32::from(value).is_finite())
        && bounds.size.width > px(0.)
        && bounds.size.height > px(0.)
}

pub(crate) fn restore(
    saved: WindowPlacement,
    minimum: Size<Pixels>,
    displays: &[Rc<dyn PlatformDisplay>],
    primary: Option<DisplayId>,
) -> Restored {
    let usable = |display: &&Rc<dyn PlatformDisplay>| {
        valid(display.visible_bounds()) && valid(display.bounds())
    };
    let display = displays
        .iter()
        .filter(usable)
        .find(|display| saved.display.is_some() && display.uuid().ok() == saved.display)
        .or_else(|| {
            displays
                .iter()
                .filter(usable)
                .find(|display| Some(display.id()) == primary)
        })
        .or_else(|| displays.iter().find(usable));
    let requested = size(
        px(saved.width as f32).max(minimum.width),
        px(saved.height as f32).max(minimum.height),
    );
    let Some(display) = display else {
        return Restored {
            bounds: Bounds::new(point(px(0.), px(0.)), requested),
            display: None,
            minimum,
        };
    };
    let work = display.visible_bounds();
    let dimensions = requested.min(&work.size);
    let mut bounds = Bounds::centered_at(work.center(), dimensions);
    if saved.display.is_some()
        && display.uuid().ok() == saved.display
        && let Some((x, y)) = saved.x.zip(saved.y)
    {
        let offset = saved
            .display_origin
            .map_or(point(px(0.), px(0.)), |(x, y)| {
                display.bounds().origin - point(px(x as f32), px(y as f32))
            });
        bounds.origin = point(px(x as f32), px(y as f32)) + offset;
    }
    bounds.origin.x = bounds.origin.x.clamp(
        work.origin.x,
        work.origin.x + work.size.width - dimensions.width,
    );
    bounds.origin.y = bounds.origin.y.clamp(
        work.origin.y,
        work.origin.y + work.size.height - dimensions.height,
    );
    Restored {
        bounds,
        display: Some(display.id()),
        minimum: minimum.min(&work.size),
    }
}

pub(crate) fn remember(
    mut saved: WindowPlacement,
    bounds: WindowBounds,
    display: Option<Rc<dyn PlatformDisplay>>,
) -> Option<WindowPlacement> {
    match bounds {
        WindowBounds::Fullscreen(_) => return None,
        WindowBounds::Windowed(bounds) => {
            if !valid(bounds) {
                return None;
            }
            saved.x = Some(f32::from(bounds.origin.x) as i32);
            saved.y = Some(f32::from(bounds.origin.y) as i32);
            saved.width = f32::from(bounds.size.width) as u32;
            saved.height = f32::from(bounds.size.height) as u32;
            saved.maximized = false;
        }
        WindowBounds::Maximized(_) => {
            saved.maximized = true;
            if let Some(display) = &display
                && saved.display != display.uuid().ok()
            {
                let moved = saved.display_origin.zip(saved.x.zip(saved.y)).map(
                    |((old_x, old_y), (x, y))| {
                        let origin = display.bounds().origin;
                        (
                            (f32::from(origin.x) + (x as f32 - old_x as f32)) as i32,
                            (f32::from(origin.y) + (y as f32 - old_y as f32)) as i32,
                        )
                    },
                );
                saved.x = moved.map(|(x, _)| x);
                saved.y = moved.map(|(_, y)| y);
            }
        }
    }
    if let Some(display) = display {
        saved.display = display.uuid().ok();
        saved.display_origin = Some((
            f32::from(display.bounds().origin.x) as i32,
            f32::from(display.bounds().origin.y) as i32,
        ));
    }
    Some(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)]
    struct Display {
        uuid: uuid::Uuid,
        bounds: Bounds<Pixels>,
        visible: Bounds<Pixels>,
    }
    impl PlatformDisplay for Display {
        fn id(&self) -> DisplayId {
            DisplayId::new(1)
        }
        fn uuid(&self) -> gpui::Result<uuid::Uuid> {
            Ok(self.uuid)
        }
        fn bounds(&self) -> Bounds<Pixels> {
            self.bounds
        }
        fn visible_bounds(&self) -> Bounds<Pixels> {
            self.visible
        }
    }
    fn screen(origin: (f32, f32), dimensions: (f32, f32)) -> Rc<dyn PlatformDisplay> {
        Rc::new(Display {
            uuid: uuid::Uuid::new_v4(),
            bounds: Bounds::new(
                point(px(origin.0), px(origin.1)),
                size(px(dimensions.0), px(dimensions.1)),
            ),
            visible: Bounds::new(
                point(px(origin.0), px(origin.1 + 30.)),
                size(px(dimensions.0), px(dimensions.1 - 70.)),
            ),
        })
    }
    #[test]
    fn moving_a_display_keeps_the_windows_local_position_inside_its_work_area() {
        let display = screen((-1600., -200.), (1600., 1000.));
        let saved = WindowPlacement {
            display: display.uuid().ok(),
            display_origin: Some((0, 0)),
            x: Some(100),
            y: Some(60),
            width: 900,
            height: 600,
            ..WindowPlacement::main_window()
        };
        let restored = restore(saved, size(px(720.), px(480.)), &[display], None);
        assert_eq!(restored.bounds.origin, point(px(-1500.), px(-140.)));
        assert_eq!(restored.bounds.size, size(px(900.), px(600.)));
    }
    #[test]
    fn a_removed_monitor_or_invalid_position_cannot_hide_the_window() {
        let display = screen((0., 0.), (1280., 800.));
        let saved = WindowPlacement {
            display: Some(uuid::Uuid::new_v4()),
            x: Some(90000),
            y: Some(-90000),
            width: 7600,
            height: 4200,
            ..WindowPlacement::main_window()
        };
        let restored = restore(saved, size(px(720.), px(480.)), &[display], None);
        assert_eq!(
            restored.bounds,
            Bounds::new(point(px(0.), px(30.)), size(px(1280.), px(730.)))
        );
    }
    #[test]
    fn the_scaled_minimum_is_capped_by_a_smaller_display() {
        let display = screen((0., 0.), (640., 480.));
        let restored = restore(
            WindowPlacement::main_window(),
            size(px(1080.), px(720.)),
            &[display],
            None,
        );
        assert_eq!(restored.minimum, size(px(640.), px(410.)));
        assert_eq!(restored.bounds.size, restored.minimum);
    }
    #[test]
    fn maximizing_does_not_replace_the_normal_geometry_and_fullscreen_does_not_write() {
        let display = screen((0., 0.), (1920., 1080.));
        let saved = remember(
            WindowPlacement::main_window(),
            WindowBounds::Windowed(Bounds::new(
                point(px(100.), px(80.)),
                size(px(900.), px(600.)),
            )),
            Some(display.clone()),
        )
        .unwrap();
        let maximized = remember(
            saved,
            WindowBounds::Maximized(display.bounds()),
            Some(display.clone()),
        )
        .unwrap();
        assert!(maximized.maximized);
        assert_eq!(
            (maximized.x, maximized.y, maximized.width, maximized.height),
            (saved.x, saved.y, saved.width, saved.height)
        );
        assert!(
            remember(
                maximized,
                WindowBounds::Fullscreen(display.bounds()),
                Some(display)
            )
            .is_none()
        );
    }
    #[test]
    fn legacy_global_positions_are_clamped_and_absent_displays_use_a_safe_origin() {
        let display = screen((-1200., 0.), (1200., 900.));
        let saved = WindowPlacement {
            display: display.uuid().ok(),
            x: Some(-1100),
            y: Some(60),
            width: 800,
            height: 600,
            ..WindowPlacement::default()
        };
        assert_eq!(
            restore(saved, size(px(480.), px(320.)), &[display], None)
                .bounds
                .origin,
            point(px(-1100.), px(60.))
        );
        assert_eq!(
            restore(saved, size(px(480.), px(320.)), &[], None)
                .bounds
                .origin,
            point(px(0.), px(0.))
        );
    }
}
