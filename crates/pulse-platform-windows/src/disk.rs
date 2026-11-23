use std::time::{Duration, Instant};

use pulse_domain::Availability;
use windows::{
    Win32::{Foundation::ERROR_ACCESS_DENIED, System::Performance::*},
    core::{HRESULT, PCWSTR, w},
};

pub const SOURCE: &str = r"PDH / \PhysicalDisk(_Total)\Disk Bytes/sec";
pub const READ_LATENCY: &str = r"PDH / \PhysicalDisk(_Total)\Avg. Disk sec/Read";
pub const WRITE_LATENCY: &str = r"PDH / \PhysicalDisk(_Total)\Avg. Disk sec/Write";

#[derive(Default)]
pub struct Disk {
    query: Option<Query>,
    retry_at: Option<Instant>,
    failure: Option<Availability<f64>>,
}

struct Query {
    handle: PDH_HQUERY,
    counters: [Result<PDH_HCOUNTER, u32>; 7],
    previous: Option<Instant>,
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this query is owned by this thread; PDH also frees its child counter.
        unsafe { PdhCloseQuery(self.handle) };
    }
}

impl Query {
    fn open() -> Result<Self, u32> {
        let mut handle = PDH_HQUERY::default();
        // SAFETY: null data source selects live counters; the output handle is writable.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if status != 0 {
            return Err(status);
        }
        let mut query = Self {
            handle,
            counters: [Err(PDH_CSTATUS_NO_COUNTER); 7],
            previous: None,
        };
        // SAFETY: path is a static, NUL-terminated UTF-16 string and the query is live.
        for (slot, path) in query.counters.iter_mut().zip([
            w!(r"\PhysicalDisk(_Total)\Disk Bytes/sec"),
            w!(r"\PhysicalDisk(_Total)\Avg. Disk sec/Read"),
            w!(r"\PhysicalDisk(_Total)\Avg. Disk sec/Write"),
            w!(r"\PhysicalDisk(_Total)\Disk Reads/sec"),
            w!(r"\PhysicalDisk(_Total)\Disk Writes/sec"),
            w!(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"),
            w!(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"),
        ]) {
            let mut counter = PDH_HCOUNTER::default();
            let status = unsafe { PdhAddEnglishCounterW(handle, path, 0, &mut counter) };
            *slot = if status == 0 {
                Ok(counter)
            } else {
                Err(status)
            };
        }
        Ok(query)
    }

    fn read(&mut self, now: Instant) -> Result<[Availability<f64>; 5], u32> {
        // SAFETY: PDH owns a live query and its counter until Drop.
        let status = unsafe { PdhCollectQueryData(self.handle) };
        if status != 0 {
            return Err(status);
        }
        let prev = self.previous.replace(now);
        if prev.is_none_or(|last| now.duration_since(last) > Duration::from_secs(15)) {
            return Ok([Availability::Pending; 5]);
        }
        let get = |i: usize| match self.counters[i].and_then(formatted) {
            Ok(v) if v.is_finite() && v >= 0. => Availability::Available(v),
            Ok(_) => Availability::Pending,
            Err(e) => availability(e),
        };
        Ok([
            get(0),
            latency(get(1), get(3)),
            latency(get(2), get(4)),
            get(5),
            get(6),
        ])
    }
}

fn latency(seconds: Availability<f64>, operations: Availability<f64>) -> Availability<f64> {
    match operations {
        Availability::Available(v) if v > 0. => match seconds {
            Availability::Available(s) if (s * 1000.).is_finite() => {
                Availability::Available(s * 1000.)
            }
            Availability::Available(_) => Availability::Pending,
            missing => missing,
        },
        Availability::Available(_) => Availability::Pending,
        missing => missing,
    }
}

pub(crate) fn formatted(counter: PDH_HCOUNTER) -> Result<f64, u32> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: output is writable; PDH_FMT_DOUBLE selects the matching union arm below.
    let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut value) };
    if status != 0 {
        return Err(status);
    }
    if !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
        return Err(value.CStatus);
    }
    Ok(unsafe { value.Anonymous.doubleValue })
}

pub(crate) fn availability<T>(status: u32) -> Availability<T> {
    match status {
        PDH_CSTATUS_NO_COUNTER | PDH_CSTATUS_NO_OBJECT | PDH_CSTATUS_NO_INSTANCE => {
            Availability::Unsupported
        }
        PDH_NO_DATA | PDH_INVALID_DATA | PDH_CSTATUS_INVALID_DATA => Availability::Pending,
        status
            if status == HRESULT::from_win32(ERROR_ACCESS_DENIED.0).0 as u32
                || status == ERROR_ACCESS_DENIED.0 =>
        {
            Availability::PermissionRequired
        }
        status => Availability::Error(status as i32),
    }
}

impl Disk {
    pub fn sample(&mut self) -> [Availability<f64>; 5] {
        let now = Instant::now();
        if let Some(query) = &mut self.query {
            return match query.read(now) {
                Ok(value) => value,
                Err(status) => {
                    query.previous = None;
                    [availability(status); 5]
                }
            };
        }
        if self.retry_at.is_some_and(|deadline| deadline > now) {
            return [self.failure.unwrap_or(Availability::Pending); 5];
        }
        match Query::open() {
            Ok(mut query) => {
                let first = query
                    .read(now)
                    .unwrap_or_else(|status| [availability(status); 5]);
                self.query = Some(query);
                first
            }
            Err(status) => {
                let failure = availability(status);
                self.failure = Some(failure);
                self.retry_at = Some(now + Duration::from_secs(30));
                [failure; 5]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_failure_is_not_a_measurement() {
        assert_eq!(
            availability::<f64>(PDH_CSTATUS_NO_OBJECT),
            Availability::Unsupported
        );
        assert_eq!(availability::<f64>(PDH_INVALID_DATA), Availability::Pending);
        assert_eq!(availability::<f64>(5), Availability::PermissionRequired);
    }

    #[test]
    fn disk_latency_has_no_value_without_completed_operations() {
        assert_eq!(
            latency(Availability::Available(0.012), Availability::Available(6.)),
            Availability::Available(12.)
        );
        assert_eq!(
            latency(Availability::Available(0.), Availability::Available(0.)),
            Availability::Pending
        );
    }
}
