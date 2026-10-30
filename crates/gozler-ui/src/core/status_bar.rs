use std::time::{Duration, Instant};

use super::layout::Px;

pub(crate) fn work_widths(room: Px, wanted: &[Px], minimum: &[Px], gap: Px) -> Vec<Px> {
    let available = (room.0 - gap.0 * wanted.len().saturating_sub(1) as i32).max(0);
    let minimum: Vec<i32> = wanted
        .iter()
        .zip(minimum)
        .map(|(wanted, minimum)| wanted.0.min(minimum.0))
        .collect();
    let total_minimum: i32 = minimum.iter().sum();
    let extra: i32 = wanted
        .iter()
        .zip(&minimum)
        .map(|(wanted, minimum)| wanted.0 - minimum)
        .sum();
    let spare = (available - total_minimum).max(0).min(extra);
    wanted
        .iter()
        .zip(minimum)
        .map(|(wanted, minimum)| {
            Px(if total_minimum > available {
                (i64::from(minimum) * i64::from(available) / i64::from(total_minimum)) as i32
            } else {
                minimum
                    + (i64::from(wanted.0 - minimum) * i64::from(spare) / i64::from(extra.max(1)))
                        as i32
            })
        })
        .collect()
}

const NOTICE_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Default)]
pub(crate) enum Notice {
    #[default]
    Persistent,
    Until(Instant),
    Expired,
}

impl Notice {
    pub(crate) fn update(&mut self, transient: bool, now: Instant) {
        if !transient {
            *self = Self::Persistent;
            return;
        }
        match *self {
            Self::Persistent => *self = Self::Until(now + NOTICE_DURATION),
            Self::Until(until) if now >= until => *self = Self::Expired,
            Self::Until(_) | Self::Expired => {}
        }
    }

    pub(crate) fn visible(self) -> bool {
        !matches!(self, Self::Expired)
    }

    pub(crate) fn remaining(self, now: Instant) -> Option<Duration> {
        match self {
            Self::Until(until) => Some(until.saturating_duration_since(now)),
            Self::Persistent | Self::Expired => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_work_shares_room_without_losing_its_controls() {
        assert_eq!(
            work_widths(Px(350), &[Px(300), Px(300)], &[Px(150), Px(150)], Px(6)),
            [Px(172), Px(172)]
        );
        assert_eq!(
            work_widths(Px(80), &[Px(300), Px(300)], &[Px(150), Px(150)], Px(6)),
            [Px(37), Px(37)]
        );
    }

    #[test]
    fn repeated_updates_do_not_keep_a_completion_notice_alive() {
        let start = Instant::now();
        let mut notice = Notice::default();
        notice.update(true, start);
        notice.update(true, start + Duration::from_secs(4));
        assert!(notice.visible());
        assert_eq!(
            notice.remaining(start + Duration::from_secs(4)),
            Some(Duration::from_secs(1))
        );
        notice.update(true, start + NOTICE_DURATION);
        assert!(!notice.visible());
        notice.update(true, start + Duration::from_secs(30));
        assert!(!notice.visible());
        notice.update(false, start + Duration::from_secs(31));
        assert!(notice.visible());
        assert_eq!(notice.remaining(start + Duration::from_secs(31)), None);
    }
}
