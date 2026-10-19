//! The resource and control IDs of `resources/ids.h`, which build.rs turns
//! into the enums below: [`IconId`], [`DialogId`] and [`ControlId`].

include!(concat!(env!("OUT_DIR"), "/ids.rs"));

impl ControlId {
    /// The control whose ID is `id`.
    pub(crate) fn from_id(id: u16) -> Option<ControlId> {
        ControlId::ALL
            .into_iter()
            .find(|control| control.id() == id)
    }
}
