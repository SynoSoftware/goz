//! Network and file hashing used by the work owner.

mod work;

pub(crate) use work::{latest_release, read, sha256};
