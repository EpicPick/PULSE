use crate::wddm::counter_array;
use pulse_domain::{DISK_LIMIT, DriveSample, DriveVolume, StorageSnapshot};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use windows::{
    Win32::System::Performance::*,
    core::{PCWSTR, w},
};
use wmi::{WMIConnection, WMIResult};
mod nvme;

const REFRESH: Duration = Duration::from_secs(15);
const WMI_LIMIT: usize = 512;

#[derive(Default)]
pub struct Storage {
    inventory: Vec<DriveSample>,
    refreshed: Option<Instant>,
    opened: Option<Instant>,
    query: Option<Query>,
    omitted: bool,
    note: Option<String>,
}

impl Storage {
    pub fn sample(&mut self) -> StorageSnapshot {
        let now = Instant::now();
        let origin = *self.opened.get_or_insert(now);
        if self
            .refreshed
            .is_none_or(|at| now.duration_since(at) >= REFRESH)
        {
            let before = self
                .inventory
                .iter()
                .map(|d| (d.index, d.identity.clone()))
                .collect::<Vec<_>>();
            match inventory() {
                Ok((disks, omitted)) => {
                    let changed = before
                        .iter()
                        .map(|(i, id)| (*i, id.as_str()))
                        .ne(disks.iter().map(|d| (d.index, d.identity.as_str())));
                    if changed {
                        self.query = None;
                    }
                    self.inventory = disks;
                    self.omitted = omitted;
                    self.note = None;
                }
                Err(e) => {
                    self.inventory.clear();
                    self.query = None;
                    self.note = Some(format!("Inventaire Windows indisponible : {e}"));
                }
            }
            self.refreshed = Some(now);
        }
        let mut drives = self.inventory.clone();
        if !drives.is_empty() {
            if self.query.is_none() {
                self.query = Query::open();
            }
            if let Some(query) = &mut self.query {
                query.sample(now, &mut drives);
            }
        }
        StorageSnapshot {
            elapsed_ms: origin.elapsed().as_millis() as u64,
            drives,
            omitted: self.omitted,
            note: self.note.clone(),
        }
    }
}

struct Query {
    handle: PDH_HQUERY,
    counters: [Option<PDH_HCOUNTER>; 7],
    aligned: Vec<usize>,
    last: Option<Instant>,
}
impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: this worker exclusively owns the query and its counter handles.
        unsafe {
            PdhCloseQuery(self.handle);
        }
    }
}
impl Query {
    fn open() -> Option<Self> {
        let mut handle = PDH_HQUERY::default();
        // SAFETY: local data source, writable result pointer.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) } != 0 {
            return None;
        }
        let counters = [
            w!(r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
            w!(r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
            w!(r"\PhysicalDisk(*)\% Idle Time"),
            w!(r"\PhysicalDisk(*)\Avg. Disk sec/Read"),
            w!(r"\PhysicalDisk(*)\Avg. Disk sec/Write"),
            w!(r"\PhysicalDisk(*)\Disk Reads/sec"),
            w!(r"\PhysicalDisk(*)\Disk Writes/sec"),
        ]
        .map(|path| {
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: paths are NUL-terminated and the query owns added counters.
            (unsafe { PdhAddEnglishCounterW(handle, path, 0, &mut counter) } == 0)
                .then_some(counter)
        });
        Some(Self {
            handle,
            counters,
            aligned: Vec::new(),
            last: None,
        })
    }
    fn sample(&mut self, now: Instant, drives: &mut [DriveSample]) {
        // SAFETY: this query remains live while sampling.
        if unsafe { PdhCollectQueryData(self.handle) } != 0 {
            self.last = None;
            return;
        }
        if self
            .last
            .replace(now)
            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(5))
        {
            return;
        }
        let mut values = vec![[None; 7]; drives.len()];
        for (i, counter) in self.counters.iter().enumerate() {
            if let Some(counter) = counter {
                let mut seen = [false; DISK_LIMIT];
                let _ = counter_array(*counter, &mut self.aligned, |name, v| {
                    if let Some(id) = counter_index(name)
                        && let Ok(slot) = drives.binary_search_by_key(&id, |d| d.index)
                    {
                        values[slot][i] = (!seen[slot]).then_some(v);
                        seen[slot] = true;
                    }
                });
            }
        }
        for (d, [read, write, idle, r_ms, w_ms, rops, wops]) in drives.iter_mut().zip(values) {
            d.read_bps = read;
            d.write_bps = write;
            d.busy_percent = idle.filter(|v| (0. ..=100.).contains(v)).map(|v| 100. - v);
            d.read_ms = latency(r_ms, rops);
            d.write_ms = latency(w_ms, wops);
        }
    }
}

