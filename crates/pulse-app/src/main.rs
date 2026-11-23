#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod language;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use pulse_history::{SessionCommand, SessionReply, SessionWriter};
use pulse_runtime::{Mailbox, Worker};
use pulse_ui::State;
use rune::{App, AppSpec, Tx};

struct GamingWorker {
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _worker: Worker,
}
impl Drop for GamingWorker {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

fn main() {
    if let Err(e) = run() {
        pulse_ui::with_language(language::load().resolve(language::system()), || {
            let detail = e.to_string();
            let message = pulse_ui::translate_message(&detail);
            eprintln!("PULSE: {message}");
            #[cfg(all(windows, not(debug_assertions)))]
            pulse_platform_windows::show_startup_error(&message);
        });
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut record = None;
    let mut replay = None;
    let mut retain = None;
    let mut export = None;
    let mut include_processes = false;
    let mut args = Vec::new();
    let mut input = std::env::args().skip(1);
    while let Some(arg) = input.next() {
        match arg.as_str() {
            "--live" => {} // compatibility; local monitoring is now the default
            "--with-processes" => include_processes = true,
            "--record" => {
                record = Some(PathBuf::from(
                    input.next().ok_or("Chemin manquant après --record")?,
                ))
            }
            "--replay" => {
                replay = Some(PathBuf::from(
                    input.next().ok_or("Chemin manquant après --replay")?,
                ))
            }
            "--retain" => {
                retain = Some(
                    input
                        .next()
                        .ok_or("Nombre manquant après --retain")?
                        .parse::<usize>()?,
                )
            }
            "--export-csv" => {
                export = Some(PathBuf::from(
                    input.next().ok_or("Chemin manquant après --export-csv")?,
                ))
            }
            #[cfg(debug_assertions)]
            "--snap-language" => {
                args.push(arg);
                args.push(
                    input
                        .next()
                        .ok_or("Missing language after --snap-language")?,
                );
            }
            _ => args.push(arg),
        }
    }
    if !args.is_empty() {
        #[cfg(debug_assertions)]
        if args.iter().any(|arg| arg == "--snap")
            && replay.is_none()
            && record.is_none()
            && retain.is_none()
            && export.is_none()
            && !include_processes
        {
            return capture(args);
        }
        return Err("Utilisation : PULSE.exe [--record nouveau.sqlite [--retain N]] ou PULSE.exe --replay session.sqlite".into());
    }
    if replay.is_some() && record.is_some()
        || (retain.is_some() || include_processes) && record.is_none()
    {
        return Err(
            "L’enregistrement et la relecture sont exclusifs ; --retain et --with-processes exigent --record.".into(),
        );
    }
    if let Some(destination) = export {
        let session = replay
            .as_ref()
            .ok_or("--export-csv exige --replay session.sqlite")?;
        return pulse_history::export_csv(session, &destination);
    }
    let points = replay
        .as_ref()
        .map(|path| pulse_history::replay_with_contributors(path))
        .transpose()?;
    let mut writer = SessionWriter::default();
    let initial_recording = if let Some(path) = record {
        let status = writer.apply(SessionCommand::Start {
            path,
            retain: retain.unwrap_or(3600),
            include_processes,
        });
        if !status.recording {
            return Err(status.message.into());
        }
        Some(status)
    } else {
        None
    };
    let native = cfg!(windows);
    #[cfg(not(windows))]
    if points.is_none() {
        return Err("Le collecteur système requiert Windows".into());
    }
    let mailbox = Mailbox::default();
    let outcomes = Mailbox::default();
    let session_replies = Mailbox::default();
    let gaming_replies = Mailbox::default();
    let (gaming_commands, gaming_requests) = std::sync::mpsc::sync_channel(1);
    let gaming_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (session_commands, session_requests) = std::sync::mpsc::sync_channel(1);
    let replay_mode = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(points.is_some()));
    let network_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gpu_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let storage_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let storage = Mailbox::default();
    let (commands, requests) = pulse_actions::channel();
    let mut spec = AppSpec::new("PULSE", (1440., 900.), system_state, pulse_ui::view);
    spec.min_size = Some((1024., 640.));
    spec.poll = Some(poll);
    let inbox = mailbox.clone();
    let replies = outcomes.clone();
    let session_inbox = session_replies.clone();
    let gaming_inbox = gaming_replies.clone();
    let gaming_ui = gaming_cancel.clone();
    let replay_ui = replay_mode.clone();
    let network_ui = network_requested.clone();
    let gpu_ui = gpu_requested.clone();
    let storage_ui = storage_requested.clone();
    let storage_inbox = storage.clone();
    let app = App::new(spec).args(Vec::new()).init(move || {
        #[cfg(windows)]
        let mut state = system_state();
        #[cfg(not(windows))]
        let mut state = State::replay(points.unwrap());
        state.system_language = language::system();
        state.language_preference = language::load();
        state.save_language = Some(language::save);
        state.inbox = native.then_some(inbox);
        state.command = native.then_some(commands);
        state.replies = native.then_some(replies);
        state.network_requested = native.then_some(network_ui);
        state.gpu_requested = native.then_some(gpu_ui);
        state.storage_requested = native.then_some(storage_ui);
        state.storage_inbox = native.then_some(storage_inbox);
        state.session_commands = native.then_some(session_commands);
        state.session_replies = native.then_some(session_inbox);
        state.gaming.commands = native.then_some(gaming_commands);
        state.gaming.replies = native.then_some(gaming_inbox);
        state.gaming.cancel = native.then_some(gaming_ui);
        state.replay_mode = native.then_some(replay_ui);
        if let Some(reply) = initial_recording {
            state.apply_session(reply);
        }
        #[cfg(windows)]
        if let Some(points) = points {
            state.apply_session(SessionReply {
                recording: false,
                message: "Session ouverte en lecture seule.".into(),
                replay: Some(points),
            });
        }
        state
    });
    #[cfg(windows)]
    let app = if let Some(placement) = pulse_platform_windows::secondary_window() {
        app.window_options(rune::WindowOptions {
            position: Some(placement.position),
            physical_size: Some(placement.size),
            min_size: Some((640., 400.)),
            ..Default::default()
        })
        .open_inactive()
    } else {
        app
    };
    let handle = app.handle();
    let worker_handle = handle.clone();
    let mut worker = None;
    let mut gaming_worker = None;
    #[cfg(windows)]
    let mut storage_worker = None;
    let mut gaming_requests = Some(gaming_requests);
    let mut requests = Some(requests);
    let mut session_requests = Some(session_requests);
    let mut writer = Some(writer);
    let app = app.on_ready(move |_| {
        if !native || worker.is_some() {
            return;
        }
        #[cfg(windows)]
        {
            let requested = storage_requested.clone();
            let replay = replay_mode.clone();
            let handle = worker_handle.clone();
            let output = storage.clone();
            match Worker::spawn(move || {
                let mut sampler = pulse_platform_windows::Storage::default();
                move || {
                    if handle.is_visible()
                        && requested.load(std::sync::atomic::Ordering::Relaxed)
                        && !replay.load(std::sync::atomic::Ordering::Acquire)
                    {
                        output.publish(sampler.sample());
                        handle.wake();
                    }
                    Duration::from_secs(1)
                }
            }) {
                Ok(thread) => {
                    let _ = storage_worker.insert(thread);
                }
                Err(e) => storage.publish(pulse_domain::StorageSnapshot {
                    elapsed_ms: 0,
                    drives: vec![],
                    omitted: false,
                    note: Some(format!("Collecteur Stockage indisponible : {e}")),
                }),
            }
        }
        let incoming = gaming_requests.take().expect("worker starts once");
        let cancel = gaming_cancel.clone();
        let notify = worker_handle.clone();
        let replies = gaming_replies.clone();
        match Worker::spawn(move || {
            move || {
                if let Ok(request) = incoming.try_recv() {
                    let result = pulse_gaming::run(request, &cancel).map(std::sync::Arc::new);
                    replies.publish(result);
                    notify.wake();
                }
                Duration::from_millis(200)
            }
        }) {
            Ok(thread) => {
                let _ = gaming_worker.insert(GamingWorker {
                    cancel: gaming_cancel.clone(),
                    _worker: thread,
                });
            }
            Err(e) => {
                gaming_replies.publish(Err(format!(
                    "Impossible de démarrer le service PresentMon : {e}"
                )));
                worker_handle.wake();
            }
        };
        let mailbox = mailbox.clone();
        let handle = worker_handle.clone();
        let start = Instant::now();
        let requests = requests.take().expect("worker starts once");
        let outcomes = outcomes.clone();
        let session_replies = session_replies.clone();
        let session_requests = session_requests.take().expect("worker starts once");
        let mut writer = writer.take().expect("worker starts once");
        let replay_mode = replay_mode.clone();
        let network_requested = network_requested.clone();
        let gpu_requested = gpu_requested.clone();
        match Worker::spawn(move || {
            #[cfg(windows)]
            let mut collector = None;
            move || {
                if let Ok(command) = session_requests.try_recv() {
                    let reply = writer.apply(command);
                    if reply.replay.is_some() {
                        replay_mode.store(true, std::sync::atomic::Ordering::Release);
                        #[cfg(windows)]
                        {
                            collector = None;
                        }
                    }
                    session_replies.publish(reply);
                    handle.wake();
                    return Duration::from_millis(150);
                }
                if let Ok(command) = requests.try_recv() {
                    #[cfg(windows)]
                    outcomes.publish(pulse_platform_windows::execute(command));
                    #[cfg(not(windows))]
                    {
                        let _ = command;
                        outcomes.publish(pulse_actions::Outcome::Protected);
                    }
                }
                if replay_mode.load(std::sync::atomic::Ordering::Acquire) {
                    return Duration::from_millis(250);
                }
                #[cfg(windows)]
                let snapshot = {
                    let request = handle.is_visible()
                        && network_requested.load(std::sync::atomic::Ordering::Relaxed);
                    let gpu_detail = handle.is_visible()
                        && gpu_requested.load(std::sync::atomic::Ordering::Relaxed);
                    let mut snapshot = collector
                        .get_or_insert_with(pulse_platform_windows::Collector::default)
                        .sample_with_details(request, gpu_detail);
                    snapshot.elapsed_ms = start.elapsed().as_millis() as u64;
                    if snapshot.process_error.is_none() || !snapshot.processes.is_empty() {
                        snapshot.contributors =
                            Some(pulse_domain::top_contributors(&snapshot.processes).into());
                    }
                    snapshot
                };
                #[cfg(not(windows))]
                let snapshot: pulse_domain::Snapshot = unreachable!("native Windows only");
                if let Some(reply) = writer.append_with_contributors(
                    pulse_history::Point {
                        utc_ms: snapshot.utc_ms,
                        elapsed_ms: snapshot.elapsed_ms,
                        values: [
                            snapshot.cpu.value.get(),
                            snapshot.ram.value.get(),
                            snapshot.gpu.value.get(),
                        ],
                        rates: [snapshot.disk.value.get(), snapshot.network.value.get()],
                    },
                    snapshot.contributors.as_deref(),
                ) {
                    session_replies.publish(reply);
                }
                mailbox.publish(snapshot);
                handle.wake();
                Duration::from_secs(if handle.is_visible() { 1 } else { 4 })
            }
        }) {
            Ok(value) => worker = Some(value),
            Err(e) => {
                eprintln!("Collecteur PULSE : {e}");
                worker_handle.quit();
            }
        }
    });
    app.try_run()?;
    if let Some(e) = handle.window_error() {
        return Err(e.into());
    }
    Ok(())
}

fn system_state() -> State {
    #[cfg(windows)]
    {
        let mut state = State::new(false, pulse_platform_windows::pending());
        state.navigate(pulse_ui::Page::Processes);
        state
    }
    #[cfg(not(windows))]
    {
        unreachable!("native Windows only")
    }
}

#[cfg(debug_assertions)]
fn capture(mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    use pulse_ui::{LanguagePreference, Page};
    let preference = if let Some(index) = args.iter().position(|arg| arg == "--snap-language") {
        let value = args.remove(index + 1);
        args.remove(index);
        match value.as_str() {
            "fr" => LanguagePreference::French,
            "en" => LanguagePreference::English,
            _ => return Err("--snap-language accepts fr or en".into()),
        }
    } else {
        LanguagePreference::French
    };
    let mut spec = AppSpec::new("PULSE", (1440., 900.), State::demo, pulse_ui::view);
    spec.scenes = &[
        ("overview", |_| {}),
        ("monitor", |s| {
            let mut snapshot = pulse_testkit::snapshot(120);
            snapshot
                .adapters
                .push(pulse_testkit::monitor_adapter(snapshot.utc_ms));
            s.publish(std::sync::Arc::new(snapshot));
            s.navigate(Page::Monitor);
        }),
        ("processes", |s| {
            s.navigate(Page::Processes);
            let key = s.process(0).key;
            s.select_process(Some(key));
            for i in 121..=180 {
                s.publish(std::sync::Arc::new(pulse_testkit::snapshot(i)));
            }
        }),
        ("gpu", |s| s.navigate(Page::Gpu)),
        ("io", |s| {
            s.navigate(Page::Io);
            for t in 0..=60 {
                s.publish_storage(std::sync::Arc::new(pulse_testkit::storage(t)));
            }
        }),
        ("memory", |s| s.navigate(Page::Memory)),
        ("diagnostics", |s| s.navigate(Page::Diagnostics)),
        ("history", |s| {
            s.navigate(Page::History);
            s.session_commands = Some(std::sync::mpsc::sync_channel(1).0);
        }),
        ("gaming", |s| {
            s.navigate(Page::Gaming);
            s.gaming.commands = Some(std::sync::mpsc::sync_channel(1).0);
            s.gaming.bundled_presentmon = true;
        }),
        ("gaming-report", |s| {
            s.navigate(Page::Gaming);
            s.gaming.commands = Some(std::sync::mpsc::sync_channel(1).0);
            s.gaming.bundled_presentmon = true;
            let mut elapsed = 0.;
            let frames: Vec<_> = (0..3600)
                .map(|i| {
                    7.4 + 1.7 * (f64::from(i) * 0.143).sin().powi(2)
                        + if i % 193 == 0 { 24. } else { 0. }
                        + if (i / 120) % 5 == 2 { 3. } else { 0. }
                })
                .collect();
            let total: f64 = frames.iter().sum();
            let chart = frames
                .chunks(8)
                .map(|chunk| {
                    let mut peak = [elapsed, 0.];
                    for &ms in chunk {
                        elapsed += ms;
                        if ms > peak[1] {
                            peak = [elapsed, ms];
                        }
                    }
                    peak
                })
                .collect();
            let intervals = frames.len();
            let mut sorted = frames;
            sorted.sort_unstable_by(f64::total_cmp);
            let slow = intervals.div_ceil(100);
            s.gaming.report = Some(std::sync::Arc::new(pulse_gaming::Report {
                name: "Studio Render.exe".into(),
                pid: 4200,
                swapchain: 1,
                streams: 1,
                metric: pulse_gaming::Metric::Displayed,
                intervals,
                total_ms: total,
                fps: 1000. * intervals as f64 / total,
                p99_ms: sorted[(intervals * 99).div_ceil(100) - 1],
                low_one: Some(1000. * slow as f64 / sorted[intervals - slow..].iter().sum::<f64>()),
                foreground_only: true,
                chart,
            }));
        }),
        ("connections", |s| {
            use pulse_domain::{Availability, Endpoint, EndpointSnapshot, Transport};
            s.navigate(Page::Io);
            s.tab = 2;
            let mut snap = pulse_testkit::snapshot(120);
            snap.endpoints = Some(std::sync::Arc::new(EndpointSnapshot {
                utc_ms: snap.utc_ms,
                entries: [
                    (
                        Transport::Tcp,
                        "192.0.2.12:51902",
                        Some("203.0.113.7:443"),
                        4264,
                    ),
                    (Transport::Tcp, "127.0.0.1:8080", None, 4200),
                    (Transport::Tcp, "[::1]:4201", None, 4384),
                    (Transport::Udp, "192.0.2.12:5353", None, 5300),
                    (Transport::Udp, "[::1]:5355", None, 4700),
                ]
                .into_iter()
                .map(|(transport, local, remote, pid)| Endpoint {
                    transport,
                    local: local.parse().expect("fixture address"),
                    remote: remote.map(|ip| ip.parse().expect("fixture address")),
                    tcp_state: (transport == Transport::Tcp).then_some(if remote.is_some() {
                        5
                    } else {
                        2
                    }),
                    pid,
                })
                .collect(),
                tables: [
                    Availability::Available(2),
                    Availability::Available(1),
                    Availability::Available(1),
                    Availability::Available(1),
                ],
                omitted: 0,
            }));
            s.publish(std::sync::Arc::new(snap));
        }),
    ];
    App::new(spec)
        .args(args)
        .init(move || {
            let mut state = State::demo();
            state.language_preference = preference;
            state
        })
        .try_run()?;
    Ok(())
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    if let Some(requested) = &s.storage_requested {
        requested.store(
            !s.paused && s.page == pulse_ui::Page::Io && s.tab == 0,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    if let Some(requested) = &s.network_requested {
        requested.store(
            !s.paused && s.page == pulse_ui::Page::Io && s.tab == 2,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    if let Some(requested) = &s.gpu_requested {
        requested.store(
            !s.paused
                && ((s.page == pulse_ui::Page::Gpu && s.tab == 3)
                    || s.page == pulse_ui::Page::Processes),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    let mut changed = false;
    if !s.paused
        && let Some(data) = s.storage_inbox.as_ref().and_then(Mailbox::take)
    {
        s.publish_storage(data);
        changed = true;
    }
    if let Some(reply) = s.gaming.replies.as_ref().and_then(Mailbox::take) {
        s.gaming.busy = false;
        match &*reply {
            Ok(report) => {
                s.gaming.report = Some(report.clone());
                s.gaming.status = Some("Analyse terminée.".into());
            }
            Err(message) => s.gaming.status = Some(message.clone()),
        }
        changed = true;
    }
    if let Some(reply) = s.session_replies.as_ref().and_then(Mailbox::take) {
        s.apply_session(std::sync::Arc::unwrap_or_clone(reply));
        changed = true;
    }
    if let Some(reply) = s.replies.as_ref().and_then(Mailbox::take) {
        s.action_status = Some(reply.message());
        s.action_pending = false;
        changed = true;
    }
    if !s.paused
        && let Some(snapshot) = s.inbox.as_ref().and_then(Mailbox::take)
    {
        s.publish(snapshot);
        tx.instant();
        changed = true;
    }
    changed
}
