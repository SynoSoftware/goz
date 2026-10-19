//! Every decision the window makes: which commands are enabled, where each
//! control goes, what every caption says and what is saved. It holds no
//! Win32 type and makes no Win32 call, so its tests run on every OS and no
//! decision needs a window to be tested (DESIGN.md:
//! Where the unsafe lives). On other systems nothing outside the tests calls
//! it.

#![forbid(unsafe_code)]
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) mod captions;
pub(crate) mod clipboard;
pub(crate) mod commands;
pub(crate) mod fetch;
pub(crate) mod ids;
pub(crate) mod layout;
pub(crate) mod model;
pub(crate) mod open;
pub(crate) mod query;
pub(crate) mod results;
pub(crate) mod selection;
pub(crate) mod settings;
pub(crate) mod shortcuts;
pub(crate) mod text;
pub(crate) mod update;
