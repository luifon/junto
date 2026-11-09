//! The rules that decide which applications a share offers, on the systems where the client
//! lists them itself: macOS (ADR 0736) and Windows (ADR 0706).

use super::{ApplicationId, ApplicationPicture, ApplicationSource, DisplaySource, WindowSource};

/// What the client is to the system: its process, the identity of its program, and its name.
/// macOS gives a bundle identifier and a process name; Windows gives the path of the
/// program file and its file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub process_id: i32,
    pub bundle_id: String,
    pub name: String,
}

impl Own {
    /// Whether an application is the client: this process, or another start of the same
    /// program. Its windows are listed for their picture. Its sound is never offered: the
    /// exclusion of the own sound has no effect on a capture that selects an application of
    /// the same program, and another start plays the sound of its own call (ADR 0502).
    pub(crate) fn is(&self, process_id: i32, bundle_id: &str, name: &str) -> bool {
        process_id == self.process_id || (bundle_id == self.bundle_id && name == self.name)
    }
}

/// The applications whose sound can be shared alone: every listed application with a name
/// except the client, once each.
pub(crate) fn shareable_applications(
    listed: Vec<(ApplicationId, String)>,
    own: &Own,
) -> Vec<ApplicationSource> {
    let mut applications: Vec<ApplicationSource> = Vec::new();
    for (id, name) in listed {
        if name.is_empty()
            || own.is(id.process_id, &id.bundle_id, &name)
            || applications.iter().any(|held| held.id == id)
        {
            continue;
        }
        applications.push(ApplicationSource { id, name });
    }
    applications.sort_by(|one, other| {
        one.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
            .then(one.id.process_id.cmp(&other.id.process_id))
    });
    applications
}

/// The applications of the listed windows that are the client, once each, in the order of
/// the list ([`super::ShareSources::clients`]).
pub(crate) fn clients(windows: &[WindowSource], own: &Own) -> Vec<ApplicationId> {
    let mut clients: Vec<ApplicationId> = Vec::new();
    for window in windows {
        if own.is(
            window.application.process_id,
            &window.application.bundle_id,
            &window.application_name,
        ) && !clients.contains(&window.application)
        {
            clients.push(window.application.clone());
        }
    }
    clients
}

/// A rectangle in the coordinates of the screens of the system: points on macOS, pixels on
/// Windows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    /// The area that two rectangles have in common.
    pub(crate) fn overlap(&self, other: &Self) -> f64 {
        let width = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let height = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        width.max(0.0) * height.max(0.0)
    }
}

