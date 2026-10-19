//! The Windows regional format and display languages. Windows formats the
//! numbers, so its regional rules have one owner (docs/plans/ui-design.md:
//! Text and languages).

use super::wide;
use crate::core::text::Numbers;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    DATE_SHORTDATE, GetDateFormatEx, GetLocaleInfoEx, GetNumberFormatEx, GetTimeFormatEx,
    GetUserPreferredUILanguages, LOCALE_ILZERO, LOCALE_INEGNUMBER, LOCALE_RETURN_NUMBER,
    LOCALE_SDECIMAL, LOCALE_SGROUPING, LOCALE_STHOUSAND, MUI_LANGUAGE_NAME, NUMBERFMTW,
    TIME_NOSECONDS,
};
use windows::core::{PCWSTR, PWSTR};

/// The user's regional number format, read when created; a change to the
/// regional format needs a new one.
pub(crate) struct WindowsNumbers {
    leading_zero: u32,
    grouping: u32,
    negative_order: u32,
    decimal: Vec<u16>,
    thousand: Vec<u16>,
}

impl WindowsNumbers {
    pub(crate) fn new() -> WindowsNumbers {
        WindowsNumbers {
            leading_zero: locale_number(LOCALE_ILZERO).unwrap_or(1),
            grouping: grouping(&locale_text(LOCALE_SGROUPING).unwrap_or_else(|| "3;0".to_owned())),
            negative_order: locale_number(LOCALE_INEGNUMBER).unwrap_or(1),
            decimal: wide(&locale_text(LOCALE_SDECIMAL).unwrap_or_else(|| ".".to_owned())),
            thousand: wide(&locale_text(LOCALE_STHOUSAND).unwrap_or_else(|| ",".to_owned())),
        }
    }

    /// `value`, written with `.` and no grouping, in the regional format
    /// with `digits` decimals.
    fn format(&self, value: &str, digits: u8) -> String {
        let mut decimal = self.decimal.clone();
        let mut thousand = self.thousand.clone();
        let format = NUMBERFMTW {
            NumDigits: u32::from(digits),
            LeadingZero: self.leading_zero,
            Grouping: self.grouping,
            lpDecimalSep: PWSTR(decimal.as_mut_ptr()),
            lpThousandSep: PWSTR(thousand.as_mut_ptr()),
            NegativeOrder: self.negative_order,
        };
        let mut buffer = [0u16; 64];
        let value = wide(value);
        // SAFETY: `format`, its separators and `value` outlive the call,
        // which writes at most the buffer's length.
        let written = unsafe {
            GetNumberFormatEx(
                PCWSTR::null(),
                0,
                PCWSTR(value.as_ptr()),
                Some(&format),
                Some(&mut buffer),
            )
        };
        match usize::try_from(written) {
            Ok(n) if n > 0 => String::from_utf16_lossy(&buffer[..n - 1]),
            _ => String::from_utf16_lossy(&value[..value.len() - 1]),
        }
    }
}

impl Numbers for WindowsNumbers {
    fn integer(&self, n: u64) -> String {
        self.format(&n.to_string(), 0)
    }

    fn decimal(&self, n: f64, digits: u8) -> String {
        self.format(&format!("{n:.*}", usize::from(digits)), digits)
    }

    /// Empty for a time that `jiff` cannot hold.
    fn date_time(&self, filetime: i64) -> String {
        let Ok(at) =
            jiff::Timestamp::from_millisecond(gozler_core::output::filetime_to_unix_ms(filetime))
        else {
            return String::new();
        };
        let local = at.to_zoned(jiff::tz::TimeZone::system()).datetime();
        let time = SYSTEMTIME {
            wYear: u16::try_from(local.year()).unwrap_or(0),
            wMonth: local.month() as u16,
            wDayOfWeek: local.weekday().to_sunday_zero_offset() as u16,
            wDay: local.day() as u16,
            wHour: local.hour() as u16,
            wMinute: local.minute() as u16,
            wSecond: local.second() as u16,
            wMilliseconds: 0,
        };
        let mut date = [0u16; 64];
        let mut clock = [0u16; 64];
        // SAFETY: `time` is a complete SYSTEMTIME; each call writes at most
        // its buffer's length.
        let (date_len, clock_len) = unsafe {
            (
                GetDateFormatEx(
                    PCWSTR::null(),
                    DATE_SHORTDATE,
                    Some(&time),
                    PCWSTR::null(),
                    Some(&mut date),
                    PCWSTR::null(),
                ),
                GetTimeFormatEx(
                    PCWSTR::null(),
                    TIME_NOSECONDS,
                    Some(&time),
                    PCWSTR::null(),
                    Some(&mut clock),
                ),
            )
        };
        let text = |buffer: &[u16], written: i32| match usize::try_from(written) {
            Ok(n) if n > 0 => String::from_utf16_lossy(&buffer[..n - 1]),
            _ => String::new(),
        };
        format!("{} {}", text(&date, date_len), text(&clock, clock_len))
    }
}

/// `LOCALE_SGROUPING` as `NUMBERFMTW` takes it: "3;0" is 3, "3;2;0" is 32,
/// and "3" without the repeating 0 is 30.
fn grouping(text: &str) -> u32 {
    let mut digits: String = text.split(';').collect();
    match digits.strip_suffix('0') {
        Some(repeating) if text.ends_with(";0") => digits = repeating.to_owned(),
        _ => digits.push('0'),
    }
    digits.parse().unwrap_or(3)
}

fn locale_text(kind: u32) -> Option<String> {
    let mut buffer = [0u16; 16];
    // SAFETY: the user's default locale; the call writes at most the
    // buffer's length.
    let written = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buffer)) };
    let n = usize::try_from(written).ok().filter(|n| *n > 0)?;
    Some(String::from_utf16_lossy(&buffer[..n - 1]))
}

fn locale_number(kind: u32) -> Option<u32> {
    let mut buffer = [0u16; 2];
    // SAFETY: with LOCALE_RETURN_NUMBER the call writes one u32 as two u16s.
    let written = unsafe {
        GetLocaleInfoEx(
            PCWSTR::null(),
            kind | LOCALE_RETURN_NUMBER,
            Some(&mut buffer),
        )
    };
    (written > 0).then(|| u32::from(buffer[0]) | (u32::from(buffer[1]) << 16))
}

/// The Windows display languages of the user, in their order of preference,
/// such as `de-DE`.
pub(crate) fn display_languages() -> Vec<String> {
    let mut count = 0u32;
    let mut length = 0u32;
    // SAFETY: with no buffer the call reports the length it needs.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut length) }
        .is_err()
    {
        return Vec::new();
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: `buffer` has the length the first call asked for.
    if unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    }
    .is_err()
    {
        return Vec::new();
    }
    buffer
        .split(|c| *c == 0)
        .filter(|tag| !tag.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}
