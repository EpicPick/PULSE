use pulse_domain::ProcessKey;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(windows)]
mod windows_capture;

include!(concat!(env!("OUT_DIR"), "/presentmon.rs"));
pub const PRESENTMON_LICENSE: &str = include_str!("../../../third-party/PresentMon-LICENSE.txt");

pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ROWS: usize = 180_000;
const MAX_STREAMS: usize = 512;

#[derive(Debug)]
pub enum Request {
    Import(PathBuf),
    Capture {
        executable: Option<PathBuf>,
        process: ProcessKey,
        seconds: u32,
    },
    Auto {
        executable: Option<PathBuf>,
        seconds: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Displayed,
    Present,
    AppFrame,
}
impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Displayed => "Images affichées · DisplayedTime",
            Self::Present => "Présentations · MsBetweenPresents (pas le débit écran)",
            Self::AppFrame => "Cadence de l’application · FrameTime (pas le débit écran)",
        }
    }
}

#[derive(Debug)]
pub struct Report {
    pub name: String,
    pub pid: u32,
    pub swapchain: u64,
    pub streams: usize,
    pub metric: Metric,
    pub intervals: usize,
    pub total_ms: f64,
    pub fps: f64,
    pub low_one: Option<f64>,
    pub p99_ms: f64,
    pub foreground_only: bool,
    /// Peak per bucket, preserving stalls when downsampling.
    pub chart: Vec<[f64; 2]>,
}

pub fn run(request: Request, cancel: &AtomicBool) -> Result<Report, String> {
    match request {
        Request::Import(path) => read_file(&path, None, cancel),
        #[cfg(windows)]
        Request::Capture {
            executable,
            process,
            seconds,
        } => windows_capture::with_executable(executable.as_deref(), cancel, |exe| {
            windows_capture::capture(exe, process, seconds, false, cancel)
        }),
        #[cfg(windows)]
        Request::Auto {
            executable,
            seconds,
        } => windows_capture::with_executable(executable.as_deref(), cancel, |exe| {
            windows_capture::auto(exe, seconds, cancel)
        }),
        #[cfg(not(windows))]
        Request::Capture { .. } | Request::Auto { .. } => {
            Err("La capture PresentMon requiert Windows.".into())
        }
    }
}

fn read_file(path: &Path, pid: Option<u32>, cancel: &AtomicBool) -> Result<Report, String> {
    read_filtered(path, pid, None, cancel)
}

fn read_filtered(
    path: &Path,
    pid: Option<u32>,
    foreground: Option<&[[f64; 2]]>,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Opération annulée.".into());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("CSV illisible : {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("Le CSV dépasse la limite de 32 Mio.".into());
    }
    analyze_filtered(file.take(MAX_BYTES + 1), pid, foreground, cancel)
}

#[derive(Default)]
struct Chain {
    name: String,
    displayed: Vec<f64>,
    presents: Vec<f64>,
    app_frames: Vec<f64>,
}

#[cfg(test)]
fn analyze(input: impl Read, pid: Option<u32>, cancel: &AtomicBool) -> Result<Report, String> {
    analyze_filtered(input, pid, None, cancel)
}

