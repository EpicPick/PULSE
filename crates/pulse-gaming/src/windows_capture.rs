use super::{MAX_BYTES, Report, read_filtered};
use pulse_domain::ProcessKey;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn with_executable<T>(
    override_path: Option<&Path>,
    cancel: &AtomicBool,
    run: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Capture annulée.".into());
    }
    if let Some(path) = override_path {
        return run(path);
    }
    let bytes =
        super::BUNDLED.ok_or("PresentMon n’est pas inclus dans ce build de développement.")?;
    let dir = tempfile::Builder::new()
        .prefix("pulse-cli-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let exe = dir.path().join("PresentMon-2.3.1-x64.exe");
    // The release package embeds the official, hash-verified executable; no PATH lookup.
    let mut file =
        File::create_new(&exe).map_err(|e| format!("Préparation de PresentMon : {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("Préparation de PresentMon : {e}"))?;
    drop(file);
    fs::write(
        dir.path().join("PresentMon-LICENSE.txt"),
        super::PRESENTMON_LICENSE,
    )
    .map_err(|e| e.to_string())?;
    run(&exe)
}
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_TIMEOUT},
        System::Diagnostics::Etw::{
            CONTROLTRACE_HANDLE, ControlTraceW, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_PROPERTIES,
        },
        System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        System::Threading::{
            CREATE_NO_WINDOW, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, WaitForSingleObject,
        },
        UI::WindowsAndMessaging::{
            GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
            GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        },
    },
    core::PCWSTR,
};

struct Target(HANDLE);
impl Drop for Target {
    fn drop(&mut self) {
        // SAFETY: this instance exclusively owns the handle returned by OpenProcess.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl Target {
    fn identify(pid: u32) -> Result<(Self, ProcessKey), String> {
        // SAFETY: requests read-only identity and wait rights for the supplied PID.
        let target = Self(
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            }
            .map_err(|e| format!("Processus non accessible : {e}"))?,
        );
        let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        // SAFETY: live handle and correctly sized writable output values.
        unsafe { GetProcessTimes(target.0, &mut created, &mut exit, &mut kernel, &mut user) }
            .map_err(|e| format!("Identité du processus illisible : {e}"))?;
        let creation = u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime);
        Ok((
            target,
            ProcessKey {
                pid,
                created_at_ticks: creation,
            },
        ))
    }
    fn open(key: ProcessKey) -> Result<Self, String> {
        let (target, observed) = Self::identify(key.pid)?;
        if observed != key || !target.running() {
            return Err(
                "Le processus choisi a changé ou s’est arrêté. Sélectionnez-le de nouveau.".into(),
            );
        }
        Ok(target)
    }
    fn running(&self) -> bool {
        // SAFETY: this handle supports waiting; the zero timeout never blocks.
        (unsafe { WaitForSingleObject(self.0, 0) }) == WAIT_TIMEOUT
    }
}

fn foreground_pid() -> Option<u32> {
    // SAFETY: window handles are local, used only for read-only metadata; the out buffers are valid.
    unsafe {
        let w = GetForegroundWindow();
        if w.0.is_null()
            || w == GetDesktopWindow()
            || w == GetShellWindow()
            || IsIconic(w).as_bool()
            || !IsWindowVisible(w).as_bool()
        {
            return None;
        }
        let mut class = [0; 128];
        let n = GetClassNameW(w, &mut class);
        if n <= 0
            || matches!(
                String::from_utf16_lossy(&class[..n as usize]).as_str(),
                "Progman"
                    | "WorkerW"
                    | "Shell_TrayWnd"
                    | "Shell_SecondaryTrayWnd"
                    | "MultitaskingViewFrame"
                    | "TaskSwitcherWnd"
                    | "XamlExplorerHostIslandWindow"
                    | "CabinetWClass"
            )
        {
            return None;
        }
        let mut pid = 0;
        if GetWindowThreadProcessId(w, Some(&mut pid)) == 0 || pid <= 4 || pid == std::process::id()
        {
            return None;
        }
        Some(pid)
    }
}

pub fn auto(executable: &Path, seconds: u32, cancel: &AtomicBool) -> Result<Report, String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut seen = None;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) {
            return Err("Capture annulée.".into());
        }
        let now = Instant::now();
        let pid = foreground_pid();
        match (seen, pid) {
            (Some((old, since)), Some(pid)) if old == pid => {
                if now.duration_since(since) >= Duration::from_millis(750) {
                    let (_target, key) = Target::identify(pid)?;
                    return capture(executable, key, seconds, true, cancel);
                }
            }
            (_, Some(pid)) => seen = Some((pid, now)),
            (_, None) => seen = None,
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(
        "Aucune fenêtre détectée. Activez la capture auto, puis revenez directement dans le jeu."
            .into(),
    )
}

