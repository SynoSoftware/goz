use std::time::Duration;

pub(crate) const HEARTBEAT: Duration = Duration::from_millis(100);
pub(crate) const STALL: Duration = Duration::from_millis(200);
const REPEAT: Duration = Duration::from_secs(1);
pub(crate) const LOG_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) message: u32,
    pub(crate) control: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContextStack {
    pub(crate) current: Option<Context>,
    pub(crate) depth: usize,
}

impl ContextStack {
    /// The caller restores the returned context after its nested handlers end.
    pub(crate) fn enter(&mut self, context: Context) -> Self {
        let previous = *self;
        self.current = Some(context);
        self.depth += 1;
        previous
    }
}

struct Unanswered {
    sequence: usize,
    since: Duration,
    cycles: Option<u64>,
    next_report: Duration,
}

#[derive(Default)]
pub(crate) struct Watchdog {
    next: usize,
    unanswered: Option<Unanswered>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Stall {
    pub(crate) wall: Duration,
    pub(crate) cpu_cycles: Option<u64>,
    pub(crate) context: ContextStack,
}

impl Watchdog {
    /// Times are elapsed from one monotonic origin; CPU counts use raw cycles.
    /// Repeating an unanswered sequence never restarts its interval.
    pub(crate) fn heartbeat(&mut self, now: Duration, cycles: Option<u64>) -> usize {
        self.unanswered
            .get_or_insert_with(|| {
                self.next = self.next.wrapping_add(1).max(1);
                Unanswered {
                    sequence: self.next,
                    since: now,
                    cycles,
                    next_report: now + STALL,
                }
            })
            .sequence
    }

    pub(crate) fn acknowledge(&mut self, sequence: usize) {
        if self
            .unanswered
            .as_ref()
            .is_some_and(|pending| pending.sequence == sequence)
        {
            self.unanswered = None;
        }
    }

    pub(crate) fn reset(&mut self) {
        self.unanswered = None;
    }

    pub(crate) fn overdue(
        &mut self,
        now: Duration,
        cycles: Option<u64>,
        context: ContextStack,
    ) -> Option<Stall> {
        let pending = self.unanswered.as_mut()?;
        if now < pending.next_report {
            return None;
        }
        pending.next_report = now + REPEAT;
        Some(Stall {
            wall: now.saturating_sub(pending.since),
            cpu_cycles: cycles
                .zip(pending.cycles)
                .and_then(|(now, since)| now.checked_sub(since)),
            context,
        })
    }
}

impl Stall {
    pub(crate) fn line(&self) -> String {
        let (message, control) = match self.context.current {
            Some(context) => (context.message.to_string(), context.control),
            None => ("none".to_owned(), None),
        };
        let control = control.map_or_else(|| "none".to_owned(), |id| id.to_string());
        let cycles = self
            .cpu_cycles
            .map_or_else(|| "unavailable".to_owned(), |cycles| cycles.to_string());
        format!(
            "wall_ms={} cpu_cycles={cycles} message={message} control={control} depth={}\n",
            self.wall.as_millis(),
            self.context.depth,
        )
    }
}

pub(crate) fn retain_log(bytes: &mut Vec<u8>, line: &[u8]) {
    if line.len() >= LOG_BYTES {
        bytes.clear();
        bytes.extend_from_slice(&line[line.len() - LOG_BYTES..]);
    } else {
        let remove = bytes
            .len()
            .saturating_add(line.len())
            .saturating_sub(LOG_BYTES);
        bytes.drain(..remove);
        bytes.extend_from_slice(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unanswered_heartbeats_report_until_the_matching_acknowledgment() {
        let mut watchdog = Watchdog::default();
        let context = ContextStack::default();
        let first = watchdog.heartbeat(Duration::ZERO, Some(1_000));
        assert_eq!(watchdog.heartbeat(HEARTBEAT, Some(1_200)), first);
        assert!(
            watchdog
                .overdue(STALL - Duration::from_millis(1), Some(1_900), context)
                .is_none()
        );
        let stall = watchdog.overdue(STALL, Some(1_900), context).unwrap();
        assert_eq!(
            (stall.wall, stall.cpu_cycles),
            (Duration::from_millis(200), Some(900))
        );
        assert!(
            watchdog
                .overdue(Duration::from_millis(1_199), Some(2_000), context)
                .is_none()
        );
        assert_eq!(
            watchdog
                .overdue(Duration::from_millis(1_200), Some(2_000), context)
                .unwrap()
                .wall,
            Duration::from_millis(1_200)
        );
        watchdog.acknowledge(first + 1);
        assert!(
            watchdog
                .overdue(Duration::from_millis(2_200), None, context)
                .is_some()
        );
        watchdog.acknowledge(first);
        assert!(
            watchdog
                .overdue(Duration::from_secs(4), Some(3_000), context)
                .is_none()
        );
        let next = watchdog.heartbeat(Duration::from_secs(4), None);
        assert_ne!(first, next);
        watchdog.reset();
        assert_ne!(watchdog.heartbeat(Duration::from_secs(5), None), next);
    }

    #[test]
    fn nested_context_restores_outer_ids_and_logs_only_numeric_context() {
        let mut stack = ContextStack::default();
        let outside = stack.enter(Context {
            message: 273,
            control: Some(12),
        });
        let outer = stack.enter(Context {
            message: 78,
            control: Some(24),
        });
        assert_eq!(stack.depth, 2);
        stack = outer;
        assert_eq!(
            Stall {
                wall: STALL,
                cpu_cycles: Some(900),
                context: stack
            }
            .line(),
            "wall_ms=200 cpu_cycles=900 message=273 control=12 depth=1\n"
        );
        stack = outside;
        assert_eq!(stack, ContextStack::default());
    }

    #[test]
    fn slow_log_retains_only_its_newest_bytes_even_for_an_oversized_line() {
        let mut bytes = vec![b'x'; LOG_BYTES];
        retain_log(&mut bytes, b"new\n");
        assert_eq!(bytes.len(), 65_536);
        assert_eq!(&bytes[65_532..], b"new\n");
        retain_log(&mut bytes, &vec![b'y'; LOG_BYTES + 1]);
        assert_eq!(bytes, vec![b'y'; 65_536]);
    }
}
