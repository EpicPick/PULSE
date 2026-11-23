#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessKey {
    pub pid: u32,
    pub created_at_ticks: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Availability<T> {
    Available(T),
    Unsupported,
    PermissionRequired,
    #[default]
    Pending,
    Error(i32),
}

impl<T: Copy> Availability<T> {
    pub fn get(self) -> Option<T> {
        match self {
            Self::Available(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Bytes,
    Celsius,
    Watts,
    Megahertz,
    Milliseconds,
    BytesPerSecond,
    Count,
    Path,
    PriorityClass,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample<T> {
    pub utc_ms: u64,
    pub unit: Unit,
    pub source: &'static str,
    pub value: Availability<T>,
}

#[derive(Debug)]
pub struct Process {
    pub key: ProcessKey,
    pub name: String,
    pub parent_pid: u32,
    pub image_path: Sample<std::sync::Arc<str>>,
    pub icon: Option<std::sync::Arc<[u8]>>,
    pub cpu: Sample<f64>,
    pub working_set: Sample<u64>,
    pub private_bytes: Sample<u64>,
    pub threads: Sample<u32>,
    pub handles: Sample<u32>,
    pub io_read: Sample<f64>,
    pub io_write: Sample<f64>,
    pub priority: Sample<Priority>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Idle,
    Low,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Très basse",
            Self::Low => "Basse",
            Self::Normal => "Normale",
            Self::AboveNormal => "Au-dessus de la normale",
            Self::High => "Haute",
            Self::Realtime => "Temps réel",
        }
    }
}

#[derive(Debug)]
pub struct Snapshot {
    pub utc_ms: u64,
    pub elapsed_ms: u64,
    pub cpu: Sample<f64>,
    pub logical_cpus: Vec<LogicalCpu>,
    pub logical_cpu_status: Availability<()>,
    pub ram: Sample<f64>,
    pub gpu: Sample<f64>,
    pub adapters: Vec<GpuAdapter>,
    pub gpu_processes: Option<std::sync::Arc<GpuProcesses>>,
    pub disk: Sample<f64>,
    pub disk_read_latency: Sample<f64>,
    pub disk_write_latency: Sample<f64>,
    pub network: Sample<f64>,
    pub monitor: Monitor,
    pub endpoints: Option<std::sync::Arc<EndpointSnapshot>>,
    pub processes: Vec<Process>,
    pub contributors: Option<std::sync::Arc<[Contributor]>>,
    pub skipped: usize,
    pub process_error: Option<i32>,
}

/// Live-only context for the hardware monitor; not part of the session schema.
#[derive(Debug, Default)]
pub struct Monitor {
    pub uptime_ms: Option<u64>,
    /// Used and total physical bytes, from the same Windows reading.
    pub ram_bytes: Availability<[u64; 2]>,
    pub disk_rates: [Availability<f64>; 2],
    pub network_rates: [Availability<f64>; 2],
    pub cpu_thermal: Availability<CpuThermal>,
}

pub const DISK_LIMIT: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct DriveSample {
    pub index: u32,
    /// Windows PnP identity, used only in memory to invalidate time series after hot-plug.
    pub identity: String,
    pub model: String,
    pub bus: String,
    pub bytes: Option<u64>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub busy_percent: Option<f64>,
    pub read_ms: Option<f64>,
    pub write_ms: Option<f64>,
    pub health: Option<u16>,
    pub temperature: Option<u8>,
    pub wear: Option<u8>,
    pub power_on_hours: Option<u64>,
    pub written_bytes: Option<u64>,
    pub nvme_smart: bool,
    pub volumes: Vec<DriveVolume>,
}

#[derive(Debug, Clone)]
pub struct DriveVolume {
    pub letter: String,
    pub name: String,
    pub bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub spans_multiple_disks: bool,
}

#[derive(Debug)]
pub struct StorageSnapshot {
    pub elapsed_ms: u64,
    pub drives: Vec<DriveSample>,
    pub omitted: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct CpuThermal {
    pub celsius: f64,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contributor {
    pub key: ProcessKey,
    pub name: String,
    pub cpu_percent: Option<f64>,
    pub private_bytes: Option<u64>,
}

pub const CONTRIBUTOR_LIMIT: usize = 12;

/// Union of the six largest measured CPU and private-memory consumers.
pub fn top_contributors(processes: &[Process]) -> Vec<Contributor> {
    fn insert(top: &mut Vec<usize>, index: usize, better: impl Fn(usize) -> bool) {
        let slot = top
            .iter()
            .position(|&other| better(other))
            .unwrap_or(top.len());
        if slot >= 6 {
            return;
        }
        if top.len() == 6 {
            top.pop();
        }
        top.insert(slot, index);
    }
    let mut cpu = Vec::with_capacity(6);
    let mut ram = Vec::with_capacity(6);
    for (index, p) in processes.iter().enumerate() {
        if let Some(value) = p.cpu.value.get().filter(|v| (0. ..=100.).contains(v)) {
            insert(&mut cpu, index, |other| {
                let other = &processes[other];
                value
                    .total_cmp(&other.cpu.value.get().unwrap())
                    .then_with(|| other.key.cmp(&p.key))
                    .is_gt()
            });
        }
        if let Some(value) = p.private_bytes.value.get() {
            insert(&mut ram, index, |other| {
                let other = &processes[other];
                value
                    .cmp(&other.private_bytes.value.get().unwrap())
                    .then_with(|| other.key.cmp(&p.key))
                    .is_gt()
            });
        }
    }
    cpu.iter()
        .copied()
        .chain(ram.into_iter().filter(|i| !cpu.contains(i)))
        .map(|i| {
            let p = &processes[i];
            Contributor {
                key: p.key,
                name: p.name.chars().take(260).collect(),
                cpu_percent: p.cpu.value.get().filter(|v| (0. ..=100.).contains(v)),
                private_bytes: p.private_bytes.value.get(),
            }
        })
        .collect()
}

pub const MIB: u64 = 1024 * 1024;
/// Width and height of the straight-alpha RGBA process icon.
pub const PROCESS_ICON_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Debug)]
pub struct Endpoint {
    pub transport: Transport,
    pub local: std::net::SocketAddr,
    pub remote: Option<std::net::SocketAddr>,
    pub tcp_state: Option<u32>,
    pub pid: u32,
}

#[derive(Debug)]
pub struct EndpointSnapshot {
    pub utc_ms: u64,
    pub entries: Vec<Endpoint>,
    /// TCPv4, TCPv6, UDPv4, UDPv6.
    pub tables: [Availability<usize>; 4],
    pub omitted: usize,
}

#[derive(Debug)]
pub struct LogicalCpu {
    pub group: u16,
    pub number: u32,
    pub usage: Sample<f64>,
    pub frequency: Sample<f64>,
}

#[derive(Debug)]
pub struct GpuAdapter {
    pub id: String,
    pub name: String,
    pub utilization: Sample<f64>,
    pub vram_used: Sample<f64>,
    pub vram_total: Sample<f64>,
    pub temperature: Sample<f64>,
    pub clock: Sample<f64>,
    pub power: Sample<f64>,
    pub fan: Sample<f64>,
    pub encode: Sample<f64>,
    pub decode: Sample<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuKey {
    pub luid_high: u32,
    pub luid_low: u32,
    pub physical: u32,
}

#[derive(Debug)]
pub struct GpuProcess {
    pub process: ProcessKey,
    pub name: String,
    pub adapter: GpuKey,
    pub busiest_engine: Option<String>,
    pub activity: Sample<f64>,
    pub dedicated: Sample<f64>,
    pub shared: Sample<f64>,
}

#[derive(Debug)]
pub struct GpuProcesses {
    pub utc_ms: u64,
    pub rows: Vec<GpuProcess>,
    /// Activity, dedicated and shared counters in that order.
    pub status: [Availability<()>; 3],
    pub excluded: usize,
}
