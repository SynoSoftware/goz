use super::results::Shown;
use super::search_list::SearchList;
use super::selection::{FolderContents, Selection, Sums};
use gozler_core::attributes::Attributes;

#[derive(Default)]
pub(super) struct Inspector {
    shown: Facts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgeTimer {
    Keep,
    Restart,
    Stop,
}

#[derive(Default, PartialEq)]
enum Facts {
    #[default]
    Empty,
    Results(u64),
    TooMany(u64),
    Reading,
    Entry {
        path: String,
        is_folder: bool,
        size: Size,
        modified: Option<i64>,
        attributes: Option<Attributes>,
    },
    Several(Sums),
}

#[derive(PartialEq)]
enum Size {
    File(Option<u64>),
    Calculating,
    Folder {
        count: u64,
        bytes: u64,
        incomplete: bool,
    },
}

impl Inspector {
    pub(super) fn refresh(&mut self, list: &SearchList) -> AgeTimer {
        self.observe(Facts::of(list))
    }

    fn observe(&mut self, facts: Facts) -> AgeTimer {
        if self.shown == facts {
            return AgeTimer::Keep;
        }
        self.shown = facts;
        if self.modified().is_some() {
            AgeTimer::Restart
        } else {
            AgeTimer::Stop
        }
    }

    pub(super) fn modified(&self) -> Option<i64> {
        match self.shown {
            Facts::Entry { modified, .. } => modified,
            _ => None,
        }
    }
}

impl Facts {
    fn of(list: &SearchList) -> Self {
        let (entry, contents) = match list.selection() {
            Selection::None => {
                return match list.results().shown() {
                    Shown::Rows(_) => Self::Results(list.results().total()),
                    Shown::Nothing | Shown::Message(_) => Self::Empty,
                };
            }
            Selection::TooMany(count) => return Self::TooMany(*count),
            Selection::Reading { .. } | Selection::Filtering { .. } => return Self::Reading,
            Selection::Files(files) => match files.as_ref() {
                [entry] => (entry, None),
                _ => return Self::Several(list.selection_sums()),
            },
            Selection::Folder { folder, contents } => (folder, Some(contents)),
        };
        let size = match contents {
            None => Size::File(entry.size),
            Some(FolderContents::Calculating(_)) => Size::Calculating,
            Some(FolderContents::Counted(totals)) => Size::Folder {
                count: totals.count,
                bytes: totals.size,
                incomplete: totals.volumes_incomplete || totals.metadata_pending,
            },
        };
        Self::Entry {
            path: entry.path.clone(),
            is_folder: entry.is_dir,
            size,
            modified: entry.mtime_ft,
            attributes: entry.attributes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_inspection_keeps_the_age_timer_running() {
        let entry = |path: &str| Facts::Entry {
            path: path.to_owned(),
            is_folder: false,
            size: Size::File(Some(12)),
            modified: Some(100),
            attributes: None,
        };
        let mut inspector = Inspector::default();
        assert_eq!(inspector.observe(entry("a")), AgeTimer::Restart);
        assert_eq!(inspector.observe(entry("a")), AgeTimer::Keep);
        assert_eq!(inspector.observe(entry("b")), AgeTimer::Restart);
        assert_eq!(inspector.observe(Facts::Empty), AgeTimer::Stop);
    }
}