#[derive(Default)]
struct Foreground {
    open: Option<f64>,
    last: Option<f64>,
    windows: Vec<[f64; 2]>,
}
impl Foreground {
    fn observe(&mut self, at: f64, focused: bool) {
        if focused {
            self.open.get_or_insert(at);
            self.last = Some(at);
        } else if let (Some(start), Some(end)) = (self.open.take(), self.last.take())
            && end > start
        {
            // Keep only the portion bracketed by two foreground observations.
            self.windows.push([start, end]);
        }
    }
}

fn qpc_ms(frequency: i64) -> Result<f64, String> {
    let mut count = 0;
    // SAFETY: the API writes one i64 to a valid output pointer.
    unsafe { QueryPerformanceCounter(&mut count) }.map_err(|e| format!("Horloge QPC : {e}"))?;
    Ok(count as f64 * 1000. / frequency as f64)
}

struct CaptureChild(Child, String);
impl Drop for CaptureChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(),Ok(Some(exit)) if exit.success()) {
            // Only our own helper, never the target process.
            let _ = self.0.kill();
            let _ = self.0.wait();
            stop_trace(&self.1);
        }
    }
}

fn stop_trace(session: &str) {
    #[repr(C)]
    struct TraceBuffer {
        properties: EVENT_TRACE_PROPERTIES,
        names: [u16; 4096],
    }
    let mut buffer = TraceBuffer {
        properties: Default::default(),
        names: [0; 4096],
    };
    buffer.properties.Wnode.BufferSize = std::mem::size_of::<TraceBuffer>() as u32;
    buffer.properties.LoggerNameOffset = std::mem::offset_of!(TraceBuffer, names) as u32;
    buffer.properties.LogFileNameOffset = buffer.properties.LoggerNameOffset + 4096;
    let name: Vec<u16> = session.encode_utf16().chain([0]).collect();
    // SAFETY: the named session is unique to this capture; the aligned buffer includes both output strings.
    unsafe {
        let _ = ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name.as_ptr()),
            &mut buffer.properties,
            EVENT_TRACE_CONTROL_STOP,
        );
    }
}

fn command(exe: &Path, pid: u32, seconds: u32, session: &str, output: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args([
        "--process_id",
        &pid.to_string(),
        "--timed",
        &seconds.to_string(),
        "--terminate_after_timed",
        "--terminate_on_proc_exit",
        "--no_console_stats",
        "--no_track_input",
        "--no_track_gpu",
        "--v2_metrics",
        "--session_name",
        session,
        "--output_file",
    ])
    .arg(output)
    .creation_flags(CREATE_NO_WINDOW.0);
    cmd
}

