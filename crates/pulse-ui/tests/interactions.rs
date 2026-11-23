use pulse_ui::{Page, State, view};
use rune::{Env, Runtime};

#[test]
fn storage_switches_detail_between_disks_and_never_invents_smart() {
    let mut state = State::demo();
    state.navigate(Page::Io);
    state.publish_storage(std::sync::Arc::new(pulse_testkit::storage(50)));
    let mut rt = Runtime::new(
        state,
        view,
        (1440., 900.),
        Env {
            system_dark: true,
            scale: 1.,
        },
    );
    rt.frame(0.);
    assert!(rt.find("storage-inventory").is_some());
    assert!(format!("{:?}", rt.elements()).contains("Windows"));
    tap(&mut rt, "storage-disk-2");
    let screen = format!("{:?}", rt.elements());
    assert!(
        screen.contains("E:") && screen.contains("Sauvegarde") && screen.contains("Non exposée")
    );
    assert!(!screen.contains("C:  Windows"));
}

#[test]
fn cpu_detail_shows_the_cpu_sensor_and_no_gpu_summary() {
    let mut snapshot = pulse_testkit::snapshot(120);
    snapshot.monitor.cpu_thermal =
        pulse_domain::Availability::Available(pulse_domain::CpuThermal {
            celsius: 61.2,
            label: "CPU (Tctl/Tdie)",
        });
    let mut s = State::new(false, snapshot);
    s.navigate(Page::Monitor);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: true,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "direct-cpu");
    let rendered = format!("{:?}", rt.elements());
    assert!(rendered.contains("61.2 °C"));
    assert!(!rendered.contains("Charge graphique") && !rendered.contains("Charge des ressources"));
}

#[test]
fn monitor_shows_ram_vram_and_temperature_without_leaving_one_screen() {
    let mut snapshot = pulse_testkit::snapshot(120);
    let mut adapter = pulse_testkit::monitor_adapter(snapshot.utc_ms);
    snapshot
        .adapters
        .push(pulse_testkit::monitor_adapter(snapshot.utc_ms));
    adapter.id = "second".into();
    adapter.name = "Second adaptateur".into();
    adapter.temperature.value = pulse_domain::Availability::Unsupported;
    snapshot.adapters.push(adapter);
    let mut s = State::new(false, snapshot);
    s.navigate(Page::Processes);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: true,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "open-monitor");
    assert_eq!(rt.state.page, Page::Monitor);
    for key in [
        "monitor-cpu",
        "monitor-ram",
        "monitor-gpu",
        "monitor-vram",
        "monitor-disk",
        "monitor-network",
    ] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    let labels = format!("{:?}", rt.elements());
    for text in [
        "32.0 Gio",
        "6.2 / 12.0 Gio",
        "64 °C",
        "Non mesurée",
        "PC allumé depuis",
    ] {
        assert!(labels.contains(text), "{text}");
    }
    tap(&mut rt, "monitor-next-gpu");
    let labels = format!("{:?}", rt.elements());
    assert!(labels.contains("Second adaptateur") && !labels.contains("64 °C"));
}

#[test]
fn start_page_has_single_navigation_metric_focus_and_contextual_shortcuts() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    assert!(rt.find("top-navigation/performance").is_none());
    for id in ["monitor", "processes", "gaming", "history"] {
        assert!(rt.find(&format!("navigation/{id}")).is_some());
    }
    for id in ["performance", "gpu", "io", "memory", "diagnostics"] {
        assert!(rt.find(&format!("navigation/{id}")).is_none());
    }
    tap(&mut rt, "overview-metric-1");
    assert_eq!(rt.state.tab, 1);
    assert!(format!("{:?}", rt.elements()).contains("Par mémoire privée"));
    tap(&mut rt, "overview-metric-2");
    tap(&mut rt, "overview-gpu-processes");
    assert_eq!((rt.state.page, rt.state.tab), (Page::Gpu, 3));
    tap(&mut rt, "navigation/monitor");
    tap(&mut rt, "direct-home");
    tap(&mut rt, "overview-gaming");
    assert_eq!(rt.state.page, Page::Gaming);
}

#[test]
fn gaming_auto_starts_without_a_selected_process() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let mut s = State::new(false, pulse_testkit::snapshot(120));
    s.gaming.commands = Some(sender);
    s.gaming.bundled_presentmon = true;
    s.navigate(Page::Gaming);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    assert!(rt.state.gaming.target.is_none() && receiver.try_recv().is_err());
    tap(&mut rt, "gaming-auto");
    assert!(matches!(
        receiver.try_recv().unwrap(),
        pulse_gaming::Request::Auto {
            executable: None,
            seconds: 30
        }
    ));
    assert!(rt.state.gaming.busy);
}

