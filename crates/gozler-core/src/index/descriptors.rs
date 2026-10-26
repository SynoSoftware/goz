//! Folder security descriptors, each distinct one kept once per index.
//!
//! A folder's descriptor decides which users may list it, and so who sees
//! the entries in it. Volumes have few distinct descriptors, because a folder
//! usually inherits its parent's permissions, so the entry table keeps a
//! 4-byte [`DescriptorId`] per folder and this table keeps the bytes. The
//! index compares descriptors only as bytes; it never parses one.

use core::num::NonZeroU32;

use super::store::{ComponentBytes, hash_bytes};

/// One distinct descriptor of one index instance. IDs start at 1 and are
/// only ever added while the instance lives, so an ID names the same bytes
/// for the instance's whole life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DescriptorId(NonZeroU32);

// The descriptor column takes 4 bytes an entry, `None` included.
const _: () = assert!(size_of::<Option<DescriptorId>>() == 4);

impl DescriptorId {
    /// The ID's place in the table's and the answers' vectors.
    pub(crate) fn slot(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// What one descriptor costs the table besides its bytes: its box and its
/// lookup bucket.
const PER_ID: u64 = (size_of::<Box<[u8]>>() + size_of::<DescriptorId>() + 1) as u64;

/// How many bytes the table may grow past twice what its last
/// [`DescriptorTable::collect`] kept before it asks for the next one.
const SLACK: u64 = 1 << 20;

/// Every distinct descriptor of one index, by [`DescriptorId`].
#[derive(Debug, Default)]
pub(crate) struct DescriptorTable {
    /// The bytes of each ID, at [`DescriptorId::slot`].
    bytes: Vec<Box<[u8]>>,
    /// The IDs, hashed by their bytes, so equal bytes always get the same ID.
    by_bytes: hashbrown::HashTable<DescriptorId>,
    /// What every ID costs: its bytes plus [`PER_ID`].
    held: u64,
    /// `held` right after the last [`Self::collect`]: what the folders used
    /// then.
    kept: u64,
}

impl DescriptorTable {
    /// The ID of `descriptor`, added when the table does not hold it yet.
    pub(crate) fn id_of(&mut self, descriptor: &[u8]) -> DescriptorId {
        let hash = hash_bytes(descriptor);
        if let Some(&id) = self
            .by_bytes
            .find(hash, |&id| &*self.bytes[id.slot()] == descriptor)
        {
            return id;
        }
        self.bytes.push(descriptor.into());
        self.held += descriptor.len() as u64 + PER_ID;
        let id = u32::try_from(self.bytes.len())
            .ok()
            .and_then(NonZeroU32::new)
            .map(DescriptorId)
            .expect("a volume has fewer than 2^32 distinct descriptors");
        let bytes = &self.bytes;
        self.by_bytes
            .insert_unique(hash, id, |&i| hash_bytes(&bytes[i.slot()]));
        id
    }

    /// Whether the table has grown enough since the last [`Self::collect`]
    /// that the descriptors no folder uses any more may be worth dropping.
    /// The threshold doubles with what a collection keeps, so a volume that
    /// only gains descriptors pays for few collections.
    pub(crate) fn wants_collection(&self) -> bool {
        self.held > 2 * self.kept + SLACK
    }

    /// Keeps only the descriptors that `column` names, under new IDs, and
    /// rewrites `column` to them. Every ID given out before changes meaning,
    /// so the caller must make sure that nothing keeps one.
    pub(crate) fn collect(&mut self, column: &mut [Option<DescriptorId>]) {
        let mut kept = DescriptorTable::default();
        let mut new_ids: Vec<Option<DescriptorId>> = vec![None; self.bytes.len()];
        for id in column.iter_mut().flatten() {
            let slot = id.slot();
            *id = *new_ids[slot].get_or_insert_with(|| kept.id_of(&self.bytes[slot]));
        }
        kept.kept = kept.held;
        *self = kept;
    }

    /// The bytes of `id`, which this table gave out.
    #[cfg(test)]
    pub(crate) fn bytes(&self, id: DescriptorId) -> &[u8] {
        &self.bytes[id.slot()]
    }

    pub(crate) fn memory(&self) -> ComponentBytes {
        let held: u64 = self.bytes.iter().map(|b| b.len() as u64).sum();
        ComponentBytes::of_vec(&self.bytes)
            + ComponentBytes {
                used: held,
                allocated: held,
            }
            + ComponentBytes::of_hash_table::<DescriptorId>(
                self.by_bytes.len(),
                self.by_bytes.capacity(),
            )
    }
}
