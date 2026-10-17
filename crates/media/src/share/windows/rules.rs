//! The decisions of the Windows capture as plain functions: which windows are listed, how a
//! screen and a window are named by a number, how the client and an application are
//! identified, which applications started the client, and how a packet of the sound capture
//! becomes stereo frames. They run in the ordinary tests of every system (ADR 0706).

use crate::share::listing::{Own, shareable_applications};
use crate::share::{ApplicationId, ApplicationSource, WindowSource};

/// A window is listed when each side has at least this many pixels. Smaller ones are parts
/// of an interface that a system or an application draws as windows.
pub(crate) const SMALLEST_WINDOW: u32 = 64;

/// The window classes of the shell itself: the desktop and the task bars. They are visible
/// top-level windows with a title, and a share of one of them shows no application.
const SHELL_CLASSES: [&str; 4] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// The program that draws the frames of the applications of the Windows store. The
/// application of such a window is the program of its inner window.
pub(crate) const FRAME_HOST: &str = "applicationframehost.exe";

/// A top-level window as the system describes it, before the rule of the list is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedWindow {
    pub id: u32,
    pub title: String,
    pub class: String,
    pub visible: bool,
    /// The window manager hides it: it is on another virtual desktop, or it belongs to a
    /// suspended application of the store.
    pub cloaked: bool,
    pub minimized: bool,
    /// `WS_EX_TOOLWINDOW`: a floating tool bar or a palette.
    pub tool: bool,
    /// `WS_EX_APPWINDOW`: the application asks for the window to be shown in the task bar.
    pub app_window: bool,
    /// `WS_EX_NOACTIVATE`: the window never takes the input focus.
    pub no_activate: bool,
    /// The window has an owner window: it is a dialog or a pop-up of another window.
    pub owned: bool,
    /// The size of the visible frame in pixels.
    pub size: (u32, u32),
    /// The application and its name, when the program of the window could be read.
    pub application: Option<(ApplicationId, String)>,
}

/// Whether a window is one that a person switches to: the rule of the task bar and of
/// Alt+Tab. A window that asks for the task bar is shown; otherwise an owned window, a tool
/// window and a window that never takes the focus are not.
fn switchable(window: &ListedWindow) -> bool {
    window.app_window || !(window.owned || window.tool || window.no_activate)
}

/// The windows that a person can share: visible application windows of another application
/// than the client, on the present virtual desktop, not minimized.
pub(crate) fn shareable_windows(listed: Vec<ListedWindow>, own: &Own) -> Vec<WindowSource> {
    listed
        .into_iter()
        .filter(|window| window.visible && !window.cloaked && !window.minimized)
        .filter(switchable)
        .filter(|window| !window.title.trim().is_empty())
        .filter(|window| !SHELL_CLASSES.contains(&window.class.as_str()))
        .filter(|window| window.size.0 >= SMALLEST_WINDOW && window.size.1 >= SMALLEST_WINDOW)
        .filter_map(|window| {
            let (application, application_name) = window.application?;
            if own.is(
                application.process_id,
                &application.bundle_id,
                &application_name,
            ) {
                return None;
            }
            Some(WindowSource {
                id: window.id,
                title: window.title,
                application,
                application_name,
                width: window.size.0,
                height: window.size.1,
            })
        })
        .collect()
}

/// The number of a screen from the device name that the system gives it, `\\.\DISPLAY2`.
pub(crate) fn display_number(device: &str) -> Option<u32> {
    let number = device.strip_prefix(r"\\.\DISPLAY")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse().ok().filter(|number| *number > 0)
}

/// The number of a window from its handle. A handle of a window has 32 significant bits on
/// 64-bit Windows, so that 32-bit programs can use it: the number holds the whole handle.
pub(crate) fn window_number(handle: isize) -> u32 {
    handle as u32
}

/// The handle of a window from its number: the lower 32 bits, extended with their sign, as
/// the system extends the handle of a 32-bit program.
pub(crate) fn window_handle(number: u32) -> isize {
    number as i32 as isize
}