#[test]
fn gaming_selects_an_instance_and_only_captures_on_explicit_press() {
    use pulse_gaming::Request;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut s = State::new(false, pulse_testkit::snapshot(120));
    s.gaming.commands = Some(sender);
    s.gaming.cancel = Some(cancel.clone());
    s.navigate(Page::Processes);
    let process = s.process(0).key;
    s.selected = Some(process);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "measure-frames");
    assert_eq!(rt.state.page, Page::Gaming);
    tap(&mut rt, "gaming-tool");
    rt.resolve_file_dialog(Some("official-presentmon.exe".into()))
        .unwrap();
    for _ in 0..24 {
        rt.frame(1. / 60.);
    }
    tap(&mut rt, "capture-15");
    assert_eq!(rt.state.gaming.seconds, 15);
    assert!(receiver.try_recv().is_err());
    tap(&mut rt, "gaming-start");
    assert!(
        matches!(receiver.try_recv().unwrap(),Request::Capture { process: key, seconds:15, .. } if key == process)
    );
    tap(&mut rt, "gaming-cancel");
    assert!(cancel.load(Ordering::Acquire));
}

#[test]
fn wddm_process_tab_is_accessible_without_an_nvml_adapter() {
    use pulse_domain::{Availability, GpuKey, GpuProcess, GpuProcesses, Sample, Unit};
    let mut snap = pulse_testkit::snapshot(120);
    let row = |index: usize, cpu, bytes| {
        let p = &snap.processes[index];
        let value = |unit, value| Sample {
            unit,
            value: Availability::Available(value),
            utc_ms: 10,
            source: "WDDM test",
        };
        GpuProcess {
            process: p.key,
            name: p.name.clone(),
            adapter: GpuKey {
                luid_high: 0,
                luid_low: 191,
                physical: 0,
            },
            busiest_engine: Some("Compute".into()),
            activity: value(Unit::Percent, cpu),
            dedicated: value(Unit::Bytes, bytes),
            shared: value(Unit::Bytes, 0.),
        }
    };
    snap.gpu_processes = Some(std::sync::Arc::new(GpuProcesses {
        utc_ms: 10,
        rows: vec![row(0, 85., 50_000.), row(1, 5., 500_000.)],
        status: [Availability::Available(()); 3],
        excluded: 0,
    }));
    assert!(snap.adapters.is_empty());
    let mut rt = Runtime::new(
        State::new(false, snap),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "direct-gpu");
    tap(&mut rt, "tab-3");
    assert!(rt.find("gpu-process-table").is_some());
    assert!(format!("{:?}", rt.elements()).contains("Compute · 85.0 %"));
    tap(&mut rt, "gpu-sort-vram");
    assert!(rt.state.gpu_sort_memory);
}

#[test]
fn session_dialogs_are_scriptable_and_replay_restores_live_state() {
    use pulse_history::{SessionCommand, SessionReply};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let mut s = State::new(false, pulse_testkit::snapshot(120));
    let mode = Arc::new(AtomicBool::new(false));
    s.replay_mode = Some(mode.clone());
    s.session_commands = Some(sender);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "navigation/history");
    tap(&mut rt, "session-start");
    assert_eq!(rt.file_dialog().unwrap().0, rune::FileDialogKind::Save);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(0.);
    assert!(receiver.try_recv().is_err());
    tap(&mut rt, "session-start");
    rt.resolve_file_dialog(Some("new.sqlite".into())).unwrap();
    rt.frame(0.);
    assert!(
        matches!(receiver.try_recv().unwrap(), SessionCommand::Start { path, retain: 3600, include_processes: false } if path == std::path::Path::new("new.sqlite"))
    );
    rt.update(|s, tx| {
        s.apply_session(SessionReply {
            recording: true,
            message: "Started".into(),
            replay: None,
        });
        tx.instant();
    });
    rt.frame(0.);
    tap(&mut rt, "session-stop");
    assert!(matches!(receiver.try_recv().unwrap(), SessionCommand::Stop));
    let points = rt.state.history.clone();
    rt.update(|s, tx| {
        s.apply_session(SessionReply {
            recording: false,
            message: "Stopped".into(),
            replay: None,
        });
        tx.instant();
    });
    rt.frame(0.);
    tap(&mut rt, "session-open");
    assert_eq!(rt.file_dialog().unwrap().0, rune::FileDialogKind::Open);
    rt.resolve_file_dialog(Some("new.sqlite".into())).unwrap();
    rt.frame(0.);
    assert!(
        matches!(receiver.try_recv().unwrap(), SessionCommand::Open(path) if path == std::path::Path::new("new.sqlite"))
    );
    rt.update(|s, tx| {
        s.apply_session(SessionReply {
            recording: false,
            message: "Opened".into(),
            replay: Some(pulse_history::Replay {
                points,
                ..Default::default()
            }),
        });
        tx.instant();
    });
    rt.frame(0.);
    assert!(rt.state.replay && mode.load(Ordering::Acquire));
    tap(&mut rt, "session-live");
    assert!(!rt.state.replay && !mode.load(Ordering::Acquire));
    assert_eq!(rt.state.snapshot.processes.len(), 500);
}

