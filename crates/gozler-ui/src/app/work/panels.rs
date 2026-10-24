//! Per-panel row calculation, counting and quick-search execution.

use crate::core::panel::{Calculated, Calculation, Counted, Counting, Finding, Found, Side};
use crate::core::work::{Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::Task;
use std::time::Instant;

pub(in crate::app) struct Work {
    calculations: [Task<Calculation, Calculated>; 2],
    counts: [Task<Counting, Counted>; 2],
    finding: [Task<Finding, Found>; 2],
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            calculations: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Cancel,
                    Calculation::items,
                    |input, _| input.run(),
                )
            }),
            counts: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Cancel,
                    Counting::items,
                    |input, _| input.run(),
                )
            }),
            finding: std::array::from_fn(|_| {
                Task::pass(
                    waker.clone(),
                    Replacement::Finish,
                    Finding::items,
                    |input, _| input.run(),
                )
            }),
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn calculate(&mut self, side: Side, input: Calculation) {
        let task = &mut self.calculations[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_calculated(&mut self) -> Vec<(Side, Calculated)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.calculations[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retired.append(retired);
        answers
    }

    pub(in crate::app) fn count(&mut self, side: Side, input: Counting) {
        let task = &mut self.counts[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn find(&mut self, side: Side, input: Finding) {
        let task = &mut self.finding[side.index()];
        task.replace(input);
        let retired = task.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_found(&mut self) -> Vec<(Side, Found)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.finding[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retired.append(retired);
        answers
    }

    pub(in crate::app) fn take_counts(&mut self) -> Vec<(Side, Counted)> {
        let mut retired = Retirement::default();
        let mut answers = Vec::new();
        for side in Side::BOTH {
            let task = &mut self.counts[side.index()];
            if let Some((_, answer)) = task.take() {
                answers.push((side, answer));
            }
            retired.append(task.take_retired());
        }
        self.retired.append(retired);
        answers
    }

    pub(in crate::app) fn waiting(&self, side: Side, now: Instant) -> Option<Waiting> {
        self.calculations[side.index()].waiting(now)
    }

    pub(in crate::app) fn cancel(&mut self, side: Side) {
        self.calculations[side.index()].cancel();
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = false;
        for task in &mut self.calculations {
            active |= task.tick(now).is_some();
        }
        for task in &mut self.counts {
            active |= task.tick(now).is_some();
        }
        for task in &mut self.finding {
            active |= task.tick(now).is_some();
        }
        active
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        for task in &mut self.calculations {
            self.retired.append(task.take_retired());
        }
        for task in &mut self.counts {
            self.retired.append(task.take_retired());
        }
        for task in &mut self.finding {
            self.retired.append(task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        for task in &mut self.calculations {
            task.cancel();
            self.retired.append(task.close_delivery());
        }
        for task in &mut self.counts {
            task.cancel();
            self.retired.append(task.close_delivery());
        }
        for task in &mut self.finding {
            task.cancel();
            self.retired.append(task.close_delivery());
        }
        self.take_retired()
    }
}
