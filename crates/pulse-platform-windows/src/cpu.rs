use std::time::{Duration, Instant};

use pulse_domain::{Availability, LogicalCpu, Unit};
use windows::{
    Win32::System::{
        Performance::*,
        Threading::{GetActiveProcessorCount, GetActiveProcessorGroupCount},
    },
    core::{HSTRING, PCWSTR},
};

use super::{
    disk::{availability, formatted},
    sample,
};

const LOAD: &str = r"PDH / \Processor Information(group,number)\% Processor Time";
const FREQ: &str = r"PDH / \Processor Information(group,number)\Processor Frequency";

struct Entry {
    group: u16,
    number: u32,
    usage: Result<PDH_HCOUNTER, u32>,
    clock: Result<PDH_HCOUNTER, u32>,
}

struct Query {
    handle: PDH_HQUERY,
    entries: Vec<Entry>,
    previous: Option<Instant>,
}
impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the worker owns the query and its child counters.
        unsafe { PdhCloseQuery(self.handle) };
    }
}
impl Query {
    fn open() -> Result<Self, u32> {
        let mut handle = PDH_HQUERY::default();
        // SAFETY: writable output handle; a null source requests live local counters.
        let code = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if code != 0 {
            return Err(code);
        }
        let mut query = Self {
            handle,
            entries: Vec::new(),
            previous: None,
        };
        let add = |group, number, field| {
            let path = HSTRING::from(format!(r"\Processor Information({group},{number})\{field}"));
            let mut out = PDH_HCOUNTER::default();
            // SAFETY: query is live, path is NUL terminated and output writable.
            let code = unsafe { PdhAddEnglishCounterW(handle, &path, 0, &mut out) };
            if code == 0 { Ok(out) } else { Err(code) }
        };
        // SAFETY: no input pointers; group numbers are taken from the ID range reported by Windows.
        for group in 0..unsafe { GetActiveProcessorGroupCount() } {
            for number in 0..unsafe { GetActiveProcessorCount(group) }.min(64) {
                query.entries.push(Entry {
                    group,
                    number,
                    usage: add(group, number, "% Processor Time"),
                    clock: add(group, number, "Processor Frequency"),
                });
            }
        }
        Ok(query)
    }
    fn collect(&mut self, utc: u64, now: Instant) -> (Vec<LogicalCpu>, Availability<()>) {
        if self.entries.is_empty() {
            return (Vec::new(), Availability::Unsupported);
        }
        // SAFETY: PDH owns a live query until Drop.
        let code = unsafe { PdhCollectQueryData(self.handle) };
        let previous = if code == 0 {
            self.previous.replace(now)
        } else {
            self.previous.take()
        };
        let warm = previous.is_some_and(|p| now.duration_since(p) <= Duration::from_secs(15));
        let read = |counter: Result<PDH_HCOUNTER, u32>, rate, validate: fn(f64) -> bool| {
            let Ok(counter) = counter else {
                return availability(counter.unwrap_err());
            };
            if code != 0 {
                return availability(code);
            }
            if rate && !warm {
                return Availability::Pending;
            }
            match formatted(counter) {
                Ok(value) if validate(value) => Availability::Available(value),
                Ok(_) => Availability::Pending,
                Err(code) => availability(code),
            }
        };
        let samples = self
            .entries
            .iter()
            .map(|e| LogicalCpu {
                group: e.group,
                number: e.number,
                usage: sample(
                    utc,
                    Unit::Percent,
                    LOAD,
                    read(e.usage, true, |x| (0. ..=100.).contains(&x)),
                ),
                frequency: sample(
                    utc,
                    Unit::Megahertz,
                    FREQ,
                    read(e.clock, false, |x| x.is_finite() && x > 0.),
                ),
            })
            .collect();
        (
            samples,
            if code == 0 {
                Availability::Available(())
            } else {
                availability(code)
            },
        )
    }
}

#[derive(Default)]
pub struct Cpu {
    query: Option<Query>,
    retry: Option<(Instant, Availability<()>)>,
}

impl Cpu {
    pub fn sample(&mut self, utc: u64) -> (Vec<LogicalCpu>, Availability<()>) {
        let now = Instant::now();
        if let Some((_, status)) = self.retry.filter(|(until, _)| now < *until) {
            return (Vec::new(), status);
        }
        if self.query.is_none() {
            match Query::open() {
                Ok(query) => self.query = Some(query),
                Err(code) => {
                    let status = availability(code);
                    self.retry = Some((now + Duration::from_secs(30), status));
                    return (Vec::new(), status);
                }
            }
        }
        self.query.as_mut().unwrap().collect(utc, now)
    }
}