#[test]
fn clicking_the_timeline_shows_producers_from_the_selected_historical_point() {
    use pulse_domain::{Contributor, ProcessKey};
    use pulse_history::{ContributorPoint, Point, Replay};
    let time = |at| Point {
        utc_ms: at,
        elapsed_ms: at,
        values: [Some(62.), Some(41.), None],
        rates: [None; 2],
    };
    let proc = Contributor {
        key: ProcessKey {
            pid: 90331,
            created_at_ticks: 47,
        },
        name: "processus-deja-ferme.exe".into(),
        cpu_percent: Some(52.),
        private_bytes: None,
    };
    let state = State::replay(Replay {
        points: [time(1000), time(2000), time(3000)].into(),
        contributors: [ContributorPoint {
            elapsed_ms: 2000,
            entries: vec![proc].into(),
        }]
        .into(),
    });
    let mut rt = Runtime::new(
        state,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "timeline-plot");
    assert_eq!(rt.state.cursor, Some(2000));
    assert!(rt.state.snapshot.processes.is_empty());
    assert!(format!("{:?}", rt.elements()).contains("processus-deja-ferme.exe"));
}

#[test]
fn connections_tab_filters_a_real_mode_snapshot_by_protocol() {
    use pulse_domain::{Availability, Endpoint, EndpointSnapshot, Transport};
    use std::sync::Arc;
    let entry = |transport, local: &str, pid| Endpoint {
        transport,
        local: local.parse().unwrap(),
        remote: None,
        tcp_state: (transport == Transport::Tcp).then_some(2),
        pid,
    };
    let mut snap = pulse_testkit::snapshot(120);
    snap.endpoints = Some(Arc::new(EndpointSnapshot {
        utc_ms: 123,
        omitted: 0,
        tables: [
            Availability::Available(1),
            Availability::Available(0),
            Availability::Available(1),
            Availability::Available(0),
        ],
        entries: vec![
            entry(Transport::Tcp, "127.0.0.1:31080", 70001),
            entry(Transport::Udp, "127.0.0.1:31535", 70002),
        ],
    }));
    let mut rt = Runtime::new(
        State::new(false, snap),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "direct-io");
    tap(&mut rt, "tab-2");
    assert!(rt.find("connections-table").is_some());
    tap(&mut rt, "connections-search");
    rt.paste("UDP");
    rt.frame(0.);
    let render = format!("{:?}", rt.elements());
    assert!(render.contains("127.0.0.1:31535"));
    assert!(!render.contains("127.0.0.1:31080"));
}

#[test]
fn tree_expands_and_collapses_without_losing_icons_or_process_identity() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "navigation/processes");
    tap(&mut rt, "process-tree");
    assert_eq!(rt.state.shown.len(), 500);
    let parent = rt.state.process(0).key;
    let key = format!("branch-{}-{}", parent.pid, parent.created_at_ticks);
    assert!(rt.state.tree_rows[0].has_children);
    assert_eq!(rt.state.tree_rows[1].depth, 1);
    tap(&mut rt, &key);
    assert!(rt.state.collapsed.contains(&parent));
    assert!(rt.state.shown.len() < 500);
    tap(&mut rt, &key);
    assert_eq!(rt.state.shown.len(), 500);
}

#[test]
fn recycled_parent_pid_is_not_used_to_build_a_branch() {
    let mut snapshot = pulse_testkit::snapshot(119);
    snapshot.processes.truncate(2);
    snapshot.processes[1].parent_pid = snapshot.processes[0].key.pid;
    snapshot.processes[0].key.created_at_ticks = snapshot.processes[1].key.created_at_ticks + 1;
    let mut state = State::new(false, snapshot);
    state.tree = true;
    state.project();
    assert_eq!(state.shown.len(), 2);
    assert!(
        state
            .tree_rows
            .iter()
            .all(|r| r.depth == 0 && !r.has_children)
    );
}

#[test]
fn hardware_detail_and_global_search_open_the_filtered_process_table() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1024., 640.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "direct-cpu");
    assert_eq!(rt.state.page, Page::Performance);
    tap(&mut rt, "global-search");
    rt.paste("4200");
    rt.frame(0.);
    rt.key(rune::KeyEvent {
        key: rune::Key::Enter,
        mods: rune::Mods::default(),
    });
    rt.frame(0.);
    assert_eq!(rt.state.page, Page::Processes);
    assert_eq!(rt.state.shown.len(), 1);
    assert_eq!(rt.state.process(0).key.pid, 4200);
}

