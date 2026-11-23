use std::{
    collections::HashMap,
    ptr,
    time::{Duration, Instant},
};

use pulse_domain::Availability;
use windows::{
    Win32::{
        Foundation::{E_POINTER, ERROR_ACCESS_DENIED, NO_ERROR},
        NetworkManagement::{
            IpHelper::{
                FreeMibTable, GetIfTable2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, MIB_IF_ROW2,
                MIB_IF_TABLE2,
            },
            Ndis::IfOperStatusUp,
        },
    },
    core::{GUID, HRESULT},
};

pub const SOURCE: &str = "IP Helper / GetIfTable2 (Ethernet + Wi-Fi actifs)";
type Counters = HashMap<(GUID, u64), (u64, u64)>;

#[derive(Default)]
pub struct Network {
    last: Option<Instant>,
    previous: Counters,
    current: Counters,
}

struct Table(*mut MIB_IF_TABLE2);
impl Drop for Table {
    fn drop(&mut self) {
        // SAFETY: this allocation came from GetIfTable2 and has a single owner.
        unsafe { FreeMibTable(self.0.cast()) }
    }
}

impl Network {
    pub fn sample(&mut self) -> [Availability<f64>; 3] {
        let mut table = ptr::null_mut();
        // SAFETY: writable output pointer. On success, the table includes NumEntries rows.
        let status = unsafe { GetIfTable2(&mut table) };
        if status != NO_ERROR || table.is_null() {
            self.last = None;
            self.previous.clear();
            let failure = if status == ERROR_ACCESS_DENIED {
                Availability::PermissionRequired
            } else if status == NO_ERROR {
                Availability::Error(E_POINTER.0)
            } else {
                Availability::Error(HRESULT::from_win32(status.0).0)
            };
            return [failure; 3];
        }
        let now = Instant::now();
        let table = Table(table);
        // SAFETY: use the offset of the ABI's variable-length trailing array, not a Rust
        // reference to the one-element declaration; the OS allocation stays live here.
        let rows = unsafe {
            let count = (*table.0).NumEntries as usize;
            let start = table
                .0
                .cast::<u8>()
                .add(std::mem::offset_of!(MIB_IF_TABLE2, Table))
                .cast::<MIB_IF_ROW2>();
            std::slice::from_raw_parts(start, count)
        };
        self.current.clear();
        for row in rows {
            if row.OperStatus == IfOperStatusUp && matches!(row.Type, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211)
                // HardwareInterface bit from the documented InterfaceAndOperStatusFlags layout.
                && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
            {
                // SAFETY: Value is the integral representation of the NET_LUID union.
                let luid = unsafe { row.InterfaceLuid.Value };
                self.current
                    .insert((row.InterfaceGuid, luid), (row.InOctets, row.OutOctets));
            }
        }
        let result = if self.current.is_empty() {
            [Availability::Unsupported; 3]
        } else {
            self.last
                .and_then(|last| rate(&self.previous, &self.current, now.duration_since(last)))
                .map_or([Availability::Pending; 3], |v| {
                    v.map(Availability::Available)
                })
        };
        self.last = Some(now);
        std::mem::swap(&mut self.previous, &mut self.current);
        result
    }
}

fn rate(previous: &Counters, current: &Counters, elapsed: Duration) -> Option<[f64; 3]> {
    if elapsed.is_zero()
        || elapsed > Duration::from_secs(15)
        || previous.len() != current.len()
        || current.is_empty()
    {
        return None;
    }
    let (mut rx, mut tx) = (0_u128, 0_u128);
    for (id, &(input, output)) in current {
        let &(old_in, old_out) = previous.get(id)?;
        rx += u128::from(input.checked_sub(old_in)?);
        tx += u128::from(output.checked_sub(old_out)?);
    }
    Some([rx + tx, rx, tx].map(|v| v as f64 / elapsed.as_secs_f64()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reports_complete_intervals() {
        let id = (GUID::zeroed(), 42);
        let initial = Counters::from([(id, (100, 200))]);
        let changed = Counters::from([(id, (200, 300))]);
        let elapsed = Duration::from_secs(2);
        assert_eq!(rate(&initial, &changed, elapsed), Some([100., 50., 50.]));
        assert_eq!(rate(&initial, &initial, elapsed), Some([0.; 3]));
        assert_eq!(rate(&changed, &initial, elapsed), None);
        assert_eq!(rate(&initial, &changed, Duration::from_secs(20)), None);
        assert_eq!(
            rate(
                &initial,
                &Counters::from([((GUID::zeroed(), 43), (200, 300))]),
                elapsed
            ),
            None
        );
    }
}
