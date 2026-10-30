use std::time::{Duration, Instant};

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
