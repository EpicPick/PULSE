use super::{disk::availability, sample};
use pulse_domain::{Availability, GpuKey, GpuProcess, GpuProcesses, Process, ProcessKey, Unit};
use std::{
    collections::HashMap,
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Win32::System::Performance::*,
    core::{PCWSTR, w},
};

const PATHS: [PCWSTR; 3] = [
    w!(r"\GPU Engine(*)\Utilization Percentage"),
    w!(r"\GPU Process Memory(*)\Dedicated Usage"),
    w!(r"\GPU Process Memory(*)\Shared Usage"),
];
const SOURCE: [&str; 3] = [
    "WDDM · PDH GPU Engine / Utilization Percentage",
    "WDDM · PDH GPU Process Memory / Dedicated Usage",
    "WDDM · PDH GPU Process Memory / Shared Usage",
];
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 2048;

struct Query {
    handle: PDH_HQUERY,
    counters: [Result<PDH_HCOUNTER, u32>; 3],
    aligned: Vec<usize>,
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this worker is the sole owner of the query and its counters.
        unsafe {
            PdhCloseQuery(self.handle);
        }
    }
}

impl Query {
    fn open() -> Result<Self, u32> {
        let mut handle = PDH_HQUERY::default();
        // SAFETY: writable out handle and a null data source for local live sampling.
        let code = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if code != 0 {
            return Err(code);
        }
        let mut query = Self {
            handle,
            counters: [Err(PDH_CSTATUS_NO_COUNTER); 3],
            aligned: Vec::new(),
        };
        for (slot, path) in query.counters.iter_mut().zip(PATHS) {
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: the query is live and path is a constant NUL-terminated UTF-16 string.
            let code = unsafe { PdhAddEnglishCounterW(handle, path, 0, &mut counter) };
            *slot = if code == 0 { Ok(counter) } else { Err(code) };
        }
        Ok(query)
    }

    fn scan(&mut self, index: usize, mut use_value: impl FnMut(&str, f64)) -> Result<(), u32> {
        let counter = self.counters[index]?;
        counter_array(counter, &mut self.aligned, |name, value| {
            if index != 0 || value <= 100. {
                use_value(name, value);
            }
        })
    }
}

pub(crate) fn counter_array(
    counter: PDH_HCOUNTER,
    aligned: &mut Vec<usize>,
    mut use_value: impl FnMut(&str, f64),
) -> Result<(), u32> {
    for _ in 0..3 {
        let capacity = aligned.len() * size_of::<usize>();
        let mut bytes = capacity as u32;
        let mut count = 0;
        let data = aligned.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // SAFETY: the buffer has native pointer alignment; null requests sizing.
        let code = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                (capacity > 0).then_some(data),
            )
        };
        if code == PDH_MORE_DATA {
            if !(1..=MAX_BYTES).contains(&(bytes as usize)) {
                return Err(code);
            }
            aligned.resize((bytes as usize).div_ceil(size_of::<usize>()), 0);
            continue;
        }
        if code != 0 {
            return Err(code);
        }
        if count == 0 {
            return Ok(());
        }
        if count as usize > capacity / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() {
            return Err(PDH_INVALID_DATA);
        }
        let base = data as usize;
        let max_addr = base + capacity;
        // SAFETY: successful API return and the item array fits in the initialized buffer.
        for item in unsafe { std::slice::from_raw_parts(data, count as usize) } {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let address = item.szName.0 as usize;
            if address < base || address >= max_addr || !address.is_multiple_of(2) {
                continue;
            }
            let start = (address - base) / 2;
            // SAFETY: use the allocation-derived base, with offset bounds checked above.
            let name = unsafe {
                std::slice::from_raw_parts(
                    data.cast::<u16>().add(start),
                    ((max_addr - address) / 2).min(256),
                )
            };
            let Some(len) = name.iter().position(|&v| v == 0) else {
                continue;
            };
            // SAFETY: queried as PDH_FMT_DOUBLE and status denotes a valid reading.
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            if value.is_finite() && value >= 0. {
                use_value(&String::from_utf16_lossy(&name[..len]), value);
            }
        }
        return Ok(());
    }
    Err(PDH_MORE_DATA)
}

fn instance(name: &str) -> Option<(u32, GpuKey, Option<&str>)> {
    let mut parts = name.split('_');
    if parts.next()? != "pid" {
        return None;
    }
    let pid = parts.next()?.parse().ok()?;
    if parts.next()? != "luid" {
        return None;
    }
    let mut hex = || u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok();
    let (luid_high, luid_low) = (hex()?, hex()?);
    if parts.next()? != "phys" {
        return None;
    }
    let physical = parts.next()?.split('#').next()?.parse().ok()?;
    let engine = name
        .split_once("_engtype_")
        .map(|(_, v)| v.split('#').next().unwrap_or(v))
        .filter(|v| !v.is_empty());
    Some((
        pid,
        GpuKey {
            luid_high,
            luid_low,
            physical,
        },
        engine,
    ))
}

#[derive(Default)]
struct Values {
    data: [Option<f64>; 3],
    engine: Option<String>,
}

#[derive(Default)]
pub struct Wddm {
    query: Option<Query>,
    cached: Option<(Instant, Arc<GpuProcesses>)>,
    retry: Option<(Instant, u32)>,
    previous: HashMap<u32, ProcessKey>,
    last_good: Option<Instant>,
}

