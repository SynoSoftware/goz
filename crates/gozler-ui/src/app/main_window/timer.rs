use crate::core::fetch::List;
use crate::core::panel::Side;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Timer {
    Search(List),
    Searching(List),
    Selection(List),
    Status,
    Age,
    DropScroll,
    Save,
    PanelSettle(Side),
    PanelAgain(Side),
    PanelReading(Side),
    OperationsPoll,
    Dates,
    Clipboard,
    Work,
    NativeSelection,
    OperationDelivery,
    StatusNotice,
}

const IDS: &[(Timer, usize)] = &[
    (Timer::Search(List::Search), 1),
    (Timer::Status, 2),
    (Timer::Selection(List::Search), 3),
    (Timer::Age, 4),
    (Timer::DropScroll, 6),
    (Timer::Save, 7),
    (Timer::PanelSettle(Side::Left), 8),
    (Timer::PanelSettle(Side::Right), 9),
    (Timer::PanelAgain(Side::Left), 10),
    (Timer::PanelAgain(Side::Right), 11),
    (Timer::Search(List::Panel(Side::Left)), 12),
    (Timer::Search(List::Panel(Side::Right)), 13),
    (Timer::Selection(List::Panel(Side::Left)), 14),
    (Timer::Selection(List::Panel(Side::Right)), 15),
    (Timer::OperationsPoll, 16),
    (Timer::Searching(List::Search), 18),
    (Timer::Searching(List::Panel(Side::Left)), 19),
    (Timer::Searching(List::Panel(Side::Right)), 20),
    (Timer::Dates, 21),
    (Timer::Clipboard, 22),
    (Timer::Work, 23),
    (Timer::NativeSelection, 24),
    (Timer::OperationDelivery, 25),
    (Timer::PanelReading(Side::Left), 26),
    (Timer::PanelReading(Side::Right), 27),
    (Timer::StatusNotice, 28),
];

impl Timer {
    pub(super) fn id(self) -> usize {
        IDS.iter()
            .find_map(|(timer, id)| (*timer == self).then_some(*id))
            .expect("each timer has one Windows ID")
    }

    pub(super) fn from_id(id: usize) -> Option<Self> {
        IDS.iter()
            .find_map(|(timer, each)| (*each == id).then_some(*timer))
    }
}