pub fn capture(
    executable: &Path,
    key: ProcessKey,
    seconds: u32,
    foreground_only: bool,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    if !(5..=120).contains(&seconds) {
        return Err("La durée doit être comprise entre 5 et 120 s.".into());
    }
    if !executable.is_absolute()
        || !executable
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err("Choisissez le fichier .exe du CLI officiel PresentMon.".into());
    }
    let executable = executable
        .canonicalize()
        .map_err(|e| format!("PresentMon introuvable : {e}"))?;
    if !executable.is_file() {
        return Err("Le chemin choisi n’est pas un exécutable PresentMon.".into());
    }
    let target = Target::open(key)?;
    let mut frequency = 0;
    if foreground_only {
        // SAFETY: writable output, used only after success and validation.
        unsafe { QueryPerformanceFrequency(&mut frequency) }
            .map_err(|e| format!("Horloge QPC : {e}"))?;
        if frequency <= 0 {
            return Err("Fréquence QPC indisponible.".into());
        }
    }
    let mut foreground = Foreground::default();
    let temp = tempfile::Builder::new()
        .prefix("pulse-presentmon-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let session = format!("PULSE-{}-{unique}", std::process::id());
    let output = temp.path().join("frames.csv");
    let stderr = temp.path().join("presentmon-error.log");
    let log = File::create_new(&stderr).map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Acquire) {
        return Err("Opération annulée.".into());
    }
    let mut cmd = command(&executable, key.pid, seconds, &session, &output);
    if foreground_only {
        cmd.arg("--qpc_time_ms");
    }
    let mut child = CaptureChild(
        cmd.current_dir(temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .map_err(|e| format!("Impossible de lancer PresentMon : {e}"))?,
        session,
    );
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds) + 12);
    let exit = loop {
        if cancel.load(Ordering::Acquire) || !target.running() {
            return Err(if cancel.load(Ordering::Relaxed) {
                "Capture annulée.".into()
            } else {
                "Le processus a quitté avant la fin de la capture. Aucun résultat ambigu n’est conservé.".into()
            });
        }
        if foreground_only {
            let at = qpc_ms(frequency)?;
            foreground.observe(at, foreground_pid() == Some(key.pid));
        }
        if let Some(exit) = child.0.try_wait().map_err(|e| e.to_string())? {
            break exit;
        }
        if Instant::now() >= deadline {
            return Err("PresentMon n’a pas terminé dans le délai autorisé.".into());
        }
        if oversized(&output, MAX_BYTES)? || oversized(&stderr, 1024 * 1024)? {
            return Err("Capture interrompue : limite de taille atteinte.".into());
        }
        thread::sleep(Duration::from_millis(if foreground_only {
            50
        } else {
            100
        }));
    };
    if !target.running() {
        return Err("Le processus ciblé s’est arrêté ; la capture a été écartée.".into());
    }
    if !exit.success() || !output.is_file() {
        let mut bytes = Vec::new();
        if let Ok(log) = File::open(stderr) {
            let _ = log.take(4096).read_to_end(&mut bytes);
        }
        let message = String::from_utf8_lossy(&bytes);
        let reason = message.trim();
        return Err(format!(
            "PresentMon n’a pas produit de capture exploitable ({exit}). {}{}",
            if reason.is_empty() { "" } else { reason },
            " Vérifiez le CLI 2.x et les droits ETW du compte (Utilisateurs des journaux de performances). PULSE ne s’élève pas."
        ));
    }
    foreground.observe(0., false);
    read_filtered(
        &output,
        Some(key.pid),
        foreground_only.then_some(&foreground.windows),
        cancel,
    )
}

fn oversized(path: &Path, limit: u64) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(info) => Ok(info.len() > limit),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_loss_splits_the_capture_and_drops_unbracketed_time() {
        let mut focus = Foreground::default();
        for (t, on) in [
            (100., true),
            (150., true),
            (200., false),
            (250., true),
            (300., true),
            (350., false),
            (400., true),
        ] {
            focus.observe(t, on);
        }
        focus.observe(450., false);
        assert_eq!(focus.windows, [[100., 150.], [250., 300.]]);
    }
    #[test]
    fn capture_uses_explicit_pid_and_does_not_track_input_or_request_admin() {
        let c = command(
            Path::new("presentmon.exe"),
            123,
            30,
            "pulse-test",
            Path::new("path with spaces.csv"),
        );
        let args = c
            .get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|a| a == ["--process_id", "123"]));
        assert!(args.contains(&"--no_track_input".into()));
        assert!(!args.contains(&"--restart_as_admin".into()));
        assert!(!args.contains(&"--stop_existing_session".into()));
        assert_eq!(args.last().unwrap(), "path with spaces.csv");
    }

    #[test]
    #[ignore = "requires an ETL fixture and bundled PresentMon or PULSE_TEST_PRESENTMON"]
    fn official_cli_decodes_a_real_trace_without_a_window() {
        let external = std::env::var_os("PULSE_TEST_PRESENTMON");
        let etl = std::env::var_os("PULSE_TEST_ETL").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let csv = tmp.path().join("trace.csv");
        let (result, exe) = with_executable(
            external.as_deref().map(Path::new),
            &AtomicBool::default(),
            |exe| {
                Command::new(exe)
                    .args(["--etl_file"])
                    .arg(etl)
                    .args([
                        "--v2_metrics",
                        "--qpc_time_ms",
                        "--no_console_stats",
                        "--no_track_input",
                        "--no_track_gpu",
                        "--output_file",
                    ])
                    .arg(&csv)
                    .creation_flags(CREATE_NO_WINDOW.0)
                    .output()
                    .map(|output| (output, exe.to_path_buf()))
                    .map_err(|e| e.to_string())
            },
        )
        .unwrap();
        if external.is_none() {
            assert!(
                !exe.exists(),
                "the bundled helper must be cleaned up after use"
            );
        }
        assert!(result.status.success(), "{:?}", result);
        let report =
            read_filtered(&csv, None, Some(&[[0., f64::MAX]]), &AtomicBool::default()).unwrap();
        assert_eq!(report.metric, super::super::Metric::Displayed);
        assert!(report.intervals >= 100 && report.fps > 0.);
        eprintln!(
            "PresentMon réel : {} images, {:.1} FPS, P99 {:.2} ms",
            report.intervals, report.fps, report.p99_ms
        );
    }
}
