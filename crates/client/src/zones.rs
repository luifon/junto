//! The time zones of a daily quiet time (ADR 0412, ADR 0798): the names that the person
//! chooses from, the zone of the system as the first choice, and the clock of a named
//! zone at an instant.
//!
//! A quiet time states the name of a zone of the IANA time zone database, and the server
//! holds it while the clock of that zone shows a time inside the window. The names and
//! the rules of the zones are carried in the binary, so the list is the same on every
//! system and does not depend on files of the system.

use chrono::{TimeZone as _, Timelike as _};
use chrono_tz::Tz;
use time::OffsetDateTime;

/// The zone that a quiet time names when the zone of the system is not known.
pub const FALLBACK: &str = "UTC";

/// The most names that one search of the list returns.
pub const MOST_MATCHES: usize = 50;

/// The zone with a name, when the database of this build holds it.
fn zone(name: &str) -> Option<Tz> {
    name.parse().ok()
}

/// True for a name of a zone that this build holds. The server decides with its own
/// database whether it accepts the name.
pub fn is_known(name: &str) -> bool {
    zone(name).is_some()
}

/// Every name of the database, in the order of the alphabet.
pub fn names() -> impl Iterator<Item = &'static str> {
    chrono_tz::TZ_VARIANTS.iter().map(|zone| zone.name())
}

/// The time zone of the system: the zone that the variable `TZ` states
/// ([`zone_of_variable`]), otherwise the zone that the system states, when this build holds
/// it, and [`FALLBACK`] otherwise. It reads files of the system, such as `/etc/localtime`:
/// the application calls it at the start and on a background thread, not on the thread of
/// the window (ADR 0730).
#[allow(clippy::disallowed_methods)]
pub fn system_zone() -> Tz {
    let variable = std::env::var("TZ").ok();
    variable
        .as_deref()
        .and_then(|value| zone_of_variable(value, |path| std::fs::read_link(path).ok()))
        .or_else(|| {
            iana_time_zone::get_timezone()
                .ok()
                .as_deref()
                .and_then(zone)
        })
        .unwrap_or(Tz::UTC)
}

/// The zone that a value of `TZ` states, in the forms that the C library reads, after an
/// optional `:`: a name of the database; a path of a file of the database
/// (`/usr/share/zoneinfo/Europe/Lisbon`), whose name is the part after `zoneinfo/`; or a
/// path of a link to such a file (`/etc/localtime`), which `read_link` follows once. A
/// rule in the form of POSIX (`<-03>3`) names no zone of the database and gives none, as
/// does a path of a file that is not in a directory `zoneinfo`.
pub fn zone_of_variable(
    value: &str,
    read_link: impl Fn(&std::path::Path) -> Option<std::path::PathBuf>,
) -> Option<Tz> {
    let value = value.strip_prefix(':').unwrap_or(value);
    if let Some(named) = zone(value) {
        return Some(named);
    }
    if !value.starts_with('/') {
        return None;
    }
    let in_database = |path: &str| {
        path.rsplit_once("zoneinfo/")
            .and_then(|(_, name)| zone(name))
    };
    in_database(value).or_else(|| {
        let target = read_link(std::path::Path::new(value))?;
        in_database(target.to_str()?)
    })
}

/// The name of [`system_zone`]. It reads files of the system, as that function does.
#[allow(clippy::disallowed_methods)]
pub fn system() -> String {
    system_zone().name().to_owned()
}

/// The name itself when this build holds its zone, and [`FALLBACK`] otherwise.
pub fn known_or_fallback(name: Option<&str>) -> String {
    name.and_then(zone)
        .map_or(FALLBACK, |zone| zone.name())
        .to_owned()
}

/// The minute of the day, from 0 to 1439, that the clock of a named zone shows at an
/// instant. Absent for a name that this build does not hold: such a zone gives no quiet
/// time, as on the server (ADR 0412).
pub fn minute_in(name: &str, at: OffsetDateTime) -> Option<u16> {
    let zone = zone(name)?;
    let local = zone
        .timestamp_opt(at.unix_timestamp(), at.nanosecond())
        .single()?;
    u16::try_from(local.hour() * 60 + local.minute()).ok()
}