/// The applications whose windows can be shared together (ADR 0738), from the windows that
/// a person can share, each with its frame, and the screens with theirs. The share of an
/// application shows one screen: the screen that shows the largest part of the windows of
/// the application, the first listed of equal ones. An application without a window on a
/// screen is not offered. The list has the order of [`shareable_applications`].
pub(crate) fn application_pictures(
    windows: &[(WindowSource, Bounds)],
    displays: &[(DisplaySource, Bounds)],
) -> Vec<ApplicationPicture> {
    let mut applications: Vec<(&ApplicationId, &str)> = Vec::new();
    for (window, _) in windows {
        if !applications
            .iter()
            .any(|(id, _)| **id == window.application)
        {
            applications.push((&window.application, &window.application_name));
        }
    }
    let mut pictures: Vec<ApplicationPicture> = applications
        .into_iter()
        .filter_map(|(application, name)| {
            let own: Vec<&Bounds> = windows
                .iter()
                .filter(|(window, _)| window.application == *application)
                .map(|(_, frame)| frame)
                .collect();
            let (display, frame, shown) = displays
                .iter()
                .map(|(display, frame)| {
                    let shown: f64 = own.iter().map(|window| window.overlap(frame)).sum();
                    (display, frame, shown)
                })
                .fold(
                    None,
                    |best: Option<(&DisplaySource, &Bounds, f64)>, next| match best {
                        Some(best) if best.2 >= next.2 => Some(best),
                        _ => Some(next),
                    },
                )?;
            if shown <= 0.0 {
                return None;
            }
            Some(ApplicationPicture {
                application: application.clone(),
                name: name.to_owned(),
                display: display.id,
                width: display.width,
                height: display.height,
                windows: own
                    .iter()
                    .filter(|window| window.overlap(frame) > 0.0)
                    .count(),
            })
        })
        .collect();
    pictures.sort_by(|one, other| {
        one.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
            .then(
                one.application
                    .process_id
                    .cmp(&other.application.process_id),
            )
    });
    pictures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own {
            process_id: 100,
            bundle_id: "chat.junto.client".to_owned(),
            name: "Junto".to_owned(),
        }
    }

    fn application(process_id: i32, bundle_id: &str) -> ApplicationId {
        ApplicationId {
            process_id,
            bundle_id: bundle_id.to_owned(),
        }
    }

    #[test]
    fn the_client_is_its_own_process_and_every_other_start_of_the_same_program() {
        let own = own();
        assert!(own.is(100, "chat.junto.client", "Junto"));
        // A second instance of the application.
        assert!(own.is(200, "chat.junto.client", "Junto"));
        // Another application, also one that took the process identifier meanwhile.
        assert!(!own.is(200, "example.player", "Player"));
        assert!(own.is(100, "example.player", "Player"));
        // A build without a bundle has no bundle identifier: the name decides.
        let unbundled = Own {
            process_id: 100,
            bundle_id: String::new(),
            name: "junto".to_owned(),
        };
        assert!(unbundled.is(300, "", "junto"));
        assert!(!unbundled.is(300, "", "ffplay"));
        assert!(!unbundled.is(300, "example.player", "junto"));
    }

    #[test]
    fn the_client_is_not_among_the_applications_whose_sound_is_offered() {
        let listed = vec![
            (application(9, "example.zeta"), "zeta".to_owned()),
            (application(100, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(200, "chat.junto.client"), "Junto".to_owned()),
            (application(7, "example.player"), "Player".to_owned()),
            (application(8, "example.nameless"), String::new()),
        ];
        let names: Vec<String> = shareable_applications(listed, &own())
            .into_iter()
            .map(|application| application.name)
            .collect();
        assert_eq!(names, ["Player", "zeta"]);
    }

    /// The windows of the client and of a second start of it are listed; their applications
    /// are named as the client, once each, and none of them is offered for its sound.
    #[test]
    fn the_windows_of_the_client_are_named_and_its_sound_is_not_offered() {
        let window = |id: u32, application: ApplicationId, name: &str| WindowSource {
            id,
            title: format!("window {id}"),
            application,
            application_name: name.to_owned(),
            width: 800,
            height: 600,
        };
        let windows = [
            window(1, application(100, "chat.junto.client"), "Junto"),
            window(2, application(7, "example.player"), "Player"),
            window(3, application(100, "chat.junto.client"), "Junto"),
            window(4, application(200, "chat.junto.client"), "Junto"),
        ];
        assert_eq!(
            clients(&windows, &own()),
            [
                application(100, "chat.junto.client"),
                application(200, "chat.junto.client")
            ]
        );
        let listed = windows
            .iter()
            .map(|window| (window.application.clone(), window.application_name.clone()))
            .collect();
        let names: Vec<String> = shareable_applications(listed, &own())
            .into_iter()
            .map(|application| application.name)
            .collect();
        assert_eq!(names, ["Player"]);
        assert!(clients(&windows[1..2], &own()).is_empty());
    }

    #[test]
    fn an_application_is_offered_on_the_screen_that_shows_most_of_its_windows() {
        let frame = |x: f64, y: f64, width: f64, height: f64| Bounds {
            x,
            y,
            width,
            height,
        };
        let window = |id: u32, application: &ApplicationId, name: &str| WindowSource {
            id,
            title: format!("window {id}"),
            application: application.clone(),
            application_name: name.to_owned(),
            width: 100,
            height: 100,
        };
        let display = |id: u32| DisplaySource {
            id,
            width: 1920,
            height: 1080,
        };
        let (editor, player, hidden) = (
            application(7, "example.editor"),
            application(8, "example.player"),
            application(9, "example.hidden"),
        );
        let displays = [
            (display(1), frame(0.0, 0.0, 1920.0, 1080.0)),
            (display(2), frame(1920.0, 0.0, 1920.0, 1080.0)),
        ];
        let windows = [
            // The editor shows a small window on screen 1 and a large one on screen 2,
            // and a window that lies across both.
            (
                window(1, &editor, "editor"),
                frame(10.0, 10.0, 100.0, 100.0),
            ),
            (
                window(2, &editor, "editor"),
                frame(2000.0, 10.0, 1000.0, 800.0),
            ),
            (
                window(3, &editor, "editor"),
                frame(1800.0, 10.0, 400.0, 100.0),
            ),
            // The player lies exactly across the edge: the first screen is taken.
            (
                window(4, &player, "Player"),
                frame(1820.0, 0.0, 200.0, 100.0),
            ),
            // A window outside every screen.
            (
                window(5, &hidden, "Hidden"),
                frame(5000.0, 5000.0, 100.0, 100.0),
            ),
        ];
        let pictures = application_pictures(&windows, &displays);
        let listed: Vec<(&str, u32, usize)> = pictures
            .iter()
            .map(|picture| (picture.name.as_str(), picture.display, picture.windows))
            .collect();
        assert_eq!(listed, [("editor", 2, 2), ("Player", 1, 1)]);
        assert_eq!(pictures[0].application, editor);
        assert_eq!((pictures[0].width, pictures[0].height), (1920, 1080));
        assert!(application_pictures(&[], &displays).is_empty());
        assert!(application_pictures(&windows, &[]).is_empty());
    }
}