impl Wddm {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn collect(&mut self, t: u64, processes: &[Process]) -> Arc<GpuProcesses> {
        let now = Instant::now();
        if let Some((at, data)) = &self.cached
            && now.duration_since(*at) < Duration::from_secs(2)
        {
            return data.clone();
        }
        if let Some((until, code)) = self.retry
            && now < until
        {
            return Self::empty(t, availability(code));
        }
        if self.query.is_none() {
            match Query::open() {
                Ok(query) => self.query = Some(query),
                Err(code) => {
                    self.retry = Some((now + Duration::from_secs(30), code));
                    return Self::empty(t, availability(code));
                }
            }
        }
        let query = self.query.as_mut().unwrap();
        // SAFETY: the query remains live throughout collection and reading.
        let code = unsafe { PdhCollectQueryData(query.handle) };
        let last = self.last_good.take();
        let current: HashMap<_, _> = processes.iter().map(|p| (p.key.pid, p)).collect();
        if code != 0 {
            self.previous.clear();
            return Self::empty(t, availability(code));
        }
        self.last_good = Some(now);
        let warm = last.is_some_and(|old| now.duration_since(old) <= Duration::from_secs(10));
        let belongs = |pid| {
            current
                .get(&pid)
                .filter(|p| self.previous.get(&pid) == Some(&p.key))
        };
        let mut map = HashMap::<(u32, GpuKey), Values>::new();
        let mut excluded = 0;
        let status = std::array::from_fn(|i| {
            if !warm {
                return query.counters[i].map_or_else(availability, |_| Availability::Pending);
            }
            match query.scan(i, |name, value| {
                let Some((pid, adapter, engine)) = instance(name) else {
                    return;
                };
                if (i == 0 && engine.is_none()) || belongs(pid).is_none() || value == 0. {
                    return;
                }
                if map.len() >= MAX_ROWS && !map.contains_key(&(pid, adapter)) {
                    excluded += 1;
                    return;
                }
                let entry = map.entry((pid, adapter)).or_default();
                // A busy process can use several engines at the same time. Never add percentages.
                if entry.data[i].is_none_or(|old| value > old) {
                    entry.data[i] = Some(value);
                    if i == 0 {
                        entry.engine = engine.map(str::to_owned);
                    }
                }
            }) {
                Ok(()) => Availability::Available(()),
                Err(code) => availability(code),
            }
        });
        let reading = |i: usize, unit, value| {
            let availability = match (value, status[i]) {
                (Some(v), _) => Availability::Available(v),
                (None, Availability::PermissionRequired) => Availability::PermissionRequired,
                (None, Availability::Unsupported) => Availability::Unsupported,
                (None, Availability::Error(e)) => Availability::Error(e),
                _ => Availability::Pending,
            };
            sample(t, unit, SOURCE[i], availability)
        };
        let mut rows = map
            .into_iter()
            .filter_map(|((pid, adapter), values)| {
                let p = current.get(&pid)?;
                Some(GpuProcess {
                    process: p.key,
                    name: p.name.clone(),
                    adapter,
                    busiest_engine: values.engine,
                    activity: reading(0, Unit::Percent, values.data[0]),
                    dedicated: reading(1, Unit::Bytes, values.data[1]),
                    shared: reading(2, Unit::Bytes, values.data[2]),
                })
            })
            .collect::<Vec<_>>();
        rows.sort_unstable_by(|a, b| {
            b.activity
                .value
                .get()
                .unwrap_or(-1.)
                .total_cmp(&a.activity.value.get().unwrap_or(-1.))
                .then_with(|| {
                    b.dedicated
                        .value
                        .get()
                        .unwrap_or(-1.)
                        .total_cmp(&a.dedicated.value.get().unwrap_or(-1.))
                })
                .then_with(|| a.process.cmp(&b.process))
                .then_with(|| a.adapter.cmp(&b.adapter))
        });
        self.previous.clear();
        self.previous
            .extend(processes.iter().map(|p| (p.key.pid, p.key)));
        let data = Arc::new(GpuProcesses {
            utc_ms: t,
            rows,
            status,
            excluded,
        });
        self.cached = Some((now, data.clone()));
        data
    }
    fn empty(t: u64, status: Availability<()>) -> Arc<GpuProcesses> {
        Arc::new(GpuProcesses {
            utc_ms: t,
            rows: Vec::new(),
            status: [status; 3],
            excluded: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_structured_gpu_instance_keys() {
        let (_, key, engine) =
            instance("pid_1752_luid_0x00000000_0x0000ccaf_phys_0_eng_3_engtype_VideoDecode#1")
                .unwrap();
        assert_eq!(
            key,
            GpuKey {
                luid_high: 0,
                luid_low: 0xccaf,
                physical: 0
            }
        );
        assert_eq!(engine, Some("VideoDecode"));
        assert!(instance("pid_1752_phys_0").is_none());
        assert!(instance("pid_1_luid_0xFFFFFFFF_0x00000001_phys_2").is_some());
    }

    #[test]
    fn samples_the_real_pdh_gpu_tables_without_opening_a_window() {
        let mut collector = crate::Collector::default();
        let first = collector.sample_with_details(false, true);
        let first = first.gpu_processes.as_ref().unwrap();
        assert!(first.rows.is_empty());
        std::thread::sleep(Duration::from_millis(2050));
        let second = collector.sample_with_details(false, true);
        let data = second.gpu_processes.as_ref().unwrap();
        eprintln!(
            "WDDM: {:?}; {} processus/adaptateurs",
            data.status,
            data.rows.len()
        );
        assert!(
            data.rows
                .iter()
                .all(|row| second.processes.iter().any(|p| p.key == row.process))
        );
        assert!(data.rows.iter().all(|row| {
            row.activity
                .value
                .get()
                .is_none_or(|v| (0. ..=100.).contains(&v))
        }));
    }
}