/// The identity of a program from the path of its file: the path in lower case, because the
/// file system of Windows does not tell case apart, and the file name.
pub(crate) fn program_identity(path: &str) -> (String, String) {
    let lower = path.to_lowercase();
    let file = lower
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&lower)
        .to_owned();
    (lower, file)
}

/// The name of an application as a person reads it: the description in the version
/// information of its program file, or the file name without its extension.
pub(crate) fn application_name(description: Option<&str>, path: &str) -> String {
    if let Some(description) = description.map(str::trim).filter(|text| !text.is_empty()) {
        return description.to_owned();
    }
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => file.to_owned(),
    }
}

/// The client as the capture of Windows identifies it, from its process and the path of its
/// program file. Another start of the same program has the same path and name.
pub(crate) fn own(process_id: u32, path: &str, description: Option<&str>) -> Own {
    Own {
        process_id: process_id as i32,
        bundle_id: program_identity(path).0,
        name: application_name(description, path),
    }
}

/// The application of a process, as the lists of a share give it.
pub(crate) fn application(
    process_id: u32,
    path: &str,
    description: Option<&str>,
) -> (ApplicationId, String) {
    (
        ApplicationId {
            process_id: process_id as i32,
            bundle_id: program_identity(path).0,
        },
        application_name(description, path),
    )
}

/// A process as the process list of the system gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListedProcess {
    pub id: u32,
    /// The identifier of the process that created it, recorded at its creation. That
    /// process can have ended since, and its identifier can belong to a newer process.
    pub parent: u32,
}

/// The processes that started the client, as far as the process list of the system shows
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Creators {
    /// These processes, the nearest first.
    Known(Vec<u32>),
    /// The process list could not be read or does not hold the client, so any process can
    /// have started it.
    Unknown,
}

/// The processes that started the client: the process that created it, the process that
/// created that one, and so on. `processes` is the process list, nothing when it could not
/// be read, and `created` gives the creation time of a process.
///
/// A creator is older than every process below it. A process that is younger than a process
/// below it whose creation time is known, the client at least, took the identifier of an
/// ended creator: the chain ends there, as it ends at a creator that is not in the list. A
/// process whose creation time cannot be read is counted as a creator, and so is any process
/// when the list is unknown: an application that is withheld by mistake costs one choice of
/// sound, an application that is offered by mistake would carry the sound of the call.
pub(crate) fn ancestors(
    own: u32,
    processes: Option<&[ListedProcess]>,
    created: impl Fn(u32) -> Option<u64>,
) -> Creators {
    let Some(processes) = processes else {
        return Creators::Unknown;
    };
    let mut found: Vec<u32> = Vec::new();
    let mut child = own;
    // The creation time of the nearest process below the next creator whose time is known:
    // each accepted creator is older, so it is also the oldest known time of the chain.
    let mut limit = created(own);
    let mut listed = false;
    while let Some(entry) = processes.iter().find(|process| process.id == child) {
        listed = true;
        let parent = entry.parent;
        if parent == 0
            || parent == own
            || found.contains(&parent)
            || !processes.iter().any(|process| process.id == parent)
        {
            break;
        }
        let parent_created = created(parent);
        if let (Some(parent_time), Some(limit_time)) = (parent_created, limit)
            && parent_time > limit_time
        {
            break;
        }
        found.push(parent);
        child = parent;
        if parent_created.is_some() {
            limit = parent_created;
        }
    }
    if listed {
        Creators::Known(found)
    } else {
        Creators::Unknown
    }
}