/// A name without the differences that a person does not type: case, and the space that
/// stands for the underscore of a name.
fn folded(text: &str) -> String {
    text.trim()
        .chars()
        .map(|character| match character {
            ' ' | '-' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The names that hold the typed text, at most [`MOST_MATCHES`]: first the names whose
/// last part starts with the text, then the others, each group in the order of the
/// alphabet. An empty text matches every name. `more` is true when further names match.
pub fn matching(typed: &str) -> (Vec<&'static str>, bool) {
    let wanted = folded(typed);
    let mut first = Vec::new();
    let mut rest = Vec::new();
    for name in names() {
        let name_folded = folded(name);
        if !name_folded.contains(&wanted) {
            continue;
        }
        let place = name_folded.rsplit('/').next().unwrap_or(&name_folded);
        if !wanted.is_empty() && place.starts_with(&wanted) {
            first.push(name);
        } else {
            rest.push(name);
        }
    }
    first.append(&mut rest);
    let more = first.len() > MOST_MATCHES;
    first.truncate(MOST_MATCHES);
    (first, more)
}

/// A name as the list shows it: the underscores of the database are spaces.
pub fn shown(name: &str) -> String {
    name.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use time::{Date, Month, Time, UtcOffset};

    use super::*;

    /// An instant by its date and its time on the clock of UTC.
    fn utc(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> OffsetDateTime {
        let date = Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap();
        date.with_time(Time::from_hms(hour, minute, 0).unwrap())
            .assume_utc()
    }

    #[test]
    fn the_variable_tz_states_a_zone_by_its_name_by_the_path_of_its_file_or_by_a_link() {
        let no_link = |_: &std::path::Path| None;
        let lisbon = Some(chrono_tz::Europe::Lisbon);
        assert_eq!(zone_of_variable("Europe/Lisbon", no_link), lisbon);
        assert_eq!(zone_of_variable(":Europe/Lisbon", no_link), lisbon);
        assert_eq!(
            zone_of_variable("/usr/share/zoneinfo/Europe/Lisbon", no_link),
            lisbon
        );
        assert_eq!(
            zone_of_variable(
                ":/usr/share/zoneinfo/America/Argentina/Buenos_Aires",
                no_link
            ),
            Some(chrono_tz::America::Argentina::Buenos_Aires)
        );
        // `/etc/localtime` is a link to a file of the database, on macOS in another
        // directory than on Linux.
        let localtime = |path: &std::path::Path| {
            (path == std::path::Path::new("/etc/localtime"))
                .then(|| "/var/db/timezone/zoneinfo/Asia/Tokyo".into())
        };
        assert_eq!(
            zone_of_variable(":/etc/localtime", localtime),
            Some(chrono_tz::Asia::Tokyo)
        );
        assert_eq!(zone_of_variable("/etc/localtime", no_link), None);
        // A rule in the form of POSIX names no zone of the database.
        for value in ["<-03>3", "XYZ3", "", ":", "/usr/share/zoneinfo/Nowhere"] {
            assert_eq!(zone_of_variable(value, no_link), None, "{value}");
        }
    }

    #[test]
    fn the_list_holds_the_names_of_the_database_once_and_in_order() {
        let listed: Vec<&str> = names().collect();
        assert!(listed.len() > 300);
        for name in [
            "UTC",
            "Europe/Lisbon",
            "America/Sao_Paulo",
            "Asia/Kolkata",
            "America/Argentina/Buenos_Aires",
        ] {
            assert!(listed.contains(&name), "{name}");
            assert!(is_known(name));
        }
        assert!(listed.windows(2).all(|pair| pair[0] < pair[1]));
        // A rule in the form of POSIX is no name of the database (ADR 0412).
        for text in ["XYZ3", "", "Europe", "Europe/Nowhere", "+03:00"] {
            assert!(!is_known(text), "{text}");
        }
    }

    #[test]
    fn the_clock_of_a_zone_follows_its_rules_through_the_year() {
        // Lisbon is at UTC in winter and one hour ahead in summer.
        let winter = utc(2027, 1, 15, 22, 30);
        let summer = utc(2027, 7, 15, 22, 30);
        assert_eq!(minute_in("Europe/Lisbon", winter), Some(22 * 60 + 30));
        assert_eq!(minute_in("Europe/Lisbon", summer), Some(23 * 60 + 30));
        assert_eq!(minute_in("UTC", summer), Some(22 * 60 + 30));
        // A zone with a part of an hour, and a day that is the next one there.
        assert_eq!(minute_in("Asia/Kolkata", winter), Some(4 * 60));
        // A zone behind UTC.
        assert_eq!(minute_in("America/Sao_Paulo", winter), Some(19 * 60 + 30));
        // The instant counts, whatever offset it is written with.
        let written = crate::moment::in_offset(winter, UtcOffset::from_hms(-3, 0, 0).unwrap());
        assert_eq!(minute_in("Europe/Lisbon", written), Some(22 * 60 + 30));
        // The first and the last minute of a day.
        assert_eq!(minute_in("UTC", utc(2027, 1, 15, 0, 0)), Some(0));
        let last = utc(2027, 1, 15, 23, 59) + time::Duration::seconds(59);
        assert_eq!(minute_in("UTC", last), Some(1439));
        // A name that this build does not hold gives no clock, and so no quiet time.
        assert_eq!(minute_in("Europe/Nowhere", winter), None);
        assert_eq!(minute_in("XYZ3", winter), None);
    }

    #[test]
    fn the_hour_that_a_clock_skips_and_the_hour_that_it_repeats() {
        // Lisbon, 28 March 2027: the clock goes from 00:59 to 02:00.
        assert_eq!(
            minute_in("Europe/Lisbon", utc(2027, 3, 28, 0, 59)),
            Some(59)
        );
        assert_eq!(
            minute_in("Europe/Lisbon", utc(2027, 3, 28, 1, 0)),
            Some(2 * 60)
        );
        // Lisbon, 31 October 2027: the hour from 01:00 to 01:59 is shown twice.
        assert_eq!(
            minute_in("Europe/Lisbon", utc(2027, 10, 31, 0, 30)),
            Some(60 + 30)
        );
        assert_eq!(
            minute_in("Europe/Lisbon", utc(2027, 10, 31, 1, 30)),
            Some(60 + 30)
        );
    }

    #[test]
    fn the_zone_of_the_system_is_a_name_of_the_list() {
        #[allow(clippy::disallowed_methods)]
        let system = system();
        assert!(is_known(&system));
        assert_eq!(known_or_fallback(Some("Europe/Lisbon")), "Europe/Lisbon");
        // A system that states nothing, or a name that this build does not hold.
        assert_eq!(known_or_fallback(None), FALLBACK);
        assert_eq!(known_or_fallback(Some("Mars/Olympus")), FALLBACK);
        assert_eq!(known_or_fallback(Some("")), FALLBACK);
        assert!(is_known(FALLBACK));
    }

    #[test]
    fn a_search_finds_names_by_a_part_without_case_and_with_spaces() {
        let (found, more) = matching("sao pau");
        assert_eq!(found, ["America/Sao_Paulo"]);
        assert!(!more);
        assert_eq!(matching("SAO_PAULO").0, ["America/Sao_Paulo"]);
        assert_eq!(matching("  lisbon ").0, ["Europe/Lisbon"]);
        // A name whose last part starts with the text comes before a name that holds it
        // elsewhere.
        let (found, _) = matching("lima");
        assert_eq!(found.first(), Some(&"America/Lima"));
        let (found, _) = matching("port");
        let place = |name: &str| name.rsplit('/').next().unwrap().to_lowercase();
        let starts = found
            .iter()
            .take_while(|name| place(name).starts_with("port"))
            .count();
        assert!(starts > 0);
        assert!(
            found[starts..]
                .iter()
                .all(|name| !place(name).starts_with("port"))
        );
        // Nothing typed: the first names of the list, and the statement that more exist.
        let (all, more) = matching("");
        assert_eq!(all.len(), MOST_MATCHES);
        assert!(more);
        assert_eq!(all[0], names().next().unwrap());
        assert_eq!(matching("no such place"), (Vec::new(), false));
        assert_eq!(shown("America/Sao_Paulo"), "America/Sao Paulo");
    }
}
