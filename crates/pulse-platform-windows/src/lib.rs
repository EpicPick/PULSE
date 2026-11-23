#![cfg(windows)]
mod cpu;
mod disk;
mod storage;
pub use storage::Storage;
mod display;
mod endpoints;
mod gpu;
mod hwinfo;
mod network;
mod process_icons;
mod wddm;
pub use display::secondary_window;

pub fn ui_language_is_french() -> bool {
    // SAFETY: no arguments, no memory access through pointers; returns the user's UI LANGID.
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    // PRIMARYLANGID: lower 10 bits. LANG_FRENCH = 0x0c, including all regional variants.
    lang & 0x03ff == 0x0c
}

pub fn show_startup_error(message: &str) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        core::{HSTRING, w},
    };
    // SAFETY: HSTRING and the static title remain valid until the synchronous dialog returns.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("PULSE"),
            MB_OK | MB_ICONERROR,
        );
    }
}
use std::{
    collections::HashMap,
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pulse_actions::{Change, Command, Outcome, PriorityTarget};
use pulse_domain::{Availability, Priority, Process, ProcessKey, Sample, Snapshot, Unit};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_MORE_FILES, FILETIME, HANDLE},
        System::{Diagnostics::ToolHelp::*, ProcessStatus::*, SystemInformation::*, Threading::*},
    },
    core::{Error, HRESULT, PWSTR},
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: successful APIs transfer ownership; no other owner closes this handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Default)]
pub struct Collector {
    system: Option<(u64, u64)>,
    processes: HashMap<ProcessKey, Baseline>,
    next: HashMap<ProcessKey, Baseline>,
    paths: HashMap<ProcessKey, Sample<Arc<str>>>,
    path_buffer: Vec<u16>,
    disk: disk::Disk,
    network: network::Network,
    gpu: gpu::Gpu,
    logical_cpu: cpu::Cpu,
    icons: process_icons::Icons,
    endpoints: endpoints::Endpoints,
    wddm: wddm::Wddm,
}

struct Baseline {
    cpu_ticks: u64,
    io: Option<(Instant, [u64; 2])>,
}

fn io_rate(old: u64, new: u64, elapsed: Duration) -> Availability<f64> {
    if elapsed.is_zero() || elapsed > Duration::from_secs(15) {
        return Availability::Pending;
    }
    new.checked_sub(old).map_or(Availability::Pending, |count| {
        Availability::Available(count as f64 / elapsed.as_secs_f64())
    })
}

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.as_millis() as u64)
        .unwrap_or_default()
}

pub fn pending() -> Snapshot {
    let t = stamp();
    Snapshot {
        utc_ms: t,
        elapsed_ms: 0,
        cpu: sample(t, Unit::Percent, "GetSystemTimes", Availability::Pending),
        logical_cpus: Vec::new(),
        logical_cpu_status: Availability::Pending,
        ram: sample(
            t,
            Unit::Percent,
            "GlobalMemoryStatusEx",
            Availability::Pending,
        ),
        gpu: sample(t, Unit::Percent, gpu::SOURCE, Availability::Pending),
        adapters: Vec::new(),
        gpu_processes: None,
        disk: sample(t, Unit::BytesPerSecond, disk::SOURCE, Availability::Pending),
        disk_read_latency: sample(
            t,
            Unit::Milliseconds,
            disk::READ_LATENCY,
            Availability::Pending,
        ),
        disk_write_latency: sample(
            t,
            Unit::Milliseconds,
            disk::WRITE_LATENCY,
            Availability::Pending,
        ),
        network: sample(
            t,
            Unit::BytesPerSecond,
            network::SOURCE,
            Availability::Pending,
        ),
        monitor: Default::default(),
        processes: Vec::new(),
        contributors: None,
        endpoints: None,
        skipped: 0,
        process_error: None,
    }
}

fn sample<T>(t: u64, unit: Unit, source: &'static str, value: Availability<T>) -> Sample<T> {
    Sample {
        utc_ms: t,
        unit,
        source,
        value,
    }
}

fn error<T>(e: Error) -> Availability<T> {
    if e.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) {
        Availability::PermissionRequired
    } else {
        Availability::Error(e.code().0)
    }
}

fn ticks(x: FILETIME) -> u64 {
    u64::from(x.dwHighDateTime) << 32 | u64::from(x.dwLowDateTime)
}

fn percent(delta: u64, total: u64) -> Option<f64> {
    (total != 0 && delta <= total).then(|| delta as f64 / total as f64 * 100.)
}

