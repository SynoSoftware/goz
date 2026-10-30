//! Folder transformations own their candidate until rows and interaction can
//! be published together. A remap moves that candidate without replaying a change.

use super::rows::{Focus, Remembered, ranges};
use super::*;
use crate::core::search_list::Ended;
use crate::core::selection::row_count;

#[derive(Clone, Debug)]
pub(super) struct Interaction {
    pub(super) revision: u64,
    pub(super) selected: Arc<Vec<Range<u64>>>,
    pub(super) focused: Option<u64>,
    pub(super) anchor: Option<u64>,
    pub(super) gone: Option<GoneFocus>,
    pub(super) awaited: Option<Awaited>,
}

impl Interaction {
    fn items(&self) -> usize {
        self.selected
            .len()
            .saturating_add(self.awaited.as_ref().map_or(0, Awaited::items))
    }
}

#[derive(Debug)]
pub(super) struct End {
    pub(super) moment: u64,
    pub(super) evidence: Arc<Ended>,
}

#[derive(Debug)]
pub(super) enum Change {
    Listed {
        folder: String,
        entries: Vec<Entry>,
        read: Read,
        same: bool,
        focus: Option<Box<[u16]>>,
    },
    Watch(Vec<FolderChange>),
    Filter,
    Sizes(Box<Outcome>),
    Ended,
    Await,
}