/// The choices of the sound of one application: the applications whose sound can be shared
/// alone, and the applications of listed windows whose process started the client. Process
/// loopback captures the sound of an application with the sound of every process that it
/// started, so the sound of such an application includes the sound of the call that the
/// client plays (ADR 0706). With unknown creators no application is offered and none is
/// named: the lists state that the creators are unknown.
pub(crate) fn sound_choices(
    windows: &[WindowSource],
    own: &Own,
    creators: &Creators,
) -> (Vec<ApplicationSource>, Vec<ApplicationId>) {
    let Creators::Known(ancestors) = creators else {
        return (Vec::new(), Vec::new());
    };
    let started = |id: &ApplicationId| {
        u32::try_from(id.process_id).is_ok_and(|process| ancestors.contains(&process))
    };
    let applications = shareable_applications(
        windows
            .iter()
            .filter(|window| !started(&window.application))
            .map(|window| (window.application.clone(), window.application_name.clone()))
            .collect(),
        own,
    );
    let mut started_the_client: Vec<ApplicationId> = Vec::new();
    for window in windows {
        if started(&window.application) && !started_the_client.contains(&window.application) {
            started_the_client.push(window.application.clone());
        }
    }
    (applications, started_the_client)
}

/// The command line of the arguments of a program, so that `CommandLineToArgvW` and the start
/// code of the C runtime give each argument back as it is. An empty argument and an argument
/// with a space, a tab, a line end or a quote are put in quotes. Inside quotes a quote is
/// written as `\"`, and the backslashes before a quote or before the closing quote are
/// doubled; other backslashes stay as they are.
#[cfg_attr(not(feature = "test-support"), allow(dead_code))]
pub(crate) fn command_line(arguments: &[Vec<u16>]) -> Vec<u16> {
    const SPACE: u16 = b' ' as u16;
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    const SEPARATORS: [u16; 4] = [SPACE, b'\t' as u16, b'\n' as u16, QUOTE];
    let mut line = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index > 0 {
            line.push(SPACE);
        }
        if !argument.is_empty() && !argument.iter().any(|unit| SEPARATORS.contains(unit)) {
            line.extend_from_slice(argument);
            continue;
        }
        line.push(QUOTE);
        let mut backslashes = 0;
        for &unit in argument {
            if unit == BACKSLASH {
                backslashes += 1;
            } else {
                if unit == QUOTE {
                    line.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
                }
                backslashes = 0;
            }
            line.push(unit);
        }
        line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        line.push(QUOTE);
    }
    line
}

/// The channels and the rate of the sound capture: the capture asks the system for 32-bit
/// floats, two channels at 48 kHz, and the system converts the sound to that form.
pub(crate) const SOUND_RATE: u32 = 48_000;
pub(crate) const SOUND_CHANNELS: usize = 2;
const SOUND_FRAME_BYTES: usize = SOUND_CHANNELS * 4;