fn tap(rt: &mut Runtime<State>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..24 {
        rt.frame(1. / 60.);
    }
}

#[test]
fn native_rune_navigation_table_and_filters() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.25,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "navigation/processes");
    assert_eq!(rt.state.page, Page::Processes);
    assert_eq!(rt.state.shown.len(), 500);
    let first = rt.state.process(0).key;
    let id = format!("{}-{}", first.pid, first.created_at_ticks);
    tap(&mut rt, &id);
    assert_eq!(rt.state.selected, Some(first));
    tap(&mut rt, "header/pid");
    assert!(rt.state.process(0).key.pid < rt.state.process(499).key.pid);
    assert_eq!(rt.state.selected, Some(first));
    tap(&mut rt, "active-filter");
    assert!(rt.state.shown.len() < 500);
    assert!(rt.state.process(0).cpu.value.get().unwrap() >= 1.);
    tap(&mut rt, "search");
    rt.paste(first.pid.to_string());
    rt.frame(0.);
    assert_eq!(rt.state.shown.len(), 1);
    assert_eq!(rt.state.process(0).key, first);
    tap(&mut rt, "navigation/monitor");
    tap(&mut rt, "direct-gpu");
    tap(&mut rt, "tab-1");
    assert_eq!((rt.state.page, rt.state.tab), (Page::Gpu, 1));
}

#[test]
fn termination_dialog_never_enqueues_until_confirmed() {
    let (sender, receiver) = pulse_actions::channel();
    let mut s = State::demo();
    s.navigate(Page::Processes);
    let process = s.process(0).key;
    s.selected = Some(process);
    // Fake command sink: there is no OS worker in this test.
    s.command = Some(sender);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "ask-terminate");
    assert!(receiver.try_recv().is_err());
    tap(&mut rt, "cancel-terminate");
    assert!(receiver.try_recv().is_err());
    tap(&mut rt, "ask-terminate");
    tap(&mut rt, "confirm-terminate");
    let command = receiver.try_recv().unwrap();
    assert_eq!(command.key, process);
    assert_eq!(command.change, pulse_actions::Change::Terminate);
    assert!(rt.state.action_pending);
}

#[test]
fn priority_requires_selecting_a_class_and_confirming_the_same_process() {
    use pulse_actions::{Change, PriorityTarget};
    let (sender, receiver) = pulse_actions::channel();
    let mut s = State::new(false, pulse_testkit::snapshot(120));
    s.navigate(Page::Processes);
    let process = s.process(0).key;
    s.selected = Some(process);
    s.command = Some(sender);
    let mut rt = Runtime::new(
        s,
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "ask-priority");
    tap(&mut rt, "priority-low");
    assert!(receiver.try_recv().is_err());
    tap(&mut rt, "confirm-terminate");
    let command = receiver.try_recv().unwrap();
    assert_eq!(command.key, process);
    assert_eq!(command.change, Change::SetPriority(PriorityTarget::Low));
}

#[test]
fn timeline_and_pause_controls_commit() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1024., 640.),
        Env {
            system_dark: false,
            scale: 1.5,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "navigation/history");
    tap(&mut rt, "period-30");
    tap(&mut rt, "back-point");
    assert_eq!(rt.state.period, 30);
    assert_eq!(rt.state.cursor, Some(118_000));
    tap(&mut rt, "session-diagnostics");
    assert_eq!(rt.state.page, Page::Diagnostics);
    assert_eq!(rt.state.cursor, Some(118_000));
    tap(&mut rt, "return-live");
    assert_eq!(rt.state.cursor, None);
    tap(&mut rt, "pause");
    assert!(rt.state.paused);
}

#[test]
fn switches_to_network_history_and_preserves_missing_rates() {
    let mut rt = Runtime::new(
        State::demo(),
        view,
        (1440., 900.),
        Env {
            system_dark: false,
            scale: 1.,
        },
    );
    rt.frame(0.);
    tap(&mut rt, "direct-io");
    assert_eq!(rt.state.page, Page::Io);
    tap(&mut rt, "tab-1");
    tap(&mut rt, "period-30");
    assert_eq!((rt.state.tab, rt.state.period), (1, 30));
    let mut latest = pulse_testkit::snapshot(120);
    latest.network.value = pulse_domain::Availability::Pending;
    rt.state.publish(std::sync::Arc::new(latest));
    assert!(rt.state.io.back().unwrap().values[0].is_some());
    assert_eq!(rt.state.io.back().unwrap().values[1], None);
}