fn analyze_filtered(
    input: impl Read,
    pid: Option<u32>,
    foreground: Option<&[[f64; 2]]>,
    cancel: &AtomicBool,
) -> Result<Report, String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(input);
    let headers = reader
        .headers()
        .map_err(|e| format!("En-tête PresentMon invalide : {e}"))?;
    let find = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim_start_matches('\u{feff}').eq_ignore_ascii_case(name))
    };
    let process = find("ProcessID").ok_or("Le CSV n’a pas de colonne ProcessID.")?;
    let swapchain =
        find("SwapChainAddress").ok_or("Le CSV n’a pas de colonne SwapChainAddress.")?;
    let app = find("Application");
    let displayed = find("DisplayedTime");
    let presents = find("MsBetweenPresents");
    let app_frames = find("FrameTime");
    let qpc = find("CPUStartQPCTime").or_else(|| find("CPUStartQPCTimeInMs"));
    let latency = find("DisplayLatency");
    if foreground.is_some() && qpc.is_none() {
        return Err(
            "Horodatage QPC absent : vérifiez que vous utilisez le CLI officiel PresentMon 2.3.1."
                .into(),
        );
    }
    if displayed.is_none() && presents.is_none() && app_frames.is_none() {
        return Err("Ce CSV ne contient aucune colonne de durée PresentMon reconnue (DisplayedTime, MsBetweenPresents, FrameTime).".into());
    }
    let mut chains = BTreeMap::<(u32, u64), Chain>::new();
    let mut row = csv::StringRecord::new();
    let mut count = 0;
    while reader
        .read_record(&mut row)
        .map_err(|e| format!("CSV PresentMon invalide : {e}"))?
    {
        count += 1;
        if count > MAX_ROWS || reader.position().byte() > MAX_BYTES {
            return Err("Capture trop volumineuse pour cette version de PULSE.".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Opération annulée.".into());
        }
        let Some(id) = row
            .get(process)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| pid.is_none_or(|p| p == n))
        else {
            continue;
        };
        let Some(address) = row.get(swapchain).and_then(|s| {
            s.strip_prefix("0x")
                .or_else(|| s.strip_prefix("0X"))
                .map_or_else(
                    || s.parse::<u64>().ok(),
                    |v| u64::from_str_radix(v, 16).ok(),
                )
        }) else {
            continue;
        };
        let interval = |idx: Option<usize>| {
            idx.and_then(|i| row.get(i))
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| (0.000_001..=120_000.).contains(v))
        };
        let mut display = interval(displayed);
        let mut present = interval(presents);
        let mut app_frame = interval(app_frames);
        if let Some(windows) = foreground {
            let number = |idx: Option<usize>| {
                idx.and_then(|i| row.get(i))
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v >= 0.)
            };
            let Some(start) = number(qpc) else {
                continue;
            };
            display = display.filter(|&ms| {
                number(latency).is_some_and(|delay| inside(windows, start + delay, ms))
            });
            app_frame = app_frame.filter(|&ms| inside(windows, start, ms));
            // CPUStartQPCTime is not the timestamp of the Present() call.
            present = None;
        }
        if display.is_none() && present.is_none() && app_frame.is_none() {
            continue;
        }
        let key = (id, address);
        if !chains.contains_key(&key) && chains.len() >= MAX_STREAMS {
            return Err("Le CSV contient trop de flux distincts.".into());
        }
        let chain = chains.entry(key).or_insert_with(|| Chain {
            name: app
                .and_then(|i| row.get(i))
                .and_then(|name| name.rsplit(['\\', '/']).next())
                .unwrap_or("Processus")
                .chars()
                .take(128)
                .collect(),
            ..Default::default()
        });
        chain.displayed.extend(display);
        chain.presents.extend(present);
        chain.app_frames.extend(app_frame);
    }
    let streams = chains.len();
    let score = |x: &Chain| {
        let n = x.displayed.len();
        (
            n >= 2,
            if n >= 2 {
                n
            } else {
                x.presents.len().max(x.app_frames.len())
            },
        )
    };
    let ((pid,swapchain),chain)=chains.into_iter().max_by(|(a,x),(b,y)| {
        // Prefer a display stream; never merge separate swap chains.
        score(x).cmp(&score(y)).then_with(||b.cmp(a))
    }).ok_or(if foreground.is_some() {
        "Aucune image mesurable au premier plan. Réessayez en revenant directement dans le jeu, ou choisissez son processus manuellement."
    } else { "Aucune image mesurable : vérifiez que le jeu tourne et que PresentMon a accès à la trace." })?;
    let (metric, frames) = if chain.displayed.len() >= 2 {
        (Metric::Displayed, chain.displayed)
    } else if chain.presents.len() >= 2 {
        (Metric::Present, chain.presents)
    } else {
        (Metric::AppFrame, chain.app_frames)
    };
    if frames.len() < 2 {
        return Err("Au moins deux intervalles valides sont nécessaires.".into());
    }
    let mut report = summarize(chain.name, pid, swapchain, streams, metric, frames);
    report.foreground_only = foreground.is_some();
    Ok(report)
}

fn inside(windows: &[[f64; 2]], start: f64, duration: f64) -> bool {
    windows
        .get(windows.partition_point(|&[_, end]| end < start))
        .is_some_and(|&[first, end]| first <= start && start + duration <= end)
}