/// Gives the stereo frames of one packet of the sound capture to `push`. A packet that the
/// system marks as silent gives silence, whatever its bytes hold. A packet whose bytes do not
/// hold `frames` frames gives none: its form is not the one that was asked for.
pub(crate) fn packet_frames(
    bytes: Option<&[u8]>,
    frames: usize,
    silent: bool,
    mut push: impl FnMut([f32; 2]),
) -> bool {
    if silent {
        for _ in 0..frames {
            push([0.0, 0.0]);
        }
        return true;
    }
    let Some(bytes) = bytes else {
        return frames == 0;
    };
    let Some(needed) = frames.checked_mul(SOUND_FRAME_BYTES) else {
        return false;
    };
    if bytes.len() < needed {
        return false;
    }
    for frame in bytes[..needed].as_chunks::<SOUND_FRAME_BYTES>().0 {
        let [l0, l1, l2, l3, r0, r1, r2, r3] = *frame;
        push([
            f32::from_le_bytes([l0, l1, l2, l3]),
            f32::from_le_bytes([r0, r1, r2, r3]),
        ]);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        super::own(
            100,
            r"C:\Users\Someone\AppData\Local\Programs\Junto\junto.exe",
            Some("Junto"),
        )
    }

    fn window(id: u32, process_id: u32, path: &str) -> ListedWindow {
        ListedWindow {
            id,
            title: format!("Window {id}"),
            class: "ApplicationWindow".to_owned(),
            visible: true,
            cloaked: false,
            minimized: false,
            tool: false,
            app_window: false,
            no_activate: false,
            owned: false,
            size: (1600, 1200),
            application: Some(application(process_id, path, None)),
        }
    }

    /// A window of the program of the client, whose file has the description "Junto".
    fn client(id: u32, process_id: u32, path: &str) -> ListedWindow {
        ListedWindow {
            application: Some(application(process_id, path, Some("Junto"))),
            ..window(id, process_id, path)
        }
    }

    #[test]
    fn only_application_windows_of_other_programs_are_listed() {
        let player = r"C:\Program Files\Player\Player.exe";
        let mut hidden = window(2, 7, player);
        hidden.visible = false;
        let mut cloaked = window(3, 7, player);
        cloaked.cloaked = true;
        let mut minimized = window(4, 7, player);
        minimized.minimized = true;
        let mut tool = window(5, 7, player);
        tool.tool = true;
        let mut dialog = window(6, 7, player);
        dialog.owned = true;
        let mut asks_for_the_task_bar = window(7, 7, player);
        asks_for_the_task_bar.owned = true;
        asks_for_the_task_bar.app_window = true;
        let mut passive = window(8, 7, player);
        passive.no_activate = true;
        let mut untitled = window(9, 7, player);
        untitled.title = "  ".to_owned();
        let mut desktop = window(10, 4, r"C:\Windows\explorer.exe");
        desktop.class = "Progman".to_owned();
        let mut narrow = window(11, 7, player);
        narrow.size = (800, 40);
        let mut unread = window(12, 7, player);
        unread.application = None;
        let listed = vec![
            window(1, 7, player),
            hidden,
            cloaked,
            minimized,
            tool,
            dialog,
            asks_for_the_task_bar,
            passive,
            untitled,
            desktop,
            narrow,
            unread,
            // The client itself, and another start of the same program in another case.
            client(
                13,
                100,
                r"C:\Users\Someone\AppData\Local\Programs\Junto\junto.exe",
            ),
            client(
                14,
                200,
                r"c:\users\someone\appdata\local\programs\junto\JUNTO.EXE",
            ),
        ];
        let windows = shareable_windows(listed, &own());
        let ids: Vec<u32> = windows.iter().map(|window| window.id).collect();
        assert_eq!(ids, [1, 7]);
        assert_eq!(
            windows[0],
            WindowSource {
                id: 1,
                title: "Window 1".to_owned(),
                application: ApplicationId {
                    process_id: 7,
                    bundle_id: r"c:\program files\player\player.exe".to_owned(),
                },
                application_name: "Player".to_owned(),
                width: 1600,
                height: 1200,
            }
        );
    }

    #[test]
    fn a_program_of_another_folder_with_the_name_of_the_client_is_another_application() {
        let other = window(1, 300, r"D:\Tools\junto.exe");
        let windows = shareable_windows(vec![other], &own());
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn a_screen_is_named_by_the_number_of_its_device() {
        assert_eq!(display_number(r"\\.\DISPLAY1"), Some(1));
        assert_eq!(display_number(r"\\.\DISPLAY12"), Some(12));
        for other in [
            "",
            r"\\.\DISPLAY",
            r"\\.\DISPLAY0",
            r"\\.\DISPLAY1\Monitor0",
            r"\\.\DISPLAY-1",
            "DISPLAY1",
        ] {
            assert_eq!(display_number(other), None, "{other}");
        }
    }

    #[test]
    fn the_number_of_a_window_gives_its_handle_back() {
        for handle in [0x0001_0a2c_isize, 0x7fff_fffe, -0x0000_1234, 0x0003_04c6] {
            assert_eq!(window_handle(window_number(handle)), handle, "{handle:#x}");
        }
    }

    #[test]
    fn an_application_is_named_by_its_description_or_its_file() {
        let path = r"C:\Program Files\Mozilla Firefox\firefox.exe";
        assert_eq!(application_name(Some("Firefox"), path), "Firefox");
        assert_eq!(application_name(Some("  "), path), "firefox");
        assert_eq!(application_name(None, path), "firefox");
        assert_eq!(application_name(None, r"C:\Tools\run"), "run");
        assert_eq!(application_name(None, r"C:\Tools\.hidden"), ".hidden");
        assert_eq!(
            program_identity(r"C:\Program Files\Mozilla Firefox\Firefox.EXE"),
            (
                r"c:\program files\mozilla firefox\firefox.exe".to_owned(),
                "firefox.exe".to_owned()
            )
        );
    }

    fn process(id: u32, parent: u32) -> ListedProcess {
        ListedProcess { id, parent }
    }

    #[test]
    fn the_processes_that_started_the_client_are_its_creators_while_each_is_older() {
        // The shell (4) started a terminal (20), which started the client (100). The shell
        // was created by a process that ended (2).
        let processes = [
            process(4, 2),
            process(20, 4),
            process(100, 20),
            process(30, 4),
            process(7, 100),
        ];
        let times = |id: u32| match id {
            4 => Some(10),
            20 => Some(20),
            100 => Some(30),
            _ => Some(40),
        };
        let known = |found: &[u32]| Creators::Known(found.to_vec());
        let chain = |list: &[ListedProcess], created: &dyn Fn(u32) -> Option<u64>| {
            ancestors(100, Some(list), created)
        };
        assert_eq!(chain(&processes, &times), known(&[20, 4]));
        // The terminal ended and a newer process took its identifier: it did not start the
        // client, and the chain ends there.
        let younger = |id: u32| if id == 20 { Some(50) } else { times(id) };
        assert_eq!(chain(&processes, &younger), known(&[]));
        // A creation time that cannot be read counts as a creator.
        let unread = |id: u32| if id == 20 { None } else { times(id) };
        assert_eq!(chain(&processes, &unread), known(&[20, 4]));
        // Above a creator whose time cannot be read, a process that is younger than the
        // client took the identifier of an ended creator: the chain ends there.
        let unread_then_younger = |id: u32| match id {
            20 => None,
            4 => Some(35),
            _ => times(id),
        };
        assert_eq!(chain(&processes, &unread_then_younger), known(&[20]));
        // Above two such creators, a process that is younger than the nearest known time
        // below it, and older than the client.
        let deeper = [
            process(100, 20),
            process(20, 30),
            process(30, 40),
            process(40, 50),
            process(50, 1),
        ];
        let deeper_times = |id: u32| match id {
            100 => Some(100),
            20 => Some(60),
            30 | 40 => None,
            50 => Some(80),
            _ => None,
        };
        assert_eq!(chain(&deeper, &deeper_times), known(&[20, 30, 40]));
        // The creator ended: nothing older is known.
        assert_eq!(
            chain(&[process(100, 20), process(4, 2)], &times),
            known(&[])
        );
        // A loop of identifiers in a damaged list ends.
        let looped = [process(100, 20), process(20, 4), process(4, 20)];
        assert_eq!(chain(&looped, &|_| None), known(&[20, 4]));
        // A list without the client, or no list, does not show the creators.
        assert_eq!(chain(&[process(20, 4)], &times), Creators::Unknown);
        assert_eq!(chain(&[], &times), Creators::Unknown);
        assert_eq!(ancestors(100, None, times), Creators::Unknown);
    }

    fn listed(process_id: i32, path: &str, name: &str) -> WindowSource {
        WindowSource {
            id: process_id as u32,
            title: format!("Window of {name}"),
            application: ApplicationId {
                process_id,
                bundle_id: path.to_owned(),
            },
            application_name: name.to_owned(),
            width: 800,
            height: 600,
        }
    }

    #[test]
    fn an_application_that_started_the_client_is_not_offered_for_its_sound_alone() {
        let explorer = listed(4, r"c:\windows\explorer.exe", "Windows Explorer");
        let player = listed(7, r"c:\program files\player\player.exe", "Player");
        let terminal = listed(20, r"c:\program files\terminal\terminal.exe", "Terminal");
        let windows = [
            explorer.clone(),
            player.clone(),
            terminal.clone(),
            explorer.clone(),
        ];
        let (applications, started) =
            sound_choices(&windows, &own(), &Creators::Known(vec![20, 4]));
        assert_eq!(
            applications,
            [ApplicationSource {
                id: player.application,
                name: "Player".to_owned(),
            }]
        );
        assert_eq!(started, [explorer.application, terminal.application]);
        // Without creators every application is offered.
        let (applications, started) = sound_choices(&windows, &own(), &Creators::Known(vec![]));
        assert_eq!(applications.len(), 3);
        assert!(started.is_empty());
        // When the creators are unknown, none is offered.
        let (applications, started) = sound_choices(&windows, &own(), &Creators::Unknown);
        assert!(applications.is_empty());
        assert!(started.is_empty());
    }

    fn line(arguments: &[&str]) -> String {
        let arguments: Vec<Vec<u16>> = arguments
            .iter()
            .map(|argument| argument.encode_utf16().collect())
            .collect();
        String::from_utf16(&command_line(&arguments)).unwrap_or_default()
    }

    #[test]
    fn each_argument_of_a_command_line_is_given_back_as_it_is() {
        assert_eq!(line(&[]), "");
        assert_eq!(
            line(&["--report", r"C:\Temp\report"]),
            r"--report C:\Temp\report"
        );
        assert_eq!(line(&[r"C:\a b\c"]), r#""C:\a b\c""#);
        assert_eq!(line(&["", "x"]), r#""" x"#);
        assert_eq!(line(&[r#"a"b"#]), r#""a\"b""#);
        assert_eq!(line(&[r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(line(&[r"a b\"]), r#""a b\\""#);
        assert_eq!(line(&[r"a\b\"]), r"a\b\");
        assert_eq!(line(&["a\tb"]), "\"a\tb\"");
        assert_eq!(line(&[r#"x" --attach "C:\a"#]), r#""x\" --attach \"C:\a""#);
    }

    fn bytes(frames: &[[f32; 2]]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|[left, right]| [left.to_le_bytes(), right.to_le_bytes()])
            .flatten()
            .collect()
    }

    #[test]
    fn a_sound_packet_becomes_stereo_frames_and_a_silent_one_silence() {
        let packet = bytes(&[[0.25, -0.5], [1.0, 0.0], [-1.0, 0.125]]);
        let mut out = Vec::new();
        assert!(packet_frames(Some(&packet), 3, false, |frame| out.push(frame)));
        assert_eq!(out, [[0.25, -0.5], [1.0, 0.0], [-1.0, 0.125]]);
        // Fewer frames than the bytes hold: only those.
        out.clear();
        assert!(packet_frames(Some(&packet), 2, false, |frame| out.push(frame)));
        assert_eq!(out.len(), 2);
        // The system states silence: its bytes are not read.
        out.clear();
        assert!(packet_frames(Some(&packet), 4, true, |frame| out.push(frame)));
        assert_eq!(out, [[0.0, 0.0]; 4]);
        out.clear();
        assert!(packet_frames(None, 2, true, |frame| out.push(frame)));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_sound_packet_of_another_form_gives_nothing() {
        let packet = bytes(&[[0.25, -0.5]]);
        let mut out = Vec::new();
        assert!(!packet_frames(Some(&packet), 2, false, |frame| out.push(frame)));
        assert!(!packet_frames(None, 1, false, |frame| out.push(frame)));
        assert!(!packet_frames(Some(&packet), usize::MAX, false, |frame| {
            out.push(frame)
        }));
        assert!(out.is_empty());
        assert!(packet_frames(None, 0, false, |frame| out.push(frame)));
    }
}
