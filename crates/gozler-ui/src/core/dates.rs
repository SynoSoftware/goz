//! The date style of file lists, with one local clock snapshot for every row.
//! The inspector keeps the exact timestamp regardless of this choice.

use super::text::{Numbers, Strings, Text};
use gozler_core::output::filetime_to_unix_ms;
use jiff::{Timestamp, Zoned};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DateFormat {
    #[default]
    Full,
    DateOnly,
    Relative,
}

impl DateFormat {
    pub(crate) const ALL: [Self; 3] = [Self::Full, Self::DateOnly, Self::Relative];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Full => "date.full",
            Self::DateOnly => "date.date_only",
            Self::Relative => "date.relative",
        }
    }
}

pub(crate) struct Dates {
    pub(crate) format: DateFormat,
    now: Zoned,
}

impl Dates {
    pub(crate) fn new(format: DateFormat, now: Zoned) -> Self {
        Self { format, now }
    }

    pub(crate) fn set_time(&mut self, now: Zoned) {
        self.now = now;
    }

    pub(crate) fn text(&self, strings: &Strings, numbers: &dyn Numbers, filetime: i64) -> Text {
        Text {
            full: numbers.date_time(filetime),
            short: (self.format != DateFormat::Full)
                .then(|| self.short_text(strings, numbers, filetime)),
        }
    }

    fn short_text(&self, strings: &Strings, numbers: &dyn Numbers, filetime: i64) -> String {
        let Ok(at) = Timestamp::from_millisecond(filetime_to_unix_ms(filetime)) else {
            return String::new();
        };
        let date = at.to_zoned(self.now.time_zone().clone()).date();
        if self.format == DateFormat::Relative {
            let seconds = at.as_second() - self.now.timestamp().as_second();
            let distance = seconds.unsigned_abs();
            if distance < 60 {
                return strings.text("date.now").to_owned();
            }
            let today = self.now.date();
            if today.yesterday().ok() == Some(date) {
                return strings.text("date.yesterday").to_owned();
            }
            if today.tomorrow().ok() == Some(date) {
                return strings.text("date.tomorrow").to_owned();
            }
            if date == today {
                let (past, future, count) = if distance < 3_600 {
                    ("date.minutes_ago", "date.in_minutes", distance / 60)
                } else {
                    ("date.hours_ago", "date.in_hours", distance / 3_600)
                };
                let key = if seconds < 0 { past } else { future };
                return strings.format(key, &[&numbers.integer(count)]);
            }
        }
        numbers.date(date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::tests::{English, english};
    use gozler_core::output::unix_ms_to_filetime;

    #[test]
    fn a_short_date_format_keeps_the_full_timestamp_when_it_fits() {
        let now: Zoned = "2026-10-24T12:00:00Z[UTC]".parse().unwrap();
        let filetime = unix_ms_to_filetime(now.timestamp().as_millisecond());
        for (format, short) in [
            (DateFormat::Full, "2026-10-24T12:00:00"),
            (DateFormat::DateOnly, "2026-10-24"),
            (DateFormat::Relative, "Now"),
        ] {
            let dates = Dates::new(format, now.clone());
            for (room, expected) in [(19, "2026-10-24T12:00:00"), (18, short)] {
                assert_eq!(
                    dates
                        .text(&english(), &English, filetime)
                        .fitting(|full| full.len() <= room),
                    expected,
                );
            }
        }
    }

    #[test]
    fn relative_fallback_uses_calendar_days_and_short_dates_outside_them() {
        let dates = Dates::new(
            DateFormat::Relative,
            "2026-10-24T00:05:00-03:00[America/Cayenne]"
                .parse()
                .unwrap(),
        );
        for (at, expected) in [
            ("2026-10-22T23:59:00-03:00", "2026-10-22"),
            ("2026-10-23T23:59:00-03:00", "Yesterday"),
            ("2026-10-24T00:00:00-03:00", "5 min ago"),
            ("2026-10-25T23:59:00-03:00", "Tomorrow"),
            ("2026-10-26T00:00:00-03:00", "2026-10-26"),
        ] {
            let at: Timestamp = at.parse().unwrap();
            assert_eq!(
                dates
                    .text(
                        &english(),
                        &English,
                        unix_ms_to_filetime(at.as_millisecond()),
                    )
                    .compact(),
                expected,
            );
        }
    }
}