fn priority_from_flags(flags: u32) -> Option<Priority> {
    match flags {
        v if v == IDLE_PRIORITY_CLASS.0 => Some(Priority::Idle),
        v if v == BELOW_NORMAL_PRIORITY_CLASS.0 => Some(Priority::Low),
        v if v == NORMAL_PRIORITY_CLASS.0 => Some(Priority::Normal),
        v if v == ABOVE_NORMAL_PRIORITY_CLASS.0 => Some(Priority::AboveNormal),
        v if v == HIGH_PRIORITY_CLASS.0 => Some(Priority::High),
        v if v == REALTIME_PRIORITY_CLASS.0 => Some(Priority::Realtime),
        _ => None,
    }
}

/// Called only after explicit UI confirmation and never elevates the current token.
pub fn execute(Command { key, change }: Command) -> Outcome {
    if key.pid <= 4 || key.pid == std::process::id() {
        return Outcome::Protected;
    }
    // SAFETY: request only the rights needed, and use the same owned handle for verification
    // and termination so a PID reused between calls cannot change the target.
    let rights = match change {
        Change::Terminate => PROCESS_TERMINATE,
        Change::SetPriority(_) => PROCESS_SET_INFORMATION,
    };
    let h = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | rights, false, key.pid) }
    {
        Ok(h) => OwnedHandle(h),
        Err(e) => return Outcome::OsError(e.code().0),
    };
    let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
    if let Err(e) = unsafe { GetProcessTimes(h.0, &mut created, &mut exit, &mut kernel, &mut user) }
    {
        return Outcome::OsError(e.code().0);
    }
    if ticks(created) != key.created_at_ticks || ticks(exit) != 0 {
        return Outcome::InstanceChanged;
    }
    let mut critical = windows::core::BOOL::default();
    match unsafe { IsProcessCritical(h.0, &mut critical) } {
        Err(e) => return Outcome::OsError(e.code().0),
        Ok(()) if critical.as_bool() => return Outcome::Protected,
        Ok(()) => {}
    }
    let result = match change {
        Change::Terminate => unsafe { TerminateProcess(h.0, 1) }.map(|()| Outcome::Requested),
        Change::SetPriority(target) => {
            let class = match target {
                PriorityTarget::Idle => IDLE_PRIORITY_CLASS,
                PriorityTarget::Low => BELOW_NORMAL_PRIORITY_CLASS,
                PriorityTarget::Normal => NORMAL_PRIORITY_CLASS,
                PriorityTarget::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            };
            // SAFETY: same verified handle; these are documented non-real-time classes.
            unsafe { SetPriorityClass(h.0, class) }.map(|()| Outcome::PriorityApplied)
        }
    };
    match result {
        Ok(status) => status,
        Err(e) => Outcome::OsError(e.code().0),
    }
}

impl Collector {
    pub fn sample(&mut self) -> Snapshot {
        self.sample_with_details(false, false)
    }

