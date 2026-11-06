//! The installation's clock choice and the system's preferred hour cycle.
use serde::{Deserialize, Serialize};
use time::Time;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum HourCycle {
    Twelve,
    #[default]
    TwentyFour,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockFormat {
    TwelveHour,
    TwentyFourHour,
    #[default]
    #[serde(other)]
    System,
}

impl ClockFormat {
    pub const ALL: [Self; 3] = [Self::System, Self::TwelveHour, Self::TwentyFourHour];
    pub fn resolve(self, system: HourCycle) -> HourCycle {
        match self {
            Self::System => system,
            Self::TwelveHour => HourCycle::Twelve,
            Self::TwentyFourHour => HourCycle::TwentyFour,
        }
    }
}

impl HourCycle {
    pub fn time(self, at: Time) -> String {
        self.shown(at, false)
    }
    pub fn time_with_seconds(self, at: Time) -> String {
        self.shown(at, true)
    }
    pub fn minute(self, minute: u16) -> String {
        if minute >= 1440 {
            return "--:--".into();
        }
        Time::from_hms((minute / 60) as u8, (minute % 60) as u8, 0)
            .map(|time| self.time(time))
            .unwrap_or_else(|_| "--:--".into())
    }
    fn shown(self, at: Time, seconds: bool) -> String {
        let hour = match self {
            Self::Twelve => (at.hour() + 11) % 12 + 1,
            Self::TwentyFour => at.hour(),
        };
        let hour = if self == Self::Twelve {
            hour.to_string()
        } else {
            format!("{hour:02}")
        };
        let second = if seconds {
            format!(":{:02}", at.second())
        } else {
            String::new()
        };
        let suffix = match self {
            Self::Twelve if at.hour() < 12 => " AM",
            Self::Twelve => " PM",
            Self::TwentyFour => "",
        };
        format!("{hour}:{:02}{second}{suffix}", at.minute())
    }
}

/// Reads native preferences. Call at startup or on a background worker.
pub fn system_hour_cycle() -> HourCycle {
    platform::read().unwrap_or_default()
}

#[cfg(any(target_os = "macos", windows, test))]
fn native_pattern(pattern: &str) -> Option<HourCycle> {
    let mut quoted = false;
    let mut chars = pattern.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if !quoted {
            match character {
                'h' | 'K' => return Some(HourCycle::Twelve),
                'H' | 'k' => return Some(HourCycle::TwentyFour),
                _ => {}
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2_foundation::{NSDateFormatter, NSLocale, NSString};
    pub(super) fn read() -> Option<HourCycle> {
        let locale = NSLocale::currentLocale();
        let pattern = NSDateFormatter::dateFormatFromTemplate_options_locale(
            &NSString::from_str("j"),
            0,
            Some(&locale),
        )?;
        native_pattern(&pattern.to_string())
    }
}

#[cfg(windows)]
mod platform {
    #![allow(unsafe_code)]
    use super::*;
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTTIME};
    pub(super) fn read() -> Option<HourCycle> {
        let mut buffer = [0u16; 256];
        // SAFETY: null selects the current user's locale; the writable buffer has the stated length.
        let count = unsafe {
            GetLocaleInfoEx(
                std::ptr::null(),
                LOCALE_SSHORTTIME,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        };
        let count = usize::try_from(count).ok()?.checked_sub(1)?;
        native_pattern(&String::from_utf16(buffer.get(..count)?).ok()?)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::*;
    use std::{
        collections::HashMap,
        io::Read as _,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    /// Runs a system program with its arguments and, when given, the `LC_TIME` of the reading
    /// in place of the one of the environment; returns its output.
    pub(super) type Run<'a> = &'a dyn Fn(&str, &[&str], Option<&str>) -> Option<String>;

    /// What the reading of the hour cycle needs from the session: the desktops of
    /// `XDG_CURRENT_DESKTOP`, the configuration directory of the person, and a way to run
    /// the system programs that read settings.
    pub(super) struct Session<'a> {
        pub desktops: Vec<String>,
        pub config: Option<PathBuf>,
        pub run: Run<'a>,
    }

    pub(super) fn read() -> Option<HourCycle> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        read_session(&Session {
            desktops: std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .map(str::to_ascii_lowercase)
                .collect(),
            config,
            run: &output,
        })
    }

    /// The setting of the desktop, or else the time format of the locale: `LC_TIME` of the
    /// session, or the region of Plasma's settings when its clock follows the region.
    pub(super) fn read_session(session: &Session<'_>) -> Option<HourCycle> {
        let on = |name: &str| session.desktops.iter().any(|desktop| desktop == name);
        let run = session.run;
        let config = session.config.as_deref();
        let mut region = None;
        let setting = if on("kde") {
            let plasma = config.and_then(plasma_clock);
            if plasma.is_none() {
                region = config.and_then(plasma_region);
            }
            plasma
        } else if on("xfce") {
            config.and_then(xfce_clock)
        } else if on("x-cinnamon") || on("cinnamon") {
            gsettings(run, &["org.cinnamon.desktop.interface", "clock-use-24h"]).and_then(|value| {
                match value.as_str() {
                    "true" => Some(HourCycle::TwentyFour),
                    "false" => Some(HourCycle::Twelve),
                    _ => None,
                }
            })
        } else if on("mate") {
            mate_clock(run)
        } else if on("gnome") || on("budgie") {
            gsettings(run, &["org.gnome.desktop.interface", "clock-format"]).and_then(|value| {
                match value.as_str() {
                    "12h" => Some(HourCycle::Twelve),
                    "24h" => Some(HourCycle::TwentyFour),
                    _ => None,
                }
            })
        } else {
            None
        };
        setting
            .or_else(|| strftime_pattern(&run("/usr/bin/locale", &["t_fmt"], region.as_deref())?))
    }

    /// A value of `gsettings get`, without the quotes of a string.
    fn gsettings(run: Run<'_>, arguments: &[&str]) -> Option<String> {
        let value = run("/usr/bin/gsettings", &[&["get"], arguments].concat(), None)?;
        Some(value.trim().trim_matches('\'').to_owned())
    }

    /// The sections of a file of KDE settings, each with its keys and values, in their order.
    fn kde_sections(text: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
        let mut sections: Vec<(&str, Vec<(&str, &str)>)> = vec![("", Vec::new())];
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') && line.ends_with(']') {
                sections.push((line, Vec::new()));
            } else if let Some((key, value)) = line.split_once('=')
                && let Some((_, keys)) = sections.last_mut()
            {
                keys.push((key.trim(), value.trim()));
            }
        }
        sections
    }

    /// A file of the settings of the person, at most 1 MiB.
    fn settings_file(path: &Path) -> Option<String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(1 << 20)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    /// The hour format that a digital clock of the Plasma panel states: `use24hFormat` 0 is
    /// 12 hours, 2 is 24 hours, and 1, the default, follows the region.
    fn plasma_clock(config: &Path) -> Option<HourCycle> {
        let text = settings_file(&config.join("plasma-org.kde.plasma.desktop-appletsrc"))?;
        let sections = kde_sections(&text);
        let clocks: Vec<&str> = sections
            .iter()
            .filter(|(_, keys)| keys.contains(&("plugin", "org.kde.plasma.digitalclock")))
            .map(|(name, _)| *name)
            .collect();
        sections.iter().find_map(|(name, keys)| {
            let applet = name.strip_suffix("[Configuration][Appearance]")?;
            if !clocks.contains(&applet) {
                return None;
            }
            keys.iter()
                .find(|(key, _)| *key == "use24hFormat")
                .and_then(|(_, value)| match *value {
                    "0" => Some(HourCycle::Twelve),
                    "2" => Some(HourCycle::TwentyFour),
                    _ => None,
                })
        })
    }

    /// The locale of the time formats of Plasma's region settings, which Plasma gives the
    /// programs of its session as `LC_TIME`.
    fn plasma_region(config: &Path) -> Option<String> {
        let text = settings_file(&config.join("plasma-localerc"))?;
        let sections = kde_sections(&text);
        let formats = &sections.iter().find(|(name, _)| *name == "[Formats]")?.1;
        let value = |wanted: &str| {
            formats
                .iter()
                .find(|(key, _)| *key == wanted)
                .map(|(_, value)| *value)
        };
        value("LC_TIME")
            .or_else(|| value("LANG"))
            .filter(|locale| {
                !locale.is_empty()
                    && locale
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'))
            })
            .map(str::to_owned)
    }

    /// The attributes of each `property` element of an xfconf file, with its depth.
    fn xfconf_properties(text: &str) -> Vec<(usize, HashMap<&str, String>)> {
        let mut properties = Vec::new();
        let mut depth = 0usize;
        let mut rest = text;
        while let Some(at) = rest.find('<') {
            rest = &rest[at + 1..];
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[..end];
            rest = &rest[end + 1..];
            if tag.starts_with("/property") {
                depth = depth.saturating_sub(1);
                continue;
            }
            let Some(attributes) = tag.strip_prefix("property ") else {
                continue;
            };
            let closed = attributes.ends_with('/');
            let mut values = HashMap::new();
            let mut remaining = attributes.trim_end_matches('/');
            while let Some((name, after)) = remaining.split_once("=\"") {
                let Some((value, next)) = after.split_once('"') else {
                    break;
                };
                values.insert(name.trim(), xml_text(value));
                remaining = next;
            }
            properties.push((depth, values));
            if !closed {
                depth += 1;
            }
        }
        properties
    }

    fn xml_text(value: &str) -> String {
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    /// The hour format of the first clock of the Xfce panel, by its `mode` (2, digital, when
    /// the panel stores none): a digital clock shows `digital-time-format` (Xfce 4.18) or
    /// `digital-format` (earlier), a pattern of strftime, or the default `%R` of the panel;
    /// an LCD clock (4) shows 24 hours unless `show-military` is false; an analog, binary or
    /// fuzzy clock shows no time of day, so the locale decides.
    fn xfce_clock(config: &Path) -> Option<HourCycle> {
        let text = settings_file(&config.join("xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml"))?;
        let properties = xfconf_properties(&text);
        let mut clock: Option<(usize, HashMap<String, String>)> = None;
        let mut found = None;
        for (depth, values) in &properties {
            let name = values.get("name").map(String::as_str);
            if let Some((at, settings)) = &mut clock {
                if *depth > *at {
                    if let (Some(name), Some(value)) = (name, values.get("value")) {
                        settings.insert(name.to_owned(), value.clone());
                    }
                    continue;
                }
                found = clock.take().map(|(_, settings)| settings);
                break;
            }
            if name.is_some_and(|name| name.starts_with("plugin-"))
                && values.get("value").map(String::as_str) == Some("clock")
            {
                clock = Some((*depth, HashMap::new()));
            }
        }
        let settings = found.or(clock.map(|(_, settings)| settings))?;
        let setting = |name: &str| settings.get(name).map(String::as_str);
        match setting("mode").unwrap_or("2") {
            "2" if setting("digital-layout") == Some("2") => None,
            "2" => strftime_pattern(
                setting("digital-time-format")
                    .or(setting("digital-format"))
                    .unwrap_or("%R"),
            ),
            "4" => match setting("show-military") {
                Some("false") => Some(HourCycle::Twelve),
                _ => Some(HourCycle::TwentyFour),
            },
            _ => None,
        }
    }

    /// The format of the first clock applet of the MATE panel: `12-hour` or `24-hour`.
    fn mate_clock(run: Run<'_>) -> Option<HourCycle> {
        let objects = gsettings(run, &["org.mate.panel", "object-id-list"])?;
        let identifiers = objects
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|name| name.trim().trim_matches('\''))
            .filter(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            })
            .take(64);
        for identifier in identifiers {
            let object = format!("org.mate.panel.object:/org/mate/panel/objects/{identifier}/");
            // A read that fails ends the search: each read can take its whole deadline.
            if gsettings(run, &[&object, "applet-iid"])? != "ClockAppletFactory::ClockApplet" {
                continue;
            }
            let prefs =
                format!("org.mate.panel.applet.clock:/org/mate/panel/objects/{identifier}/prefs/");
            return match gsettings(run, &[&prefs, "format"]).as_deref() {
                Some("12-hour") => Some(HourCycle::Twelve),
                Some("24-hour") => Some(HourCycle::TwentyFour),
                _ => None,
            };
        }
        None
    }

    fn output(program: &str, arguments: &[&str], lc_time: Option<&str>) -> Option<String> {
        let mut command = Command::new(program);
        if let Some(locale) = lc_time {
            command.env_remove("LC_ALL").env("LC_TIME", locale);
        }
        let mut child = command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return None,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let mut text = Vec::new();
        child
            .stdout
            .take()?
            .take(4096)
            .read_to_end(&mut text)
            .ok()?;
        Some(String::from_utf8_lossy(&text).into_owned())
    }
    pub(super) fn strftime_pattern(pattern: &str) -> Option<HourCycle> {
        let mut chars = pattern.chars();
        while let Some(character) = chars.next() {
            if character != '%' {
                continue;
            }
            let mut conversion = chars.next()?;
            while matches!(conversion, 'E' | 'O' | '-' | '_' | '0'..='9' | '^' | '#') {
                conversion = chars.next()?;
            }
            match conversion {
                'I' | 'l' | 'r' => return Some(HourCycle::Twelve),
                'H' | 'k' | 'R' | 'T' => return Some(HourCycle::TwentyFour),
                _ => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn midnight_noon_and_seconds_follow_the_selected_cycle() {
        for (hour, twelve, twenty_four) in [
            (0, "12:05 AM", "00:05"),
            (1, "1:05 AM", "01:05"),
            (12, "12:05 PM", "12:05"),
            (23, "11:05 PM", "23:05"),
        ] {
            let at = Time::from_hms(hour, 5, 9).unwrap();
            assert_eq!(HourCycle::Twelve.time(at), twelve);
            assert_eq!(HourCycle::TwentyFour.time(at), twenty_four);
        }
        assert_eq!(
            HourCycle::Twelve.time_with_seconds(Time::from_hms(0, 0, 3).unwrap()),
            "12:00:03 AM"
        );
        assert_eq!(
            HourCycle::TwentyFour.time_with_seconds(Time::from_hms(23, 1, 3).unwrap()),
            "23:01:03"
        );
        assert_eq!(HourCycle::Twelve.minute(1320), "10:00 PM");
    }
    #[test]
    fn native_patterns_ignore_quoted_literals_and_system_is_resolved_per_caller() {
        assert_eq!(native_pattern("HH:mm 'hours'"), Some(HourCycle::TwentyFour));
        assert_eq!(
            native_pattern("'Hour' h:mm 'o''clock' a"),
            Some(HourCycle::Twelve)
        );
        assert_eq!(
            ClockFormat::System.resolve(HourCycle::Twelve),
            HourCycle::Twelve
        );
        assert_eq!(
            ClockFormat::TwentyFourHour.resolve(HourCycle::Twelve),
            HourCycle::TwentyFour
        );
        assert_eq!(
            ClockFormat::TwelveHour.resolve(HourCycle::TwentyFour),
            HourCycle::Twelve
        );
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn locale_patterns_distinguish_expansions_and_escaped_percent() {
        for pattern in ["%I:%M %p", "%r", "%OI:%M"] {
            assert_eq!(platform::strftime_pattern(pattern), Some(HourCycle::Twelve));
        }
        for pattern in ["%H:%M:%S", "%T", "%%I %H:%M", "%R"] {
            assert_eq!(
                platform::strftime_pattern(pattern),
                Some(HourCycle::TwentyFour)
            );
        }
    }
}

#[cfg(test)]
#[cfg(not(any(target_os = "macos", windows)))]
mod desktop_tests {
    use super::HourCycle;
    use super::platform::{Session, read_session};
    use std::collections::HashMap;
    use std::path::Path;

    /// The answers of the system programs, by program, arguments and `LC_TIME`.
    struct Programs(HashMap<String, String>);

    impl Programs {
        fn new(answers: &[(&str, &str)]) -> Self {
            Self(
                answers
                    .iter()
                    .map(|(call, answer)| ((*call).to_owned(), (*answer).to_owned()))
                    .collect(),
            )
        }
        fn run(&self, program: &str, arguments: &[&str], lc_time: Option<&str>) -> Option<String> {
            let mut call = format!("{program} {}", arguments.join(" "));
            if let Some(locale) = lc_time {
                call = format!("LC_TIME={locale} {call}");
            }
            self.0.get(&call).cloned()
        }
    }

    /// The locale of the session states 12 hours.
    const LOCALE_TWELVE: (&str, &str) = ("/usr/bin/locale t_fmt", "%r\n");
    /// The locale of the session states 24 hours.
    const LOCALE_TWENTY_FOUR: (&str, &str) = ("/usr/bin/locale t_fmt", "%H:%M:%S\n");

    fn read(desktop: &str, config: Option<&Path>, programs: &Programs) -> Option<HourCycle> {
        let run = |program: &str, arguments: &[&str], lc_time: Option<&str>| {
            programs.run(program, arguments, lc_time)
        };
        read_session(&Session {
            desktops: desktop.split(':').map(str::to_ascii_lowercase).collect(),
            config: config.map(Path::to_path_buf),
            run: &run,
        })
    }

    fn config(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        directory
    }

    fn plasma_clock(setting: Option<u8>) -> String {
        let mut file = "[Containments][2][Applets][7]\nimmutability=1\nplugin=org.kde.plasma.kickoff\n\n\
             [Containments][2][Applets][7][Configuration][General]\nfavoritesPortedToKAstats=true\n\n\
             [Containments][2][Applets][9]\nimmutability=1\nplugin=org.kde.plasma.digitalclock\n\n\
             [Containments][2][Applets][9][Configuration][Appearance]\nfontWeight=400\n"
            .to_owned();
        if let Some(setting) = setting {
            file.push_str(&format!("use24hFormat={setting}\n"));
        }
        file
    }

    const PLASMA_APPLETS: &str = "plasma-org.kde.plasma.desktop-appletsrc";

    /// The digital clock of Plasma states 12 or 24 hours over the locale.
    #[test]
    fn plasma_reads_the_hour_format_of_its_digital_clock() {
        let twelve = config(&[(PLASMA_APPLETS, &plasma_clock(Some(0)))]);
        let programs = Programs::new(&[LOCALE_TWENTY_FOUR]);
        assert_eq!(
            read("KDE", Some(twelve.path()), &programs),
            Some(HourCycle::Twelve)
        );
        let twenty_four = config(&[(PLASMA_APPLETS, &plasma_clock(Some(2)))]);
        let programs = Programs::new(&[LOCALE_TWELVE]);
        assert_eq!(
            read("KDE", Some(twenty_four.path()), &programs),
            Some(HourCycle::TwentyFour)
        );
    }

    /// A clock that follows the region of Plasma reads the time format of the region of its
    /// settings, which can differ from the locale of the process.
    #[test]
    fn plasma_follows_the_time_format_of_its_region() {
        let region = "[Formats]\nLANG=en_US.UTF-8\nLC_TIME=de_DE.UTF-8\n";
        let programs = Programs::new(&[
            LOCALE_TWELVE,
            ("LC_TIME=de_DE.UTF-8 /usr/bin/locale t_fmt", "%T\n"),
        ]);
        for setting in [None, Some(1)] {
            let directory = config(&[
                (PLASMA_APPLETS, &plasma_clock(setting)),
                ("plasma-localerc", region),
            ]);
            assert_eq!(
                read("KDE", Some(directory.path()), &programs),
                Some(HourCycle::TwentyFour),
                "{setting:?}"
            );
        }
        // Without settings of Plasma the locale of the session decides.
        let empty = config(&[]);
        assert_eq!(
            read("KDE", Some(empty.path()), &programs),
            Some(HourCycle::Twelve)
        );
    }

    const XFCE_PANEL: &str = "xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml";

    fn xfce_panel(clock: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\n<channel name=\"xfce4-panel\" version=\"1.0\">\n  \
             <property name=\"configver\" type=\"int\" value=\"2\"/>\n  \
             <property name=\"plugins\" type=\"empty\">\n    \
             <property name=\"plugin-1\" type=\"string\" value=\"applicationsmenu\"/>\n    \
             <property name=\"plugin-2\" type=\"string\" value=\"tasklist\">\n      \
             <property name=\"grouping\" type=\"uint\" value=\"1\"/>\n    </property>\n    \
             <property name=\"plugin-8\" type=\"string\" value=\"clock\">\n{clock}    </property>\n  \
             </property>\n</channel>\n"
        )
    }

    /// The clock of the Xfce panel states its format as a pattern of strftime, in the
    /// property of Xfce 4.18 or in the one of earlier versions, or shows the default `%R` of
    /// the panel; an LCD clock reads `show-military`; an analog clock leaves the locale.
    #[test]
    fn xfce_reads_the_format_of_its_panel_clock() {
        let programs = Programs::new(&[LOCALE_TWENTY_FOUR]);
        let twelve = config(&[(
            XFCE_PANEL,
            &xfce_panel(
                "      <property name=\"digital-layout\" type=\"uint\" value=\"3\"/>\n      \
                 <property name=\"digital-time-format\" type=\"string\" value=\"%l:%M %p\"/>\n",
            ),
        )]);
        assert_eq!(
            read("XFCE", Some(twelve.path()), &programs),
            Some(HourCycle::Twelve)
        );
        let programs = Programs::new(&[LOCALE_TWELVE]);
        let older = config(&[(
            XFCE_PANEL,
            &xfce_panel(
                "      <property name=\"digital-format\" type=\"string\" value=\"&lt;b&gt;%R&lt;/b&gt;\"/>\n",
            ),
        )]);
        assert_eq!(
            read("XFCE", Some(older.path()), &programs),
            Some(HourCycle::TwentyFour)
        );
        // A clock whose format was never changed shows the default of the panel, `%R`.
        let unchanged = config(&[(XFCE_PANEL, &xfce_panel(""))]);
        assert_eq!(
            read("XFCE", Some(unchanged.path()), &programs),
            Some(HourCycle::TwentyFour)
        );
        // An LCD clock shows 24 hours unless its `show-military` is false.
        let lcd = |military: &str| {
            config(&[(
                XFCE_PANEL,
                &xfce_panel(&format!(
                    "      <property name=\"mode\" type=\"uint\" value=\"4\"/>\n{military}"
                )),
            )])
        };
        let default = lcd("");
        assert_eq!(
            read("XFCE", Some(default.path()), &programs),
            Some(HourCycle::TwentyFour)
        );
        let twelve =
            lcd("      <property name=\"show-military\" type=\"bool\" value=\"false\"/>\n");
        let programs_24 = Programs::new(&[LOCALE_TWENTY_FOUR]);
        assert_eq!(
            read("XFCE", Some(twelve.path()), &programs_24),
            Some(HourCycle::Twelve)
        );
        // An analog clock shows no time of day: the locale decides.
        let analog = config(&[(
            XFCE_PANEL,
            &xfce_panel("      <property name=\"mode\" type=\"uint\" value=\"0\"/>\n"),
        )]);
        assert_eq!(
            read("XFCE", Some(analog.path()), &programs),
            Some(HourCycle::Twelve)
        );
        // A digital clock with the date only (layout 2) shows no time of day either.
        let date = config(&[(
            XFCE_PANEL,
            &xfce_panel(
                "      <property name=\"digital-layout\" type=\"uint\" value=\"2\"/>\n      \
                 <property name=\"digital-time-format\" type=\"string\" value=\"%R\"/>\n",
            ),
        )]);
        assert_eq!(
            read("XFCE", Some(date.path()), &programs),
            Some(HourCycle::Twelve)
        );
    }

    #[test]
    fn cinnamon_reads_its_24_hour_switch() {
        let call = "/usr/bin/gsettings get org.cinnamon.desktop.interface clock-use-24h";
        let programs = Programs::new(&[LOCALE_TWENTY_FOUR, (call, "false\n")]);
        assert_eq!(read("X-Cinnamon", None, &programs), Some(HourCycle::Twelve));
        let programs = Programs::new(&[LOCALE_TWELVE, (call, "true\n")]);
        assert_eq!(
            read("X-Cinnamon", None, &programs),
            Some(HourCycle::TwentyFour)
        );
    }

    /// MATE keeps the format in the settings of each clock applet of its panel: the objects
    /// of the panel name the applet.
    #[test]
    fn mate_reads_the_format_of_its_clock_applet() {
        let answers = |format: &'static str| {
            [
                (
                    "/usr/bin/gsettings get org.mate.panel object-id-list",
                    "['menu-bar', 'notification-area', 'object-3']\n",
                ),
                (
                    "/usr/bin/gsettings get org.mate.panel.object:/org/mate/panel/objects/menu-bar/ applet-iid",
                    "''\n",
                ),
                (
                    "/usr/bin/gsettings get org.mate.panel.object:/org/mate/panel/objects/notification-area/ applet-iid",
                    "'NotificationAreaAppletFactory::NotificationArea'\n",
                ),
                (
                    "/usr/bin/gsettings get org.mate.panel.object:/org/mate/panel/objects/object-3/ applet-iid",
                    "'ClockAppletFactory::ClockApplet'\n",
                ),
                (
                    "/usr/bin/gsettings get org.mate.panel.applet.clock:/org/mate/panel/objects/object-3/prefs/ format",
                    format,
                ),
            ]
        };
        let mut twelve = answers("'12-hour'\n").to_vec();
        twelve.push(LOCALE_TWENTY_FOUR);
        assert_eq!(
            read("MATE", None, &Programs::new(&twelve)),
            Some(HourCycle::Twelve)
        );
        let mut twenty_four = answers("'24-hour'\n").to_vec();
        twenty_four.push(LOCALE_TWELVE);
        assert_eq!(
            read("MATE", None, &Programs::new(&twenty_four)),
            Some(HourCycle::TwentyFour)
        );
        // A format without hours of the day, such as the time of Unix, leaves the locale.
        let mut unix = answers("'unix'\n").to_vec();
        unix.push(LOCALE_TWELVE);
        assert_eq!(
            read("MATE", None, &Programs::new(&unix)),
            Some(HourCycle::Twelve)
        );
    }

    #[test]
    fn gnome_reads_its_clock_format_and_another_desktop_the_locale() {
        let call = "/usr/bin/gsettings get org.gnome.desktop.interface clock-format";
        let programs = Programs::new(&[LOCALE_TWENTY_FOUR, (call, "'12h'\n")]);
        assert_eq!(
            read("ubuntu:GNOME", None, &programs),
            Some(HourCycle::Twelve)
        );
        assert_eq!(
            read("Budgie:GNOME", None, &programs),
            Some(HourCycle::Twelve)
        );
        assert_eq!(read("LXQt", None, &programs), Some(HourCycle::TwentyFour));
        assert_eq!(read("", None, &Programs::new(&[])), None);
    }
}
