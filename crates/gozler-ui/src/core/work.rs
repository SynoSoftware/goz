//! Admission for replaceable UI work. Cancelling an executing call does not
//! release its slot: only completion can admit the latest pending input.

use std::collections::LinkedList;
use std::time::Duration;

#[cfg(test)]
mod scan;

pub(crate) const SESSION_FINALIZATION: Duration = Duration::from_secs(2);
pub(crate) const INLINE_ITEMS: usize = 1_000;
pub(crate) const SLOW: Duration = Duration::from_secs(3);
const BACKGROUND_LIMIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub(crate) enum Interest {
    Awaited,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    Working,
    Slow,
    Expired,
}

impl Interest {
    pub(crate) fn after(self, elapsed: Duration) -> Waiting {
        match self {
            Self::Background if elapsed >= BACKGROUND_LIMIT => Waiting::Expired,
            Self::Awaited if elapsed >= SLOW => Waiting::Slow,
            _ => Waiting::Working,
        }
    }
}

pub(crate) enum Pass<I, O> {
    Ready(O),
    Deferred(I),
}

pub(crate) fn pass<I, O>(items: usize, input: I, run: impl FnOnce(I) -> O) -> Pass<I, O> {
    if items <= INLINE_ITEMS {
        Pass::Ready(run(input))
    } else {
        Pass::Deferred(input)
    }
}

/// Coalescing transfers ownership in constant time; the worker drops the values.
#[derive(Default)]
pub(crate) struct Retirement {
    values: LinkedList<Box<dyn Send>>,
    items: usize,
}

impl std::fmt::Debug for Retirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retirement")
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl Retirement {
    pub(crate) fn push<T: Send + 'static>(&mut self, value: T, items: usize) {
        if let Pass::Deferred(value) = pass(items, value, drop) {
            self.values.push_back(Box::new(value));
            self.items = self.items.saturating_add(items);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn items(&self) -> usize {
        self.items
    }

    pub(crate) fn append(&mut self, mut next: Self) {
        self.values.append(&mut next.values);
        self.items = self.items.saturating_add(next.items);
    }

    pub(crate) fn combine(mut self, next: Self) -> Self {
        self.append(next);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Id(u64);

pub(crate) struct Started<T> {
    pub(crate) id: Id,
    pub(crate) input: T,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replacement {
    Cancel,
    Finish,
}

pub(crate) struct Slot<T> {
    replacement: Replacement,
    next: u64,
    current: Option<Id>,
    executing: Option<Id>,
    pending: Option<Started<T>>,
}

impl<T> Slot<T> {
    pub(crate) fn new(replacement: Replacement) -> Self {
        Self {
            replacement,
            next: 0,
            current: None,
            executing: None,
            pending: None,
        }
    }

    pub(crate) fn replacement(&self) -> Replacement {
        self.replacement
    }

    pub(crate) fn is_running(&self) -> bool {
        self.executing.is_some()
    }

    /// Retains one pending input, combining it with a superseded pending input
    /// when the caller must preserve accumulated changes instead of replacing it.
    pub(crate) fn submit(
        &mut self,
        input: T,
        combine: impl FnOnce(T, T) -> T,
    ) -> Option<Started<T>> {
        self.next = self.next.checked_add(1).expect("task ids cannot wrap");
        let id = Id(self.next);
        self.current = Some(id);
        let input = match self.pending.take() {
            Some(old) => combine(old.input, input),
            None => input,
        };
        let started = Started { id, input };
        if self.executing.is_some() {
            self.pending = Some(started);
            None
        } else {
            self.executing = Some(id);
            Some(started)
        }
    }

    /// Returns the displaced pending input so its owner chooses where it drops.
    pub(crate) fn cancel(&mut self) -> Option<T> {
        self.current = None;
        self.pending.take().map(|pending| pending.input)
    }

    pub(crate) fn is_current(&self, id: Id) -> bool {
        self.current == Some(id)
    }

    pub(crate) fn current(&self) -> Option<Id> {
        self.current
    }

    /// Whether the result is current, followed by the next admitted input.
    pub(crate) fn complete(&mut self, id: Id) -> (bool, Option<Started<T>>) {
        assert_eq!(
            self.executing,
            Some(id),
            "only the executing task completes"
        );
        let accepted = self.current.is_some()
            && (self.replacement == Replacement::Finish || self.current == Some(id));
        let next = self.pending.take();
        self.executing = next.as_ref().map(|task| task.id);
        (accepted, next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_background_work_expires_while_awaited_work_stays_slow() {
        assert_eq!(
            Interest::Awaited.after(SLOW - Duration::from_nanos(1)),
            Waiting::Working
        );
        assert_eq!(Interest::Awaited.after(SLOW), Waiting::Slow);
        assert_eq!(
            Interest::Background.after(BACKGROUND_LIMIT - Duration::from_nanos(1)),
            Waiting::Working
        );
        assert_eq!(
            Interest::Background.after(BACKGROUND_LIMIT),
            Waiting::Expired
        );
        assert_eq!(
            Interest::Awaited.after(Duration::from_secs(3_600)),
            Waiting::Slow
        );
    }

    #[test]
    fn cancelled_native_work_keeps_its_slot_and_only_the_latest_replacement() {
        let mut location = Slot::new(Replacement::Cancel);
        let first = location.submit("blocked share", |_, new| new).unwrap();
        let _ = location.cancel();
        assert!(location.submit("superseded", |_, new| new).is_none());
        assert!(location.submit("latest folder", |_, new| new).is_none());

        let (accepted, next) = location.complete(first.id);
        assert!(!accepted);
        let next = next.unwrap();
        assert_eq!(next.input, "latest folder");
        let (accepted, next) = location.complete(next.id);
        assert!(accepted);
        assert!(next.is_none());
    }

    #[test]
    fn serial_writes_keep_completion_and_combine_pending_changes() {
        let mut saving = Slot::new(Replacement::Finish);
        let first = saving.submit(vec!["first"], |_, new| new).unwrap();
        assert!(saving.submit(vec!["pending"], |_, new| new).is_none());
        assert!(
            saving
                .submit(vec!["last"], |mut old, new| {
                    old.extend(new);
                    old
                })
                .is_none()
        );
        let (accepted, next) = saving.complete(first.id);
        assert!(accepted);
        let next = next.unwrap();
        assert_eq!(next.input, ["pending", "last"]);
        let (accepted, next) = saving.complete(next.id);
        assert!(accepted);
        assert!(next.is_none());
    }

    #[test]
    fn cancelling_after_delivery_invalidates_a_deferred_command() {
        let mut pairing = Slot::new(Replacement::Cancel);
        let task = pairing.submit("confirmed program", |_, new| new).unwrap();
        assert!(pairing.complete(task.id).0);
        assert!(pairing.is_current(task.id));
        let _ = pairing.cancel();
        assert!(!pairing.is_current(task.id));
    }
}