    pub fn sample_with_details(&mut self, inspect_network: bool, inspect_gpu: bool) -> Snapshot {
        let mut out = pending();
        let t = out.utc_ms;
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let mut mem = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: all output structures are valid, writable, and correctly sized for Win32.
        let groups = unsafe { GetActiveProcessorGroupCount() };
        let times = unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) };
        let mut elapsed = None;
        out.cpu.value = if groups != 1 {
            self.system = None;
            Availability::Unsupported
        } else if let Err(e) = times {
            self.system = None;
            error(e)
        } else {
            let total = ticks(kernel).saturating_add(ticks(user));
            let id = ticks(idle);
            let last = self.system.replace((total, id));
            elapsed = last
                .and_then(|(prev_total, _)| total.checked_sub(prev_total))
                .filter(|x| *x > 0);
            last.zip(elapsed)
                .and_then(|((_, prev_idle), elapsed)| {
                    elapsed
                        .checked_sub(id.checked_sub(prev_idle)?)
                        .and_then(|active| percent(active, elapsed))
                })
                .map_or(Availability::Pending, Availability::Available)
        };
        // SAFETY: no pointers or privilege requirements; Windows provides milliseconds since boot.
        out.monitor.uptime_ms = Some(unsafe { GetTickCount64() });
        out.monitor.cpu_thermal = hwinfo::sample(t);
        out.ram.value = match unsafe { GlobalMemoryStatusEx(&mut mem) } {
            Ok(()) => {
                let reading = mem.ullTotalPhys.checked_sub(mem.ullAvailPhys);
                if let Some(used) = reading.filter(|_| mem.ullTotalPhys > 0) {
                    out.monitor.ram_bytes = Availability::Available([used, mem.ullTotalPhys]);
                }
                reading
                    .and_then(|used| percent(used, mem.ullTotalPhys))
                    .map_or(Availability::Pending, Availability::Available)
            }
            Err(e) => {
                out.monitor.ram_bytes = error(e.clone());
                error(e)
            }
        };
        [
            out.disk.value,
            out.disk_read_latency.value,
            out.disk_write_latency.value,
            out.monitor.disk_rates[0],
            out.monitor.disk_rates[1],
        ] = self.disk.sample();
        let [total, rx, tx] = self.network.sample();
        out.network.value = total;
        out.monitor.network_rates = [rx, tx];
        if inspect_network {
            out.endpoints = Some(self.endpoints.sample(stamp()));
        } else {
            self.endpoints.clear();
        }
        (out.logical_cpus, out.logical_cpu_status) = self.logical_cpu.sample(t);
        let (adapters, status) = self.gpu.collect(t);
        out.gpu.value = adapters.first().map_or(status, |a| a.utilization.value);
        out.adapters = adapters;
        self.next.clear();
        self.icons.begin();
        if let Err(e) = self.enumerate(t, elapsed, groups == 1, &mut out) {
            out.process_error = Some(e.code().0);
        }
        if inspect_gpu {
            out.gpu_processes = Some(self.wddm.collect(stamp(), &out.processes));
        } else {
            self.wddm.clear();
        }
        std::mem::swap(&mut self.processes, &mut self.next);
        self.paths.retain(|key, _| self.processes.contains_key(key));
        out
    }

    fn image_path(&mut self, t: u64, key: ProcessKey, handle: HANDLE) -> Sample<Arc<str>> {
        if let Some(path) = self.paths.get(&key) {
            return path.clone();
        }
        if self.path_buffer.is_empty() {
            self.path_buffer.resize(32_768, 0);
        }
        let mut len = self.path_buffer.len() as u32;
        // SAFETY: writable UTF-16 buffer has exactly the supplied capacity; process handle is live.
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(self.path_buffer.as_mut_ptr()),
                &mut len,
            )
        };
        let value = match result {
            Ok(()) => Availability::Available(Arc::<str>::from(String::from_utf16_lossy(
                &self.path_buffer[..len as usize],
            ))),
            Err(e) => error(e),
        };
        let sample = sample(t, Unit::Path, "QueryFullProcessImageNameW", value);
        if matches!(sample.value, Availability::Available(_)) {
            self.paths.insert(key, sample.clone());
        }
        sample
    }

    fn enumerate(
        &mut self,
        t: u64,
        elapsed: Option<u64>,
        single_group: bool,
        out: &mut Snapshot,
    ) -> windows::core::Result<()> {
        // SAFETY: snapshot entries are initialized with their ABI size; handles live through calls.
        let before = unsafe { GetSystemTimePreciseAsFileTime() };
        let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        unsafe {
            Process32FirstW(snapshot.0, &mut entry)?;
        }
        loop {
            if let Some(p) = self.process(t, elapsed, single_group, ticks(before), &entry) {
                out.processes.push(p);
            } else {
                out.skipped += 1;
            }
            if let Err(e) = unsafe { Process32NextW(snapshot.0, &mut entry) } {
                return if e.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                    Ok(())
                } else {
                    Err(e)
                };
            }
        }
    }

    fn process(
        &mut self,
        t: u64,
        elapsed: Option<u64>,
        single_group: bool,
        before: u64,
        entry: &PROCESSENTRY32W,
    ) -> Option<Process> {
        let pid = entry.th32ProcessID;
        // SAFETY: open read-only; all query buffers live through Win32 calls. The memory
        // counter pointer is its ABI base; cb describes the actual extended allocation.
        let h = OwnedHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?
        });
        let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        unsafe {
            GetProcessTimes(h.0, &mut created, &mut exit, &mut kernel, &mut user).ok()?;
        }
        let created_at_ticks = ticks(created);
        // Do not join a Toolhelp row to a possibly recycled PID opened after the snapshot.
        if created_at_ticks == 0 || created_at_ticks >= before || ticks(exit) != 0 {
            return None;
        }
        let key = ProcessKey {
            pid,
            created_at_ticks,
        };
        let total = ticks(kernel).saturating_add(ticks(user));
        let mut io = IO_COUNTERS::default();
        let at = Instant::now();
        // SAFETY: live process handle and a correctly sized writable IO_COUNTERS.
        let measured = unsafe { GetProcessIoCounters(h.0, &mut io) };
        let (baseline, rates) = match measured {
            Ok(()) => {
                let bytes = [io.ReadTransferCount, io.WriteTransferCount];
                let rates = self.processes.get(&key).and_then(|last| last.io).map_or(
                    [Availability::Pending; 2],
                    |(previous_at, previous)| {
                        let elapsed = at.duration_since(previous_at);
                        [
                            io_rate(previous[0], bytes[0], elapsed),
                            io_rate(previous[1], bytes[1], elapsed),
                        ]
                    },
                );
                (Some((at, bytes)), rates)
            }
            Err(e) => (None, [error(e); 2]),
        };
        self.next.insert(
            key,
            Baseline {
                cpu_ticks: total,
                io: baseline,
            },
        );
        let cpu = if single_group {
            self.processes
                .get(&key)
                .zip(elapsed)
                .and_then(|(last, dt)| percent(total.checked_sub(last.cpu_ticks)?, dt))
                .map_or(Availability::Pending, Availability::Available)
        } else {
            Availability::Unsupported
        };
        let mut memory = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let cb = memory.cb;
        let (working_set, private) = if unsafe {
            K32GetProcessMemoryInfo(
                h.0,
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                cb,
            )
        }
        .as_bool()
        {
            (
                Availability::Available(memory.WorkingSetSize as u64),
                Availability::Available(memory.PrivateUsage as u64),
            )
        } else {
            let failure = error(Error::from_thread());
            (failure, failure)
        };
        let mut handles = 0;
        let handles = match unsafe { GetProcessHandleCount(h.0, &mut handles) } {
            Ok(()) => Availability::Available(handles),
            Err(e) => error(e),
        };
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let image_path = self.image_path(t, key, h.0);
        // SAFETY: the process handle is live and grants PROCESS_QUERY_LIMITED_INFORMATION.
        let raw = unsafe { GetPriorityClass(h.0) };
        let priority = if raw == 0 {
            error(Error::from_thread())
        } else {
            priority_from_flags(raw).map_or(Availability::Unsupported, Availability::Available)
        };
        let icon = match &image_path.value {
            Availability::Available(path) => self.icons.get(path),
            _ => None,
        };
        Some(Process {
            key,
            name: String::from_utf16_lossy(&entry.szExeFile[..end]),
            parent_pid: entry.th32ParentProcessID,
            image_path,
            icon,
            priority: sample(t, Unit::PriorityClass, "GetPriorityClass", priority),
            cpu: sample(t, Unit::Percent, "GetProcessTimes / GetSystemTimes", cpu),
            working_set: sample(
                t,
                Unit::Bytes,
                "K32GetProcessMemoryInfo.WorkingSetSize",
                working_set,
            ),
            private_bytes: sample(
                t,
                Unit::Bytes,
                "K32GetProcessMemoryInfo.PrivateUsage",
                private,
            ),
            threads: sample(
                t,
                Unit::Count,
                "Toolhelp PROCESSENTRY32W",
                Availability::Available(entry.cntThreads),
            ),
            handles: sample(t, Unit::Count, "GetProcessHandleCount", handles),
            io_read: sample(
                t,
                Unit::BytesPerSecond,
                "GetProcessIoCounters.ReadTransferCount",
                rates[0],
            ),
            io_write: sample(
                t,
                Unit::BytesPerSecond,
                "GetProcessIoCounters.WriteTransferCount",
                rates[1],
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_cpu_deltas() {
        assert_eq!(percent(0, 0), None);
        assert_eq!(percent(21, 20), None);
        assert_eq!(percent(0, 20), Some(0.));
    }

    #[test]
    fn process_io_has_no_fake_sample_after_reset_or_long_gap() {
        assert_eq!(
            io_rate(200, 500, Duration::from_secs(2)),
            Availability::Available(150.)
        );
        assert_eq!(
            io_rate(500, 200, Duration::from_secs(2)),
            Availability::Pending
        );
        assert_eq!(
            io_rate(500, 500, Duration::from_secs(1)),
            Availability::Available(0.)
        );
        assert_eq!(
            io_rate(500, 900, Duration::from_secs(16)),
            Availability::Pending
        );
    }

    #[test]
    fn self_termination_is_blocked_before_opening_the_process() {
        assert_eq!(
            execute(Command {
                key: ProcessKey {
                    pid: std::process::id(),
                    created_at_ticks: 0
                },
                change: Change::Terminate
            }),
            Outcome::Protected
        );
    }

    #[test]
    fn priority_test_child() {
        if std::env::var_os("PULSE_INTERNAL_PRIORITY_CHILD").is_some() {
            let mut ready = String::new();
            // Parent keeps stdin open while it checks the priority. No GUI/console window.
            let _ = std::io::stdin().read_line(&mut ready);
        }
    }

    #[test]
    fn changes_priority_of_owned_test_child_but_rejects_wrong_creation_time() {
        use std::{os::windows::process::CommandExt, process::Stdio};
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::priority_test_child"])
            .env("PULSE_INTERNAL_PRIORITY_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let h = OwnedHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, child.id()).unwrap()
        });
        let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        unsafe {
            GetProcessTimes(h.0, &mut created, &mut exit, &mut kernel, &mut user).unwrap();
        }
        let key = ProcessKey {
            pid: child.id(),
            created_at_ticks: ticks(created),
        };
        let result = execute(Command {
            key: ProcessKey {
                created_at_ticks: key.created_at_ticks.saturating_sub(1),
                ..key
            },
            change: Change::SetPriority(PriorityTarget::Idle),
        });
        let changed = execute(Command {
            key,
            change: Change::SetPriority(PriorityTarget::Low),
        });
        let raw = unsafe { GetPriorityClass(h.0) };
        drop(input);
        assert!(child.wait().unwrap().success());
        assert_eq!(result, Outcome::InstanceChanged);
        assert_eq!(changed, Outcome::PriorityApplied);
        assert_eq!(priority_from_flags(raw), Some(Priority::Low));
    }

    #[test]
    fn observes_current_process_with_real_identity_and_memory() {
        let mut c = Collector::default();
        let first = c.sample();
        let own = |s: &Snapshot| {
            s.processes
                .iter()
                .find(|p| p.key.pid == std::process::id())
                .unwrap()
                .key
        };
        let key = own(&first);
        assert!(key.created_at_ticks > 0);
        let own_first = first.processes.iter().find(|p| p.key == key).unwrap();
        assert_eq!(own_first.io_read.value, Availability::Pending);
        assert_eq!(own_first.io_write.value, Availability::Pending);
        let path =
            std::env::temp_dir().join(format!("pulse-io-{}-{}.bin", key.pid, key.created_at_ticks));
        use std::io::Write;
        {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            file.write_all(&[0u8; 4096]).unwrap();
        }
        std::fs::remove_file(path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        let second = c.sample();
        assert_eq!(own(&second), key);
        assert!(second.ram.value.get().is_some_and(|x| x > 0. && x <= 100.));
        let [used, total] = second.monitor.ram_bytes.get().unwrap();
        assert!(used > 0 && used <= total);
        assert_eq!(second.ram.value.get(), percent(used, total));
        assert!(second.monitor.uptime_ms.unwrap() >= first.monitor.uptime_ms.unwrap());
        for rate in second
            .monitor
            .disk_rates
            .into_iter()
            .chain(second.monitor.network_rates)
        {
            assert!(rate.get().is_none_or(|v| v.is_finite() && v >= 0.));
        }
        if unsafe { GetActiveProcessorGroupCount() } == 1 {
            assert!(second.cpu.value.get().is_some());
        }
        let p = second.processes.iter().find(|p| p.key == key).unwrap();
        assert!(
            matches!(&p.image_path.value, Availability::Available(path) if path.ends_with(".exe"))
        );
        assert!(p.private_bytes.value.get().is_some_and(|x| x > 0));
        assert!(p.io_write.value.get().is_some_and(|bps| bps > 0.));
        assert!(p.working_set.value.get().is_some_and(|x| x > 0));
        assert!(
            second
                .gpu
                .value
                .get()
                .is_none_or(|v| (0. ..=100.).contains(&v))
        );
        for a in &second.adapters {
            assert!(
                a.vram_used
                    .value
                    .get()
                    .zip(a.vram_total.value.get())
                    .is_none_or(|(used, total)| (0. ..=total).contains(&used))
            );
            eprintln!(
                "GPU {} → {:?}, VRAM {:?}, température {:?}",
                a.name, a.utilization.value, a.vram_used.value, a.temperature.value
            );
        }
        for s in [second.disk, second.network] {
            assert_eq!(s.unit, Unit::BytesPerSecond);
            assert!(s.value.get().is_none_or(|v| v.is_finite() && v >= 0.));
            eprintln!("{} → {:?}", s.source, s.value);
        }
        for s in [second.disk_read_latency, second.disk_write_latency] {
            assert_eq!(s.unit, Unit::Milliseconds);
            assert!(s.value.get().is_none_or(|v| v.is_finite() && v >= 0.));
            eprintln!("{} → {:?}", s.source, s.value);
        }
        let loads = second
            .logical_cpus
            .iter()
            .filter_map(|p| p.usage.value.get())
            .collect::<Vec<_>>();
        assert!(loads.iter().all(|load| (0. ..=100.).contains(load)));
        eprintln!(
            "CPU logiques mesurés : {} / {}",
            loads.len(),
            second.logical_cpus.len()
        );
    }
}
