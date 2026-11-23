#![forbid(unsafe_code)]
use pulse_domain::{Availability::Available, MIB, Process, ProcessKey, Sample, Snapshot, Unit};

pub const EPOCH: u64 = 1_795_350_000_000;
const NAMES: [&str; 12] = [
    "Studio Render",
    "Navigateur Atlas",
    "Code Editor",
    "Explorateur",
    "Audio Engine",
    "Desktop Shell",
    "Sync Service",
    "Media Player",
    "Indexeur",
    "Runtime Broker",
    "Workspace",
    "Console",
];

pub fn sample<T>(value: T, unit: Unit, utc_ms: u64) -> Sample<T> {
    Sample {
        value: Available(value),
        unit,
        utc_ms,
        source: "Fixture déterministe PULSE",
    }
}

pub fn system(t: u64) -> [f64; 3] {
    let x = t as f64;
    let burst = (-(x.rem_euclid(119.0) - 83.0).powi(2) / 125.0).exp();
    [
        24.0 + 10.0 * (x / 9.0).sin() + 5.0 * (x / 2.1).sin() + 46.0 * burst,
        48.0 + 5.0 * (x / 28.0).sin() + 9.0 * burst,
        58.0 + 15.0 * (x / 14.0).cos() + 5.0 * (x / 3.0).sin() + 19.0 * burst,
    ]
}

pub fn throughput(t: u64) -> [f64; 2] {
    let [cpu, _, gpu] = system(t);
    [1_700_000.0 + cpu * 115_000.0, 100_000.0 + gpu * 19_000.0]
}

pub fn snapshot(t: u64) -> Snapshot {
    let utc_ms = EPOCH + t * 1_000;
    let [cpu, ram, gpu] = system(t);
    let [disk, network] = throughput(t);
    let processes = (0..500u32)
        .map(|i| {
            let name = NAMES[i as usize % NAMES.len()];
            let name = if i < 12 {
                format!("{name}.exe")
            } else {
                format!("{name} {:03}.exe", i / 12)
            };
            let private = if i == 0 {
                640 + t % 180
            } else {
                20 + u64::from(i * 71 % 700)
            };
            Process {
                key: ProcessKey {
                    pid: 4200 + i * 4,
                    created_at_ticks: EPOCH * 10_000 + u64::from(i),
                },
                parent_pid: if i > 12 { 4200 + (i % 12) * 4 } else { 0 },
                image_path: sample(format!(r"C:\PULSE Demo\{name}").into(), Unit::Path, utc_ms),
                name,
                icon: None,
                priority: sample(pulse_domain::Priority::Normal, Unit::PriorityClass, utc_ms),
                cpu: sample(
                    if i == 0 {
                        cpu / 2.0
                    } else {
                        f64::from(i * 17 % 91) / 40.0
                    },
                    Unit::Percent,
                    utc_ms,
                ),
                working_set: sample((private * 4 / 5 + 12) * MIB, Unit::Bytes, utc_ms),
                private_bytes: sample(private * MIB, Unit::Bytes, utc_ms),
                threads: sample(3 + i * 7 % 64, Unit::Count, utc_ms),
                handles: sample(34 + i * 41 % 1000, Unit::Count, utc_ms),
                io_read: sample(f64::from(i * 47 % 7000), Unit::BytesPerSecond, utc_ms),
                io_write: sample(f64::from(i * 29 % 2000), Unit::BytesPerSecond, utc_ms),
            }
        })
        .collect();
    Snapshot {
        utc_ms,
        elapsed_ms: t * 1000,
        cpu: sample(cpu, Unit::Percent, utc_ms),
        logical_cpus: Vec::new(),
        logical_cpu_status: pulse_domain::Availability::Unsupported,
        ram: sample(ram, Unit::Percent, utc_ms),
        gpu: sample(gpu, Unit::Percent, utc_ms),
        adapters: Vec::new(),
        gpu_processes: None,
        disk: sample(disk, Unit::BytesPerSecond, utc_ms),
        disk_read_latency: sample(1.7, Unit::Milliseconds, utc_ms),
        disk_write_latency: sample(2.2, Unit::Milliseconds, utc_ms),
        network: sample(network, Unit::BytesPerSecond, utc_ms),
        monitor: pulse_domain::Monitor {
            cpu_thermal: pulse_domain::Availability::Pending,
            uptime_ms: Some(35_570_000 + t * 1000),
            ram_bytes: pulse_domain::Availability::Available([
                (ram / 100. * 32. * 1024. * MIB as f64) as u64,
                32 * 1024 * MIB,
            ]),
            disk_rates: [0.7, 0.3].map(|v| pulse_domain::Availability::Available(v * disk)),
            network_rates: [0.86, 0.14].map(|v| pulse_domain::Availability::Available(v * network)),
        },
        processes,
        contributors: None,
        endpoints: None,
        skipped: 0,
        process_error: None,
    }
}

pub fn monitor_adapter(utc_ms: u64) -> pulse_domain::GpuAdapter {
    let read = |value, unit| sample(value, unit, utc_ms);
    pulse_domain::GpuAdapter {
        id: "fixture-gpu".into(),
        name: "Adaptateur de démonstration".into(),
        utilization: read(62., Unit::Percent),
        vram_used: read(6.25 * 1024. * MIB as f64, Unit::Bytes),
        vram_total: read(12. * 1024. * MIB as f64, Unit::Bytes),
        temperature: read(64., Unit::Celsius),
        clock: read(1815., Unit::Megahertz),
        power: read(168., Unit::Watts),
        fan: read(38., Unit::Percent),
        encode: read(0., Unit::Percent),
        decode: read(0., Unit::Percent),
    }
}

pub fn storage(t: u64) -> pulse_domain::StorageSnapshot {
    use pulse_domain::{DriveSample, DriveVolume, StorageSnapshot};
    let gib = 1024_u64.pow(3);
    let drives = [
        ("SSD NVMe", "NVMe", 930, "C:", "Windows"),
        ("SSD SATA", "SATA", 1860, "D:", "Jeux"),
        ("Disque externe", "USB", 930, "E:", "Sauvegarde"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (model, bus, size, letter, name))| {
        let measured = i != 2;
        let phase = (t as f64 * 0.19).sin() * 0.5 + 0.5;
        DriveSample {
            index: i as u32,
            identity: format!("fixture-{i}"),
            model: model.into(),
            bus: bus.into(),
            bytes: Some(size * gib),
            read_bps: Some(if measured {
                (phase * 100. + 10.) * MIB as f64 / (i + 1) as f64
            } else {
                0.
            }),
            write_bps: Some(if measured {
                (phase * 15. + 2.) * MIB as f64
            } else {
                0.
            }),
            busy_percent: Some(if measured { phase * 44. } else { 0. }),
            read_ms: measured.then_some(0.45 + i as f64),
            write_ms: measured.then_some(0.71 + i as f64),
            health: measured.then_some(0),
            temperature: measured.then_some(46 - i as u8 * 8),
            wear: measured.then_some(4 + i as u8 * 6),
            power_on_hours: measured.then_some(6420),
            nvme_smart: i == 0,
            written_bytes: (i == 0).then_some(18_400_000_000_000),
            volumes: vec![DriveVolume {
                letter: letter.into(),
                name: name.into(),
                bytes: Some(size * gib),
                free_bytes: Some(size * gib / 3),
                spans_multiple_disks: false,
            }],
        }
    })
    .collect();
    StorageSnapshot {
        elapsed_ms: t * 1000,
        drives,
        omitted: false,
        note: None,
    }
}
