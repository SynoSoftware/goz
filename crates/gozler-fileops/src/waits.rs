//! Physical-device admission and explicit ordering. Ready work gets every
//! device together or none; waiting work reserves nothing.

use std::collections::HashMap;

use crate::operation::{OperationId, Wait};
use crate::settings::TransferLimit;
use crate::volumes::Disk;

#[derive(Debug, Default, Clone)]
pub(crate) struct Plan {
    pub(crate) disks: Vec<Disk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TransferId {
    pub(crate) operation: OperationId,
    pub(crate) index: usize,
}

impl Plan {
    pub(crate) fn new(mut disks: Vec<Disk>) -> Self {
        disks.sort_unstable();
        disks.dedup();
        Self { disks }
    }
}

pub(crate) fn wait(later: &Plan, earlier: &Plan) -> Option<Wait> {
    later
        .disks
        .iter()
        .any(|disk| earlier.disks.contains(disk))
        .then_some(Wait::Disk)
}

pub(crate) fn waits<'a>(
    running: &[(TransferId, &'a Plan)],
    waiting: &[(TransferId, &'a Plan)],
    limit: TransferLimit,
) -> Vec<Option<(TransferId, Wait)>> {
    let mut use_of: HashMap<&'a Disk, (u32, TransferId)> = HashMap::new();
    for &(id, plan) in running {
        reserve(&mut use_of, id, plan);
    }
    waiting
        .iter()
        .map(|&(id, plan)| {
            let blocker = plan.disks.iter().find_map(|disk| {
                use_of.get(disk).and_then(|&(count, owner)| {
                    (count >= limit.get()).then_some((owner, Wait::Disk))
                })
            });
            if blocker.is_none() {
                reserve(&mut use_of, id, plan);
            }
            blocker
        })
        .collect()
}

fn reserve<'a>(use_of: &mut HashMap<&'a Disk, (u32, TransferId)>, id: TransferId, plan: &'a Plan) {
    for disk in &plan.disks {
        let (count, _) = use_of.entry(disk).or_insert((0, id));
        *count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(operation: u64) -> TransferId {
        TransferId {
            operation: OperationId(operation),
            index: 0,
        }
    }

    fn on(disks: &[u32]) -> Plan {
        Plan::new(disks.iter().copied().map(Disk::Number).collect())
    }

    #[test]
    fn waiting_copies_do_not_reserve_their_devices() {
        let running = on(&[1, 2]);
        let blocked = on(&[1, 3]);
        let independent = on(&[3, 4]);
        assert_eq!(
            waits(
                &[(id(1), &running)],
                &[(id(2), &blocked), (id(3), &independent)],
                TransferLimit::default(),
            ),
            vec![Some((id(1), Wait::Disk)), None],
        );
    }

    #[test]
    fn admission_counts_each_device_once_and_grants_all_or_none() {
        let running = on(&[1, 1, 2]);
        let first = on(&[2, 3]);
        let second = on(&[2, 4]);
        let independent = on(&[4, 5]);
        assert_eq!(
            waits(
                &[(id(1), &running)],
                &[(id(2), &first), (id(3), &second), (id(4), &independent),],
                TransferLimit::try_from(2).unwrap(),
            ),
            vec![None, Some((id(1), Wait::Disk)), None],
        );
    }
}
