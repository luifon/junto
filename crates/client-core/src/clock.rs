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
        io::Read as _,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    pub(super) fn read() -> Option<HourCycle> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if (desktop.contains("gnome") || desktop.contains("budgie"))
            && let Some(setting) = output(
                "/usr/bin/gsettings",
                &["get", "org.gnome.desktop.interface", "clock-format"],
            )
        {
            match setting.trim().trim_matches('\'') {
                "12h" => return Some(HourCycle::Twelve),
                "24h" => return Some(HourCycle::TwentyFour),
                _ => {}
            }
        }
        strftime_pattern(&output("/usr/bin/locale", &["t_fmt"])?)
    }
    fn output(program: &str, arguments: &[&str]) -> Option<String> {
        let mut child = Command::new(program)
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
