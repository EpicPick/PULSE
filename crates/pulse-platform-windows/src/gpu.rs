use super::sample;
use nvml_wrapper::{
    Nvml,
    enum_wrappers::device::{Clock, TemperatureSensor},
    error::NvmlError,
};
use pulse_domain::{Availability, GpuAdapter, Unit};
use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

pub const SOURCE: &str = "NVIDIA NVML · activité GPU par adaptateur";

#[derive(Default)]
pub struct Gpu {
    nvml: Option<Nvml>,
    retry: Option<(Instant, Availability<f64>)>,
}

fn library() -> Option<PathBuf> {
    let mut buffer = [0; 32768];
    // SAFETY: writable UTF-16 buffer; the function returns the character count on success.
    let size = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if size == 0 || size >= buffer.len() {
        return None;
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..size])).join("nvml.dll");
    // No PATH, current-directory search, or DLL shipped next to the executable.
    path.is_absolute().then_some(path)
}

fn failure<T>(e: NvmlError) -> Availability<T> {
    use NvmlError::*;
    match e {
        NotSupported
        | LibraryNotFound
        | DriverNotLoaded
        | LibloadingError(_)
        | FunctionNotFound
        | FailedToLoadSymbol(_) => Availability::Unsupported,
        NoPermission | OperatingSystem => Availability::PermissionRequired,
        Timeout | NoData | GpuLost | ResetRequired => Availability::Pending,
        _ => Availability::Error(u32::from(e) as i32),
    }
}

impl Gpu {
    pub fn collect(&mut self, t: u64) -> (Vec<GpuAdapter>, Availability<f64>) {
        let now = Instant::now();
        if let Some((deadline, status)) = self.retry.filter(|(deadline, _)| now < *deadline) {
            let _ = deadline;
            return (Vec::new(), status);
        }
        if self.nvml.is_none() {
            let Some(path) = library() else {
                return (Vec::new(), Availability::Unsupported);
            };
            match Nvml::builder().lib_path(path.as_os_str()).init() {
                Ok(nvml) => self.nvml = Some(nvml),
                Err(e) => {
                    let status = failure(e);
                    self.retry = Some((now + Duration::from_secs(30), status));
                    return (Vec::new(), status);
                }
            }
        }
        let nvml = self.nvml.as_ref().unwrap();
        let count = match nvml.device_count() {
            Ok(n) => n.min(64),
            Err(e) => {
                let status = failure(e);
                self.nvml = None;
                self.retry = Some((now + Duration::from_secs(30), status));
                return (Vec::new(), status);
            }
        };
        let mut adapters = Vec::with_capacity(count as usize);
        let mut status = Availability::Unsupported;
        for i in 0..count {
            let dev = match nvml.device_by_index(i) {
                Ok(dev) => dev,
                Err(e) => {
                    status = failure(e);
                    continue;
                }
            };
            let (used, total) = match dev.memory_info() {
                Ok(mem) if mem.total > 0 && mem.used <= mem.total => (
                    Availability::Available(mem.used as f64),
                    Availability::Available(mem.total as f64),
                ),
                Ok(_) => (Availability::Pending, Availability::Pending),
                Err(e) => {
                    let status = failure(e);
                    (status, status)
                }
            };
            let reading = |r: Result<u32, NvmlError>, unit, source, factor| {
                sample(
                    t,
                    unit,
                    source,
                    match r {
                        Ok(v) if v != u32::MAX && (unit != Unit::Percent || v <= 100) => {
                            Availability::Available(f64::from(v) * factor)
                        }
                        Ok(_) => Availability::Pending,
                        Err(e) => failure(e),
                    },
                )
            };
            adapters.push(GpuAdapter {
                id: dev.uuid().unwrap_or_else(|_| format!("nvml-index-{i}")),
                name: dev
                    .name()
                    .unwrap_or_else(|_| format!("Adaptateur NVIDIA {}", i + 1)),
                utilization: reading(
                    dev.utilization_rates().map(|r| r.gpu),
                    Unit::Percent,
                    SOURCE,
                    1.,
                ),
                vram_used: sample(
                    t,
                    Unit::Bytes,
                    "NVML · DeviceGetMemoryInfo (occupée / réservée)",
                    used,
                ),
                vram_total: sample(
                    t,
                    Unit::Bytes,
                    "NVML · DeviceGetMemoryInfo (capacité)",
                    total,
                ),
                temperature: reading(
                    dev.temperature(TemperatureSensor::Gpu),
                    Unit::Celsius,
                    "NVML · DeviceGetTemperature",
                    1.,
                ),
                clock: reading(
                    dev.clock_info(Clock::Graphics),
                    Unit::Megahertz,
                    "NVML · DeviceGetClockInfo (graphics)",
                    1.,
                ),
                power: reading(
                    dev.power_usage(),
                    Unit::Watts,
                    "NVML · DeviceGetPowerUsage",
                    0.001,
                ),
                fan: reading(
                    dev.fan_speed(0),
                    Unit::Percent,
                    "NVML · DeviceGetFanSpeed_v2 (ventilateur 0, consigne)",
                    1.,
                ),
                encode: reading(
                    dev.encoder_utilization().map(|r| r.utilization),
                    Unit::Percent,
                    "NVML · DeviceGetEncoderUtilization",
                    1.,
                ),
                decode: reading(
                    dev.decoder_utilization().map(|r| r.utilization),
                    Unit::Percent,
                    "NVML · DeviceGetDecoderUtilization",
                    1.,
                ),
            });
        }
        (adapters, status)
    }
}