impl Change {
    pub(super) fn items(&self) -> usize {
        match self {
            Self::Listed { entries, .. } => entries.len(),
            Self::Watch(changes) => changes.len(),
            Self::Sizes(outcome) => outcome.items(),
            Self::Filter | Self::Ended | Self::Await => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdatedKind {
    Listed { again: bool },
    Watch,
    Taken,
    Sizes,
    Ended,
    Await,
}

#[derive(Debug)]
pub(crate) struct PanelUpdate {
    pub(crate) before: Place,
    pub(crate) place: Place,
    pub(crate) kind: UpdatedKind,
    pub(crate) read_again: bool,
    pub(crate) sizes: bool,
}

#[derive(Debug)]
struct Mapping {
    kind: UpdatedKind,
    read: Option<Read>,
    same: bool,
    focus: Option<Box<[u16]>>,
    reach: Option<u64>,
    changed: Vec<Box<[u16]>>,
    read_again: bool,
    sizes: bool,
}

#[derive(Debug)]
pub(crate) struct Calculation {
    pub(super) epoch: u64,
    pub(super) folder: String,
    pub(super) before: Arc<FolderRows>,
    pub(super) before_up: u64,
    pub(super) interaction: Interaction,
    pub(super) taken: Arc<Taken>,
    pub(super) ends: Vec<End>,
    pub(super) end_revision: u64,
    pub(super) input: Input,
}

#[derive(Debug)]
pub(super) enum Input {
    Change(Change),
    Remap(Box<Calculated>),
}

#[derive(Debug)]
pub(crate) struct Calculated {
    pub(super) folder: String,
    pub(super) epoch: u64,
    pub(super) before: Arc<FolderRows>,
    pub(super) rows: Option<FolderRows>,
    pub(super) taken: Arc<Taken>,
    pub(super) end_revision: u64,
    pub(super) interaction: Interaction,
    pub(super) place: Place,
    pub(super) anchor: Option<u64>,
    pub(super) selected: RowSelection,
    pub(super) awaited_place: Option<(Place, RowSelection)>,
    mapping: Mapping,
}

impl Calculation {
    pub(crate) fn items(&self) -> usize {
        let input = match &self.input {
            Input::Change(change) => change.items(),
            Input::Remap(answer) => answer.items(),
        };
        self.before
            .items()
            .saturating_add(self.interaction.items())
            .saturating_add(self.taken.len())
            .saturating_add(input)
            .saturating_add(
                self.ends
                    .iter()
                    .take(super::PENDING_ROWS + 1)
                    .map(|end| end.evidence.items())
                    .sum::<usize>(),
            )
    }

    pub(crate) fn run(self) -> Calculated {
        let Self {
            epoch,
            folder,
            before,
            before_up,
            mut interaction,
            taken,
            ends,
            end_revision,
            input,
        } = self;
        let existed = interaction.awaited.as_mut().is_some_and(|awaited| {
            let Some((origin, up)) = awaited.origin.take() else {
                return false;
            };
            let replaces = awaited
                .from
                .as_ref()
                .is_some_and(|from| origin.row_named(from, up).is_some());
            !replaces && origin.row_named(&awaited.name, up).is_some()
        });
        if existed {
            interaction.awaited = None;
        }
        let (mut rows, mut mapping) = match input {
            Input::Remap(mut answer) => (
                answer
                    .rows
                    .take()
                    .expect("an unpublished answer owns its rows"),
                answer.mapping,
            ),
            Input::Change(change) => {
                let mut rows = if matches!(change, Change::Listed { .. }) {
                    before.replacement()
                } else {
                    (*before).clone()
                };
                let mut mapping = Mapping {
                    kind: UpdatedKind::Taken,
                    read: None,
                    same: true,
                    focus: None,
                    reach: None,
                    changed: Vec::new(),
                    read_again: false,
                    sizes: false,
                };
                match change {
                    Change::Listed {
                        entries,
                        read,
                        same,
                        focus,
                        ..
                    } => {
                        mapping.kind = UpdatedKind::Listed {
                            again: same && read.sort == rows.order(),
                        };
                        mapping.same = same;
                        mapping.focus = focus;
                        rows.replace_listing(entries, read.sort);
                        mapping.read = Some(read);
                    }
                    Change::Watch(changes) => {
                        mapping.kind = UpdatedKind::Watch;
                        mapping.reach = Some(changes.len() as u64 + 1);
                        mapping.changed = changes
                            .iter()
                            .filter_map(|change| match change {
                                FolderChange::Added(entry)
                                | FolderChange::Modified(entry)
                                | FolderChange::Renamed { to: entry, .. } => {
                                    Some(entry.name.clone())
                                }
                                FolderChange::Gone(_) => None,
                            })
                            .collect();
                        mapping.read_again = rows.merge(changes);
                    }
                    Change::Filter => {}
                    Change::Sizes(outcome) => {
                        mapping.kind = UpdatedKind::Sizes;
                        rows.apply_sizes(&outcome);
                    }
                    Change::Ended => mapping.kind = UpdatedKind::Ended,
                    Change::Await => mapping.kind = UpdatedKind::Await,
                }
                (rows, mapping)
            }
        };
        let (read_again, sizes) = rows.follow_ends(ends, &folder);
        mapping.read_again |= read_again;
        mapping.sizes |= sizes;
        if let Some(read) = &mapping.read {
            let stale = rows.release(read.generation);
            mapping.read_again = stale;
        }
        rows.filter(&folder, &taken);
        let up = u64::from(!folder.is_empty() && !is_root(&folder));
        let mut remembered = if mapping.same {
            before.remember(interaction.focused, &interaction.selected, before_up)
        } else {
            Remembered::default()
        };
        let mut gone = interaction
            .gone
            .clone()
            .filter(|gone| gone.row == interaction.focused);
        let back = matches!(mapping.kind, UpdatedKind::Watch)
            && gone
                .as_ref()
                .is_some_and(|gone| mapping.changed.contains(&gone.name.raw));
        if back && let Some(gone) = &gone {
            let name = gone.name.clone();
            remembered.focused = Some(match remembered.focused.take() {
                Some(Focus::Entry { after, row, .. }) => Focus::Entry { name, after, row },
                _ => Focus::Entry {
                    name,
                    after: Vec::new(),
                    row: interaction.focused.unwrap_or(0),
                },
            });
        }
        let wanted = match &remembered.focused {
            Some(Focus::Entry { name, .. }) => Some(name.clone()),
            _ => None,
        };
        let mut place = if mapping.same {
            rows.place_of(remembered, mapping.reach, up)
        } else {
            Place {
                focused: mapping
                    .focus
                    .as_ref()
                    .and_then(|name| rows.row_of(&Name::of(name.clone()), None, up))
                    .or_else(|| (rows.count(up) > 0).then_some(0)),
                selected: Arc::default(),
            }
        };
        if place.selected.as_ref() == interaction.selected.as_ref() {
            place.selected = Arc::clone(&interaction.selected);
        }
        let anchor = if mapping.same {
            interaction
                .anchor
                .and_then(|row| match before.row(row, before_up)? {
                    Row::Up => (up != 0).then_some(0),
                    Row::Entry(entry) => rows.row_of(&entry.name(), None, up),
                    Row::Result(_) => None,
                })
        } else {
            place.focused
        };
        if matches!(mapping.kind, UpdatedKind::Watch) {
            let shows = |name: &Name| {
                place
                    .focused
                    .and_then(|row| rows.entry(row, up))
                    .is_some_and(|entry| entry.name == name.raw)
            };
            let name = match wanted {
                Some(name) if !shows(&name) => Some(gone.take().map_or(name, |gone| gone.name)),
                _ if back => None,
                _ => gone.take().map(|gone| gone.name),
            };
            interaction.gone = name.map(|name| GoneFocus {
                name,
                row: place.focused,
            });
        } else if matches!(mapping.kind, UpdatedKind::Listed { .. }) {
            interaction.gone = None;
        }
        if let Some(awaited) = &interaction.awaited
            && mapping
                .read
                .as_ref()
                .is_some_and(|read| awaited.ended.is_some_and(|ended| ended < read.generation))
            && rows.awaited_row(awaited, up).is_none()
        {
            interaction.awaited = None;
        }
        let awaited_place = interaction.awaited.as_ref().and_then(|awaited| {
            let row = rows.awaited_row(awaited, up)?;
            let mut selected = place.selected.clone();
            if awaited.select {
                let mut positions: Vec<u64> = selected.iter().flat_map(Clone::clone).collect();
                positions.push(row);
                positions.sort_unstable();
                positions.dedup();
                selected = Arc::new(ranges(&positions));
            }
            let shared = RowSelection {
                count: row_count(&selected),
                positions: Arc::clone(&selected),
            };
            Some((
                Place {
                    focused: Some(row),
                    selected,
                },
                shared,
            ))
        });
        let selected = RowSelection {
            count: row_count(&place.selected),
            positions: Arc::clone(&place.selected),
        };
        Calculated {
            epoch,
            folder,
            before,
            rows: Some(rows),
            taken,
            end_revision,
            interaction,
            place,
            anchor,
            selected,
            awaited_place,
            mapping,
        }
    }
}

impl Calculated {
    pub(super) fn started(&self) -> Option<u64> {
        self.mapping.read.as_ref().map(|read| read.generation)
    }

    pub(crate) fn items(&self) -> usize {
        self.before
            .items()
            .saturating_add(self.interaction.items())
            .saturating_add(self.rows.as_ref().map_or(0, FolderRows::items))
            .saturating_add(self.mapping.changed.len())
            .saturating_add(self.selected.positions.len())
            .saturating_add(self.taken.len())
    }

    pub(super) fn update(&mut self, before: Place) -> PanelUpdate {
        PanelUpdate {
            before,
            place: std::mem::take(&mut self.place),
            kind: self.mapping.kind,
            read_again: self.mapping.read_again,
            sizes: self.mapping.sizes,
        }
    }
}