fn summarize(
    name: String,
    pid: u32,
    swapchain: u64,
    streams: usize,
    metric: Metric,
    frames: Vec<f64>,
) -> Report {
    let intervals = frames.len();
    let total_ms = frames.iter().sum::<f64>();
    let mut time = 0.;
    let chart = frames
        .chunks(intervals.div_ceil(450))
        .map(|bucket| {
            let mut peak = [time, 0.];
            for &ms in bucket {
                time += ms;
                if ms > peak[1] {
                    peak = [time, ms];
                }
            }
            peak
        })
        .collect();
    let mut sorted = frames;
    sorted.sort_unstable_by(f64::total_cmp);
    let p99_ms = sorted[(intervals * 99).div_ceil(100) - 1];
    let low_one = (intervals >= 100).then(|| {
        let n = intervals.div_ceil(100);
        1000. * n as f64 / sorted[intervals - n..].iter().sum::<f64>()
    });
    Report {
        name,
        pid,
        swapchain,
        streams,
        metric,
        intervals,
        total_ms,
        fps: 1000. * intervals as f64 / total_ms,
        low_one,
        p99_ms,
        foreground_only: false,
        chart,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_background_frames_and_intervals_spanning_a_focus_change() {
        let data=b"ProcessID,SwapChainAddress,CPUStartQPCTimeInMs,DisplayedTime,DisplayLatency\n10,0x1,100,10,5\n10,0x1,115,10,1\n10,0x1,160,10,5\n10,0x1,200,20,2\n20,0x1,205,1,0\n";
        let report = analyze_filtered(
            &data[..],
            Some(10),
            Some(&[[100., 120.], [200., 230.]]),
            &AtomicBool::default(),
        )
        .unwrap();
        assert!(report.foreground_only);
        assert_eq!(
            (report.metric, report.intervals, report.total_ms),
            (Metric::Displayed, 2, 30.)
        );
    }
    #[test]
    fn display_intervals_do_not_mix_swapchains_or_dropped_frames() {
        let data=b"Application,ProcessID,SwapChainAddress,DisplayedTime,MsBetweenPresents\n\"game, test.exe\",10,0x1,10,8\n\"game, test.exe\",10,0x1,NA,8\n\"game, test.exe\",10,0x1,20,8\ngame.exe,10,0x2,1,1\nother.exe,20,0x1,3,3\n";
        let r = analyze(&data[..], Some(10), &AtomicBool::default()).unwrap();
        assert_eq!(
            (r.metric, r.intervals, r.total_ms, r.streams),
            (Metric::Displayed, 2, 30., 2)
        );
        assert_eq!(r.name, "game, test.exe");
        assert_eq!(r.low_one, None);
    }

    #[test]
    fn one_percent_low_uses_the_slowest_intervals_and_keeps_spikes() {
        let mut frames = vec![10.; 1000];
        frames[999] = 100.;
        let r = summarize("game".into(), 1, 1, 1, Metric::Present, frames);
        assert_eq!(r.p99_ms, 10.);
        assert!((r.low_one.unwrap() - 10000. / 190.).abs() < 0.001);
        assert!(r.chart.iter().any(|p| p[1] == 100.));
    }

    #[test]
    fn legacy_present_csv_is_labelled_as_presentation_not_display() {
        let data=b"ProcessID,SwapChainAddress,MsBetweenPresents,Application\n10,0x2,20,game.exe\n10,0x2,20,game.exe";
        let r = analyze(&data[..], None, &AtomicBool::default()).unwrap();
        assert_eq!((r.metric, r.fps), (Metric::Present, 50.));
    }

    #[test]
    fn v2_non_displayed_fallback_is_labelled_as_application_cadence() {
        let data=b"\xef\xbb\xbfApplication,ProcessID,SwapChainAddress,FrameTime,DisplayedTime\ngame.exe,10,0x2,25,NA\ngame.exe,10,0x2,25,NA";
        let r = analyze(&data[..], None, &AtomicBool::default()).unwrap();
        assert_eq!((r.metric, r.fps), (Metric::AppFrame, 40.));
    }
}
