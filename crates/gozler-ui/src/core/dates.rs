//! The date style of file lists, with one local clock snapshot for every row.
//! The inspector keeps the exact timestamp regardless of this choice.

use super::text::{Numbers, Strings};
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

    pub(crate) fn text(&self, strings: &Strings, numbers: &dyn Numbers, filetime: i64) -> String {
        if self.format == DateFormat::Full {
            return numbers.date_time(filetime);
        }
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
