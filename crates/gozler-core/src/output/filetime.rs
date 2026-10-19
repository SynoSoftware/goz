//! FILETIME conversion helpers, and the date format of the output.
//!
//! Windows FILETIME counts 100-nanosecond ticks since 1601-01-01T00:00:00
//! UTC. The USN and MFT records and gozler's wire protocol all speak FILETIME;
//! these helpers are the single conversion point for the date arithmetic that
//! needs Unix milliseconds. All functions are total: no input panics.

/// Ticks (100 ns units) from 1601-01-01T00:00:00 UTC to 1970-01-01T00:00:00
/// UTC, the offset between the FILETIME epoch and the Unix epoch.
pub const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

/// Converts a FILETIME tick count to Unix milliseconds.
///
/// Uses floor division, so pre-1970 inputs yield the correct (more negative)
/// millisecond, not a value rounded toward zero. Saturates instead of
/// overflowing for ticks within `FILETIME_UNIX_EPOCH` of `i64::MIN`.
pub fn filetime_to_unix_ms(ft: i64) -> i64 {
    ft.saturating_sub(FILETIME_UNIX_EPOCH).div_euclid(10_000)
}

/// Converts Unix milliseconds to a FILETIME tick count.
///
/// Exact for every representable result. When the millisecond value lies
/// outside the FILETIME domain (beyond roughly the year 31 million either
/// way) the tick multiplication saturates instead of overflowing: the high
/// corner clamps to `i64::MAX`, the low corner to
/// `i64::MIN + FILETIME_UNIX_EPOCH` (the epoch offset is added after the
/// saturated multiply).
pub fn unix_ms_to_filetime(ms: i64) -> i64 {
    ms.saturating_mul(10_000)
        .saturating_add(FILETIME_UNIX_EPOCH)
}

/// Formats a FILETIME as `YYYY-MM-DDTHH:MM:SS`, ISO 8601 in `tz`, so a
/// consumer parses it whatever the locale; empty for a time `jiff` cannot
/// hold.
pub fn format_date_local(filetime: i64, tz: &jiff::tz::TimeZone) -> String {
    match jiff::Timestamp::from_millisecond(filetime_to_unix_ms(filetime)) {
        Ok(ts) => ts
            .to_zoned(tz.clone())
            .strftime("%Y-%m-%dT%H:%M:%S")
            .to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn epoch_maps_to_unix_zero() {
        assert_eq!(filetime_to_unix_ms(FILETIME_UNIX_EPOCH), 0);
        assert_eq!(unix_ms_to_filetime(0), FILETIME_UNIX_EPOCH);
    }

    #[test]
    fn sub_millisecond_ticks_floor() {
        // 9_999 ticks < 1 ms after the epoch → still ms 0.
        assert_eq!(filetime_to_unix_ms(FILETIME_UNIX_EPOCH + 9_999), 0);
        // 1 tick BEFORE the epoch floors to ms -1, not 0.
        assert_eq!(filetime_to_unix_ms(FILETIME_UNIX_EPOCH - 1), -1);
    }

    #[test]
    fn extremes_do_not_panic() {
        let _ = filetime_to_unix_ms(i64::MIN);
        let _ = filetime_to_unix_ms(i64::MAX);
        // Saturating corners: high clamps to i64::MAX; low saturates the
        // multiply then adds the epoch offset.
        assert_eq!(unix_ms_to_filetime(i64::MAX), i64::MAX);
        assert_eq!(
            unix_ms_to_filetime(i64::MIN),
            i64::MIN + FILETIME_UNIX_EPOCH
        );
    }

    proptest! {
        /// ms → FILETIME → ms is exact for every ms whose FILETIME is
        /// representable (~year 31 million either way).
        #[test]
        fn ms_round_trips_exactly(ms in -900_000_000_000_000i64..=900_000_000_000_000) {
            prop_assert_eq!(filetime_to_unix_ms(unix_ms_to_filetime(ms)), ms);
        }

        /// FILETIME → ms → FILETIME floors to the containing millisecond:
        /// never later than the input, less than 10_000 ticks earlier.
        #[test]
        fn filetime_round_trip_floors(
            ft in (i64::MIN + FILETIME_UNIX_EPOCH + 10_000)..=i64::MAX
        ) {
            let back = unix_ms_to_filetime(filetime_to_unix_ms(ft));
            prop_assert!(back <= ft);
            prop_assert!(ft - back < 10_000);
        }
    }
}