fn counter_index(name: &str) -> Option<u32> {
    name.split_ascii_whitespace().next()?.parse().ok()
}
fn latency(seconds: Option<f64>, ops: Option<f64>) -> Option<f64> {
    (ops? > 0.)
        .then(|| seconds.map(|v| v * 1000.))
        .flatten()
        .filter(|v| v.is_finite())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Disk {
    index: u32,
    model: Option<String>,
    size: Option<u64>,
    serial_number: Option<String>,
    #[serde(rename = "PNPDeviceID")]
    pnp: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsDisk {
    number: u32,
    bus_type: Option<u16>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Physical {
    device_id: String,
    serial_number: Option<String>,
    health_status: Option<u16>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Reliability {
    device_id: String,
    temperature: Option<u8>,
    wear: Option<u8>,
    power_on_hours: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Volume {
    #[serde(rename = "DeviceID")]
    device_id: String,
    volume_name: Option<String>,
    size: Option<u64>,
    free_space: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Association {
    antecedent: String,
    dependent: String,
}

fn rows<T: serde::de::DeserializeOwned>(
    wmi: &WMIConnection,
    query: &str,
    limit: usize,
) -> WMIResult<Vec<T>> {
    wmi.exec_query(query)?
        .take(limit)
        .map(|row| row.and_then(|r| r.into_desr()))
        .collect()
}
fn inventory() -> WMIResult<(Vec<DriveSample>, bool)> {
    let cim = WMIConnection::new()?;
    let mut disks = rows::<Disk>(
        &cim,
        "SELECT Index,Model,Size,PNPDeviceID,SerialNumber FROM Win32_DiskDrive",
        DISK_LIMIT + 1,
    )?;
    disks.sort_unstable_by_key(|d| d.index);
    let omitted = disks.len() > DISK_LIMIT;
    disks.truncate(DISK_LIMIT);
    let volumes=rows::<Volume>(&cim,"SELECT DeviceID,VolumeName,Size,FreeSpace FROM Win32_LogicalDisk WHERE DriveType=2 OR DriveType=3",WMI_LIMIT).unwrap_or_default();
    let associations = rows::<Association>(
        &cim,
        "SELECT Antecedent,Dependent FROM Win32_LogicalDiskToPartition",
        WMI_LIMIT,
    )
    .unwrap_or_default();
    let mut volume_disks: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for pair in associations {
        if let Some((disk, letter)) = association(&pair.antecedent, &pair.dependent) {
            volume_disks.entry(letter.into()).or_default().insert(disk);
        }
    }
    let (buses,physical,reliability)=WMIConnection::with_namespace_path(r"ROOT\Microsoft\Windows\Storage").ok().map_or_else(
        ||(vec![],vec![],vec![]),
        |conn|(
            rows::<WindowsDisk>(&conn,"SELECT Number,BusType FROM MSFT_Disk",WMI_LIMIT).unwrap_or_default(),
            rows::<Physical>(&conn,"SELECT DeviceId,SerialNumber,HealthStatus FROM MSFT_PhysicalDisk",WMI_LIMIT).unwrap_or_default(),
            rows::<Reliability>(&conn,"SELECT DeviceId,Temperature,Wear,PowerOnHours FROM MSFT_StorageReliabilityCounter",WMI_LIMIT).unwrap_or_default(),
        ));
    let mut samples = Vec::new();
    for disk in &disks {
        let serial = disk
            .serial_number
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        // Storage Management device IDs are not the Win32 disk number. Join only on a unique serial.
        let health = serial
            .filter(|s| {
                disks
                    .iter()
                    .filter(|d| d.serial_number.as_deref().map(str::trim) == Some(*s))
                    .count()
                    == 1
            })
            .and_then(|s| {
                let mut matched = physical
                    .iter()
                    .filter(|p| p.serial_number.as_deref().map(str::trim) == Some(s));
                let first = matched.next()?;
                matched.next().is_none().then_some(first)
            });
        let stats = health.and_then(|p| {
            let mut matches = reliability.iter().filter(|r| r.device_id == p.device_id);
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        });
        let linked = volumes
            .iter()
            .filter_map(|v| {
                let ids = volume_disks.get(&v.device_id)?;
                ids.contains(&disk.index).then(|| DriveVolume {
                    letter: v.device_id.clone(),
                    name: v
                        .volume_name
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect(),
                    bytes: v.size,
                    free_bytes: v.free_space,
                    spans_multiple_disks: ids.len() > 1,
                })
            })
            .collect();
        let mut sample = DriveSample {
            index: disk.index,
            identity: disk
                .pnp
                .clone()
                .filter(|v| !v.is_empty())
                .or_else(|| serial.map(str::to_owned))
                .unwrap_or_default(),
            model: disk
                .model
                .as_deref()
                .unwrap_or("Disque")
                .trim()
                .chars()
                .take(100)
                .collect(),
            bus: bus(buses
                .iter()
                .find(|b| b.number == disk.index)
                .and_then(|b| b.bus_type))
            .into(),
            bytes: disk.size,
            volumes: linked,
            health: health.and_then(|p| p.health_status),
            temperature: stats
                .and_then(|x| x.temperature)
                .filter(|t| (1..=150).contains(t)),
            wear: stats.and_then(|x| x.wear).filter(|w| *w <= 100),
            power_on_hours: stats.and_then(|x| x.power_on_hours),
            ..Default::default()
        };
        if sample.bus == "NVMe" {
            nvme::read(&mut sample);
        }
        samples.push(sample);
    }
    Ok((samples, omitted))
}
fn key(path: &str) -> Option<&str> {
    Some(path.split_once('"')?.1.split_once('"')?.0)
}
fn association<'a>(disk: &str, volume: &'a str) -> Option<(u32, &'a str)> {
    if !disk.contains("Win32_DiskPartition") || !volume.contains("Win32_LogicalDisk") {
        return None;
    }
    let disk = key(disk)?
        .strip_prefix("Disk #")?
        .split_once(", Partition #")?
        .0
        .parse()
        .ok()?;
    let letter = key(volume)?;
    let bytes = letter.as_bytes();
    (bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        .then_some((disk, letter))
}
fn bus(x: Option<u16>) -> &'static str {
    match x {
        Some(1) => "SCSI",
        Some(3) => "ATA",
        Some(7) => "USB",
        Some(8) => "RAID",
        Some(10) => "SAS",
        Some(11) => "SATA",
        Some(12) => "SD",
        Some(14 | 15) => "Virtuel",
        Some(16) => "Storage Spaces",
        Some(17) => "NVMe",
        Some(19) => "UFS",
        _ => "Type non exposé",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queries_real_inventory_without_opening_a_window() {
        let mut collector = Storage::default();
        let first = collector.sample();
        assert!(first.drives.iter().all(|d| d.read_bps.is_none()));
        assert!(first.drives.windows(2).all(|d| d[0].index < d[1].index));
        eprintln!(
            "{:?}",
            first
                .drives
                .iter()
                .map(|d| (&d.model, &d.bus, d.volumes.len(), d.health, d.temperature))
                .collect::<Vec<_>>()
        );
        assert!(first.note.is_none(), "{:?}", first.note);
        std::thread::sleep(Duration::from_millis(180));
        let next = collector.sample();
        eprintln!(
            "Relevés: {:?}",
            next.drives
                .iter()
                .map(|d| (
                    d.index,
                    d.read_bps,
                    d.write_bps,
                    d.busy_percent,
                    d.nvme_smart,
                    d.temperature,
                    d.written_bytes
                ))
                .collect::<Vec<_>>()
        );
        for d in &next.drives {
            assert!(d.busy_percent.is_none_or(|v| (0. ..=100.).contains(&v)));
        }
    }
    #[test]
    fn parses_real_counter_and_association_shapes() {
        assert_eq!(counter_index("0 D:"), Some(0));
        assert_eq!(counter_index("_Total"), None);
        assert_eq!(
            association(
                r#"\\HOST\root\cimv2:Win32_DiskPartition.DeviceID="Disk #2, Partition #1""#,
                r#"\\HOST\root\cimv2:Win32_LogicalDisk.DeviceID="C:""#
            ),
            Some((2, "C:"))
        );
        assert_eq!(latency(Some(0.01), Some(0.)), None);
    }
}
