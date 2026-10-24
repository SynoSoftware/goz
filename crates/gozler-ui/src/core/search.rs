use super::search_list::SearchList;

pub(super) struct SearchArea {
    pub(super) list: SearchList,
    /// Search debounces typed text; a search panel always searches immediately.
    pub(super) due: bool,
    /// Enter in the search box gives the first row Enter once it arrives.
    pub(super) enter_on_answer: bool,
}

impl SearchArea {
    pub(super) fn new() -> Self {
        Self {
            list: SearchList::new(),
            due: false,
            enter_on_answer: false,
        }
    }
}
