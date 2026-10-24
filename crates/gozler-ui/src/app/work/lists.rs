//! List selection/cache passes and Location inspection share execution policy;
//! accepted Location observations belong to the consumer.

use crate::core::fetch::List;
use crate::core::model::ScopeCheck;
use crate::core::search_list::{Keep, Kept};
use crate::core::selection::{Filter, Filtered, Telling, Told};
use crate::core::work::{Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::time::Instant;

struct Slot {
    list: List,
    telling: Task<Telling, Told>,
    keeping: Task<Keep, Kept>,
    filtering: Task<Filter, Filtered>,
}

pub(in crate::app) struct Work {
    slots: Vec<Slot>,
    location: Task<String, (String, ScopeCheck)>,
    folder: Option<String>,
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            slots: List::ALL
                .into_iter()
                .map(|list| Slot {
                    list,
                    telling: Task::pass(
                        waker.clone(),
                        Replacement::Finish,
                        Telling::items,
                        |input, _| input.run(),
                    ),
                    keeping: Task::pass(
                        waker.clone(),
                        Replacement::Finish,
                        Keep::items,
                        |input, _| input.run(),
                    ),
                    filtering: Task::pass(
                        waker.clone(),
                        Replacement::Cancel,
                        Filter::retained_items,
                        |input, _| input.run(),
                    ),
                })
                .collect(),
            location: Task::new(waker, Replacement::Cancel, check_location).background(),
            folder: None,
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn check_location(&mut self, folder: Option<&str>) {
        if self.folder.as_deref() != folder {
            self.folder = folder.map(str::to_owned);
            match folder {
                Some(folder) => {
                    self.location.replace(folder.to_owned());
                }
                None => self.location.cancel(),
            }
        }
    }

    pub(in crate::app) fn refresh_location(&mut self) {
        if let Some(folder) = &self.folder {
            self.location.replace(folder.clone());
        }
    }

    pub(in crate::app) fn take_location(&mut self) -> Option<(String, ScopeCheck)> {
        self.location.take().map(|(_, answer)| answer)
    }

    pub(in crate::app) fn keep(&mut self, list: List, input: Keep) {
        let task = self
            .slots
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        task.keeping.replace(input);
        let retired = task.keeping.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn tell(&mut self, list: List, input: Telling) {
        let work = self
            .slots
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        work.telling.replace(input);
        let retired = work.telling.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_told(&mut self) -> Vec<(List, Told)> {
        let mut answers = Vec::new();
        let mut retired = Retirement::default();
        for work in &mut self.slots {
            if let Some((_, answer)) = work.telling.take() {
                answers.push((work.list, answer));
            }
            retired.append(work.telling.take_retired());
        }
        self.retired.append(retired);
        answers
    }

    pub(in crate::app) fn filter(&mut self, list: List, input: Filter) {
        let task = self
            .slots
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        task.filtering.replace(input);
        let retired = task.filtering.take_retired();
        self.retired.append(retired);
    }

    pub(in crate::app) fn take_kept(&mut self) -> Vec<(List, Kept)> {
        let mut retired = Retirement::default();
        let answers = self
            .slots
            .iter_mut()
            .filter_map(|work| {
                let answer = work.keeping.take().map(|(_, answer)| (work.list, answer));
                retired.append(work.keeping.take_retired());
                answer
            })
            .collect();
        self.retired.append(retired);
        answers
    }

    pub(in crate::app) fn take_filtered(&mut self) -> Vec<(List, Filtered)> {
        let mut retired = Retirement::default();
        let answers = self
            .slots
            .iter_mut()
            .filter_map(|work| {
                let answer = work.filtering.take().map(|(_, answer)| (work.list, answer));
                retired.append(work.filtering.take_retired());
                answer
            })
            .collect();
        self.retired.append(retired);
        answers
    }

    pub(super) fn slow(&self, list: List, now: Instant) -> bool {
        self.slots.iter().any(|work| {
            work.list == list
                && (work.telling.waiting(now) == Some(Waiting::Slow)
                    || work.filtering.waiting(now) == Some(Waiting::Slow))
        })
    }

    pub(super) fn cancel_selection(&mut self, list: List) {
        let work = self
            .slots
            .iter_mut()
            .find(|work| work.list == list)
            .expect("each list has a work slot");
        work.telling.cancel();
        work.filtering.cancel();
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = self.location.tick(now).is_some();
        for work in &mut self.slots {
            active |= work.telling.tick(now).is_some();
            active |= work.keeping.tick(now).is_some();
            active |= work.filtering.tick(now).is_some();
        }
        active
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        self.retired.append(self.location.take_retired());
        for work in &mut self.slots {
            self.retired.append(work.telling.take_retired());
            self.retired.append(work.keeping.take_retired());
            self.retired.append(work.filtering.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        self.location.cancel();
        self.retired.append(self.location.close_delivery());
        for work in &mut self.slots {
            work.telling.cancel();
            work.keeping.cancel();
            work.filtering.cancel();
            self.retired.append(work.telling.close_delivery());
            self.retired.append(work.keeping.close_delivery());
            self.retired.append(work.filtering.close_delivery());
        }
        self.take_retired()
    }
}

fn check_location(
    folder: String,
    _worker: &OffThread,
    cancel: &CancelToken,
) -> (String, ScopeCheck) {
    let scope = if !cancel.is_cancelled() && gozler_client::folder_is_missing(&folder) {
        ScopeCheck::Missing
    } else {
        ScopeCheck::Exists
    };
    (folder, scope)
}
