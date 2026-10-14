//! A moment that a server states, on the clock of an offset (ADR 0747).
//!
//! A server can state a moment at the end of the range of dates that this client can
//! write, such as the last second of the year 9999. On the clock of another offset that
//! moment lies outside of the range, and the conversion of the `time` library ends the
//! program there. Every conversion of the window goes through this module, and the method
//! of the library is a disallowed method of the crate (`clippy.toml`).

use time::{OffsetDateTime, UtcOffset};

/// The moment on the clock of `offset`. A moment that this clock cannot state stays on
/// the clock that it was written with.
pub fn in_offset(at: OffsetDateTime, offset: UtcOffset) -> OffsetDateTime {
    at.checked_to_offset(offset).unwrap_or(at)
}

/// The moment on the clock of UTC, with the rule of [`in_offset`].
pub fn in_utc(at: OffsetDateTime) -> OffsetDateTime {
    in_offset(at, UtcOffset::UTC)
}

#[cfg(test)]
mod tests {
    use time::format_description::well_known::Rfc3339;

    use super::*;

    fn moment(written: &str) -> OffsetDateTime {
        OffsetDateTime::parse(written, &Rfc3339).unwrap()
    }

    #[test]
    fn a_moment_at_the_end_of_the_range_stays_as_it_was_written() {
        let last = moment("9999-12-31T23:59:59Z");
        let east = UtcOffset::from_hms(1, 0, 0).unwrap();
        assert_eq!(in_offset(last, east), last);
        assert_eq!(in_offset(last, east).offset(), UtcOffset::UTC);
        // West of UTC the same moment has a date of the range.
        let west = UtcOffset::from_hms(-3, 0, 0).unwrap();
        assert_eq!(in_offset(last, west).hour(), 20);

        // A moment that is written west of UTC can lie after the range on the clock of
        // UTC.
        let written_west = moment("9999-12-31T23:59:59-01:00");
        assert_eq!(in_utc(written_west).offset(), written_west.offset());
        assert_eq!(in_utc(written_west).hour(), 23);

        let ordinary = moment("2027-01-15T23:30:00Z");
        assert_eq!(in_offset(ordinary, east).hour(), 0);
        assert_eq!(in_offset(ordinary, east).day(), 16);
    }
}
