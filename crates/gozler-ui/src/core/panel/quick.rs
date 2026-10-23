//! Quick-search keystrokes run in order against shared rows. Navigation and
//! newer native interaction invalidate a late focus change.

use super::*;
use crate::core::fetch::CachedRows;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
pub(crate) enum QuickAction {
    Character(char),
    Back,
    Step { down: bool },
}

#[derive(Debug, Default)]
pub(super) struct Quick {
    pub(super) text: Option<String>,
    pending: VecDeque<QuickAction>,
    revision: u64,
    running: Option<u64>,
}

impl Quick {
    pub(super) fn active(&self) -> bool {
        self.text.is_some() || self.running == Some(self.revision) || !self.pending.is_empty()
    }

    pub(super) fn close(&mut self, retired: &mut Retirement) -> bool {
        let active = self.active();
        self.text = None;
        self.revision += 1;
        let pending = std::mem::take(&mut self.pending);
        let items = pending.len();
        retired.push(pending, items);
        active
    }
}

#[derive(Debug)]
enum Rows {
    Folder {
        rows: Arc<FolderRows>,
        up: u64,
    },
    Search {
        generation: Option<Generation>,
        chunks: Vec<CachedRows>,
        kept: Arc<HashSet<OperationId>>,
        keeping: bool,
    },
}

impl Rows {
    fn of(panel: &Panel) -> Self {
        match panel.search() {
            Some(search) => Self::Search {
                generation: search.list.results().generation(),
                chunks: search.list.results().cached(),
                kept: Arc::clone(&search.list.kept),
                keeping: search.list.is_keeping(),
            },
            None => Self::Folder {
                rows: Arc::clone(&panel.rows),
                up: panel.up_rows(),
            },
        }
    }

    fn items(&self) -> usize {
        match self {
            Self::Folder { rows, .. } => rows.items(),
            Self::Search { chunks, kept, .. } => {
                chunks.iter().map(|chunk| chunk.rows.len()).sum::<usize>() + kept.len()
            }
        }
    }

    fn matches(&self, panel: &Panel) -> bool {
        match (self, Self::of(panel)) {
            (
                Self::Folder { rows, up },
                Self::Folder {
                    rows: current,
                    up: current_up,
                },
            ) => Arc::ptr_eq(rows, &current) && *up == current_up,
            (
                Self::Search {
                    generation,
                    chunks,
                    kept,
                    keeping,
                },
                Self::Search {
                    generation: current_generation,
                    chunks: current_chunks,
                    kept: current_kept,
                    keeping: current_keeping,
                },
            ) => {
                *generation == current_generation
                    && *keeping == current_keeping
                    && Arc::ptr_eq(kept, &current_kept)
                    && chunks.len() == current_chunks.len()
                    && chunks.iter().zip(current_chunks).all(|(old, current)| {
                        old.range == current.range && Arc::ptr_eq(&old.rows, &current.rows)
                    })
            }
            _ => false,
        }
    }

    fn first_match(&self, taken: &Taken, text: &str, from: u64, down: bool) -> Option<u64> {
        let wanted = fold(text.as_bytes());
        let mut folding = Folding::default();
        match self {
            Self::Folder { rows, up } => {
                let matches = |row: &u64| {
                    rows.entry(*row, *up)
                        .is_some_and(|entry| folding.of(&entry.name).starts_with(&wanted))
                };
                if down {
                    (from..rows.count(*up)).find(matches)
                } else {
                    (0..from).rev().find(matches)
                }
            }
            Self::Search {
                chunks,
                kept,
                keeping,
                ..
            } => {
                let mut chunks: Vec<_> = chunks.iter().collect();
                chunks.sort_unstable_by_key(|chunk| chunk.range.start);
                if !down {
                    chunks.reverse();
                }
                chunks
                    .into_iter()
                    .flat_map(|chunk| {
                        let len = chunk.rows.len();
                        (0..len).map(move |at| {
                            let at = if down { at } else { len - 1 - at };
                            (chunk.range.start + at as u64, &chunk.rows[at])
                        })
                    })
                    .filter(|(at, _)| if down { *at >= from } else { *at < from })
                    .find(|(_, item)| {
                        folding.of_str(last_name(&item.path)).starts_with(&wanted)
                            && (taken.is_empty()
                                || !taken
                                    .contains(&super::super::selection::wide_path(item), |id| {
                                        *keeping || kept.contains(&id)
                                    }))
                    })
                    .map(|(at, _)| at)
            }
        }
    }
}

pub(crate) struct Finding {
    rows: Rows,
    taken: Arc<Taken>,
    epoch: u64,
    revision: u64,
    interaction: u64,
    text: Option<String>,
    focused: Option<u64>,
    action: QuickAction,
}

pub(crate) struct Found {
    input: Finding,
    text: Option<String>,
    row: Option<u64>,
}

impl Finding {
    pub(crate) fn items(&self) -> usize {
        self.rows.items() + self.taken.len()
    }

    pub(crate) fn run(self) -> Found {
        let mut text = self.text.clone();
        let row = match self.action {
            QuickAction::Character(c) => {
                let mut wanted = text.clone().unwrap_or_default();
                wanted.push(c);
                let row = self.rows.first_match(&self.taken, &wanted, 0, true);
                if row.is_some() {
                    text = Some(wanted);
                }
                row
            }
            QuickAction::Back => text.as_mut().and_then(|text| {
                text.pop();
                self.rows.first_match(&self.taken, text, 0, true)
            }),
            QuickAction::Step { down } => text.as_deref().and_then(|text| {
                self.focused.and_then(|from| {
                    self.rows
                        .first_match(&self.taken, text, from + u64::from(down), down)
                })
            }),
        };
        Found {
            input: self,
            text,
            row,
        }
    }
}

impl Panels {
    pub(crate) fn quick_active(&self, side: Side) -> bool {
        self.panel(side).quick.active()
    }

    pub(crate) fn find_quick(&mut self, side: Side, action: QuickAction) {
        let panel = self.panel_mut(side);
        if matches!(panel.view, View::Folder | View::Search(_)) {
            panel.quick.pending.push_back(action);
        }
    }

    pub(crate) fn take_quick(&mut self, side: Side) -> Option<Finding> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        if panel.quick.running.is_some() || panel.is_calculating() {
            return None;
        }
        let action = panel.quick.pending.pop_front()?;
        panel.quick.running = Some(panel.quick.revision);
        Some(Finding {
            rows: Rows::of(panel),
            taken,
            epoch: panel.epoch,
            revision: panel.quick.revision,
            interaction: panel.interaction_revision,
            text: panel.quick.text.clone(),
            focused: panel.focused,
            action,
        })
    }

    pub(crate) fn accept_quick(&mut self, side: Side, answer: Found) -> Option<u64> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        panel.quick.running = None;
        let current =
            answer.input.epoch == panel.epoch && answer.input.revision == panel.quick.revision;
        let row = if current && answer.input.interaction == panel.interaction_revision {
            if answer.input.rows.matches(panel) && Arc::ptr_eq(&answer.input.taken, &taken) {
                panel.quick.text = answer.text.clone();
                answer.row
            } else {
                panel.quick.pending.push_front(answer.input.action);
                None
            }
        } else {
            None
        };
        let items = answer.input.items();
        panel.retired.push(answer, items);
        row
    }
}
