//! The resource and control IDs of `resources/ids.h`, which build.rs turns
//! into the enums below: [`IconId`], [`DialogId`] and [`ControlId`].

include!(concat!(env!("OUT_DIR"), "/ids.rs"));
