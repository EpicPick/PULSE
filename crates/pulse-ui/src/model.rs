use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use pulse_domain::{Availability, Process, ProcessKey, Sample, Snapshot, Unit};
use pulse_history::{ContributorPoint, Point, Replay, SessionCommand, SessionReply, ring_push};
use rune::TextEdit;
use rune_kit::{SortDir, TableState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Monitor,
    Overview,
    Performance,
    Processes,
    Gpu,
    Io,
    Memory,
    Diagnostics,
    History,
    Gaming,
}

pub struct State {
    pub page: Page,
    pub period: usize,
    pub tab: usize,
    pub paused: bool,
    pub demo: bool,
    pub replay: bool,
    pub session_commands: Option<std::sync::mpsc::SyncSender<SessionCommand>>,
    pub session_replies: Option<pulse_runtime::Mailbox<SessionReply>>,
    pub session_busy: bool,
    pub replay_mode: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub recording: bool,
    pub include_processes: bool,
    pub recording_status: Option<String>,
    live_state: Option<Box<State>>,
    pub inbox: Option<pulse_runtime::Mailbox<Snapshot>>,
    pub command: Option<std::sync::mpsc::SyncSender<pulse_actions::Command>>,
    pub replies: Option<pulse_runtime::Mailbox<pulse_actions::Outcome>>,
    pub confirmation: Option<(pulse_actions::Command, String)>,
    pub priority_picker: Option<(ProcessKey, String)>,
    pub action_pending: bool,
    pub action_status: Option<String>,
    pub query: TextEdit,
    pub global_query: TextEdit,
    pub gpu_selection: Option<String>,
    pub network_query: TextEdit,
    pub network_page: usize,
    pub network_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub gpu_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub storage_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub storage_inbox: Option<pulse_runtime::Mailbox<pulse_domain::StorageSnapshot>>,
    pub(crate) storage: Option<Arc<pulse_domain::StorageSnapshot>>,
    pub(crate) storage_history: std::collections::BTreeMap<u32, (String, VecDeque<Point>)>,
    pub(crate) disk_selected: Option<u32>,
    pub(crate) disk_page: usize,
    pub gpu_process_page: usize,
    pub gpu_sort_memory: bool,
    pub table: TableState,
    pub selected: Option<ProcessKey>,
    pub(crate) inspector_history: VecDeque<Point>,
    inspector_key: Option<ProcessKey>,
    gpu_by_process: HashMap<ProcessKey, f64>,
    pub snapshot: Arc<Snapshot>,
    pub history: VecDeque<Point>,
    pub contributor_history: VecDeque<ContributorPoint>,
    pub gaming: GamingState,
    pub contributors_by_memory: bool,
    pub io: VecDeque<Point>,
    pub shown: Vec<usize>,
    pub tree: bool,
    pub tree_rows: Vec<TreeRow>,
    pub collapsed: HashSet<ProcessKey>,
    pub matched: usize,
    pub active_only: bool,
    pub cursor: Option<u64>,
    pub tracked: Option<ProcessKey>,
    pub memory: VecDeque<Point>,
}

#[derive(Default)]
pub struct GamingState {
    pub commands: Option<std::sync::mpsc::SyncSender<pulse_gaming::Request>>,
    pub replies: Option<pulse_runtime::Mailbox<Result<Arc<pulse_gaming::Report>, String>>>,
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub target: Option<(ProcessKey, String)>,
    pub executable: Option<std::path::PathBuf>,
    pub bundled_presentmon: bool,
    pub seconds: u32,
    pub busy: bool,
    pub status: Option<String>,
    pub report: Option<Arc<pulse_gaming::Report>>,
}

impl GamingState {
    pub fn request(&mut self, request: pulse_gaming::Request) {
        if self.busy {
            return;
        }
        let Some(sender) = &self.commands else {
            return;
        };
        if let Some(cancel) = &self.cancel {
            cancel.store(false, std::sync::atomic::Ordering::Release);
        }
        let auto = matches!(&request, pulse_gaming::Request::Auto { .. });
        match sender.try_send(request) {
            Ok(()) => {
                self.busy = true;
                self.report = None;
                self.status = Some(if auto {
                    "Auto armé : reviens directement dans le jeu. Seules ses images au premier plan seront comptées."
                } else { "Traitement PresentMon en cours…" }.into());
            }
            Err(_) => self.status = Some("Le service PresentMon n’est pas disponible.".into()),
        }
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Release);
            self.status = Some("Arrêt demandé…".into());
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub has_children: bool,
}

impl State {
    pub fn replay(
        Replay {
            points: history,
            contributors,
        }: Replay,
    ) -> Self {
        let latest = history.back().copied().unwrap_or(Point {
            utc_ms: 0,
            elapsed_ms: 0,
            values: [None; 3],
            rates: [None; 2],
        });
        let value = |unit, number: Option<f64>| Sample {
            utc_ms: latest.utc_ms,
            unit,
            source: "Session SQLite PULSE · agrégats collectés",
            value: number.map_or(Availability::Unsupported, Availability::Available),
        };
        let mut state = Self::new(
            false,
            Snapshot {
                utc_ms: latest.utc_ms,
                elapsed_ms: latest.elapsed_ms,
                cpu: value(Unit::Percent, latest.values[0]),
                logical_cpus: Vec::new(),
                logical_cpu_status: Availability::Unsupported,
                ram: value(Unit::Percent, latest.values[1]),
                gpu: value(Unit::Percent, latest.values[2]),
                adapters: Vec::new(),
                monitor: Default::default(),
                gpu_processes: None,
                disk: value(Unit::BytesPerSecond, latest.rates[0]),
                disk_read_latency: value(Unit::Milliseconds, None),
                disk_write_latency: value(Unit::Milliseconds, None),
                network: value(Unit::BytesPerSecond, latest.rates[1]),
                processes: Vec::new(),
                contributors: None,
                endpoints: None,
                skipped: 0,
                process_error: None,
            },
        );
        state.io = history
            .iter()
            .map(|p| Point {
                values: [p.rates[0], p.rates[1], None],
                ..*p
            })
            .collect();
        state.history = history;
        state.contributor_history = contributors;
        state.replay = true;
        state.navigate(Page::History);
        state
    }

    pub fn demo() -> Self {
        let mut s = Self::new(true, pulse_testkit::snapshot(119));
        s.history = (0..119)
            .map(|t| Point {
                utc_ms: pulse_testkit::EPOCH + t * 1000,
                elapsed_ms: t * 1000,
                values: pulse_testkit::system(t).map(Some),
                rates: pulse_testkit::throughput(t).map(Some),
            })
            .chain(s.history)
            .collect();
        s.io = (0..119)
            .map(|t| {
                let [disk, network] = pulse_testkit::throughput(t);
                Point {
                    utc_ms: pulse_testkit::EPOCH + t * 1000,
                    elapsed_ms: t * 1000,
                    values: [Some(disk), Some(network), None],
                    rates: [None; 2],
                }
            })
            .chain(s.io)
            .collect();
        s
    }

    pub fn new(demo: bool, first: Snapshot) -> Self {
        let mut s = Self {
            page: Page::Overview,
            period: 120,
            tab: 0,
            paused: false,
            demo,
            replay: false,
            session_commands: None,
            session_replies: None,
            session_busy: false,
            replay_mode: None,
            recording: false,
            include_processes: false,
            recording_status: None,
            live_state: None,
            inbox: None,
            command: None,
            replies: None,
            confirmation: None,
            priority_picker: None,
            action_pending: false,
            action_status: None,
            query: TextEdit::new(""),
            global_query: TextEdit::new(""),
            gpu_selection: None,
            network_query: TextEdit::new(""),
            network_page: 0,
            network_requested: None,
            gpu_requested: None,
            storage_requested: None,
            storage_inbox: None,
            storage: None,
            storage_history: Default::default(),
            disk_selected: None,
            disk_page: 0,
            gpu_process_page: 0,
            gpu_sort_memory: false,
            table: TableState::new(),
            selected: None,
            inspector_history: VecDeque::with_capacity(61),
            inspector_key: None,
            gpu_by_process: HashMap::new(),
            snapshot: Arc::new(first),
            history: VecDeque::with_capacity(900),
            contributor_history: VecDeque::with_capacity(900),
            gaming: GamingState {
                seconds: 30,
                bundled_presentmon: pulse_gaming::BUNDLED.is_some(),
                ..Default::default()
            },
            contributors_by_memory: false,
            io: VecDeque::with_capacity(900),
            shown: Vec::new(),
            tree: false,
            tree_rows: Vec::new(),
            collapsed: HashSet::new(),
            matched: 0,
            active_only: false,
            cursor: None,
            tracked: None,
            memory: VecDeque::new(),
        };
        s.table.set_sort("cpu", SortDir::Descending);
        s.index_gpu();
        s.record();
        s.project();
        s
    }

    pub fn navigate(&mut self, page: Page) {
        self.page = page;
        self.tab = 0;
        self.cursor = None;
        if page == Page::Processes {
            self.project();
        }
    }

    pub fn request_session(&mut self, request: SessionCommand) {
        if self.session_busy {
            return;
        }
        if self.gaming.busy && matches!(&request, SessionCommand::Open(_)) {
            self.recording_status =
                Some("Terminez ou annulez l’analyse PresentMon avant d’ouvrir une session.".into());
            return;
        }
        let Some(sender) = &self.session_commands else {
            return;
        };
        match sender.try_send(request) {
            Ok(()) => self.session_busy = true,
            Err(_) => {
                self.recording_status = Some("Le service de sessions est indisponible.".into())
            }
        }
    }

    pub fn apply_session(&mut self, reply: SessionReply) {
        self.session_busy = false;
        self.recording = reply.recording;
        self.recording_status = Some(reply.message);
        if let Some(points) = reply.replay {
            if let Some(mode) = &self.replay_mode {
                mode.store(true, std::sync::atomic::Ordering::Release);
            }
            let mut replay = Self::replay(points);
            replay.session_commands = self.session_commands.clone();
            replay.session_replies = self.session_replies.clone();
            replay.replay_mode = self.replay_mode.clone();
            replay.recording_status = self.recording_status.take();
            if self.replay {
                replay.live_state = self.live_state.take();
                *self = replay;
            } else {
                let previous = std::mem::replace(self, replay);
                self.live_state = Some(Box::new(previous));
            }
        }
    }

    pub fn return_live(&mut self) {
        if self.session_busy || !self.replay {
            return;
        }
        if let Some(prior) = self.live_state.take() {
            *self = *prior;
            self.paused = false;
        } else {
            return;
        }
        if let Some(mode) = &self.replay_mode {
            mode.store(false, std::sync::atomic::Ordering::Release);
        }
        self.recording_status = Some("Surveillance locale reprise.".into());
    }

    pub fn can_return_live(&self) -> bool {
        self.live_state.is_some() && !self.session_busy
    }

    pub fn confirm_action(&mut self) {
        let Some((command, _)) = self.confirmation.take() else {
            return;
        };
        let Some(sender) = &self.command else {
            return;
        };
        match sender.try_send(command) {
            Ok(()) => {
                self.action_pending = true;
                self.action_status = Some("Demande en cours…".into());
            }
            Err(_) => self.action_status = Some("La demande n’a pas pu être envoyée.".into()),
        }
    }

    pub fn publish(&mut self, snapshot: Arc<Snapshot>) {
        self.snapshot = snapshot;
        self.index_gpu();
        self.sample_inspector();
        self.record();
        if !self.demo
            && self.tracked.is_some()
            && self
                .memory
                .back()
                .is_none_or(|p| self.snapshot.elapsed_ms.saturating_sub(p.elapsed_ms) >= 6000)
        {
            let p = self
                .snapshot
                .processes
                .iter()
                .find(|p| Some(p.key) == self.tracked);
            ring_push(
                &mut self.memory,
                Point {
                    utc_ms: self.snapshot.utc_ms,
                    elapsed_ms: self.snapshot.elapsed_ms,
                    values: [
                        p.and_then(|p| p.private_bytes.value.get())
                            .map(|b| b as f64),
                        p.and_then(|p| p.working_set.value.get()).map(|b| b as f64),
                        None,
                    ],
                    rates: [None; 2],
                },
            );
        }
        if self.page == Page::Processes {
            self.project();
        }
    }

    pub fn publish_storage(&mut self, data: Arc<pulse_domain::StorageSnapshot>) {
        self.storage_history.retain(|i, _| {
            data.drives
                .iter()
                .any(|d| &d.index == i && !d.identity.is_empty())
        });
        for d in data.drives.iter().filter(|d| !d.identity.is_empty()) {
            let (id, points) = self
                .storage_history
                .entry(d.index)
                .or_insert_with(|| (d.identity.clone(), VecDeque::new()));
            if *id != d.identity
                || points
                    .back()
                    .is_some_and(|p| p.elapsed_ms >= data.elapsed_ms)
            {
                id.clone_from(&d.identity);
                points.clear();
            }
            while points
                .front()
                .is_some_and(|p| data.elapsed_ms.saturating_sub(p.elapsed_ms) > 60_000)
                || points.len() >= 61
            {
                points.pop_front();
            }
            points.push_back(Point {
                utc_ms: 0,
                elapsed_ms: data.elapsed_ms,
                values: [d.read_bps, d.write_bps, d.busy_percent],
                rates: [None; 2],
            });
        }
        self.storage = Some(data);
    }

    fn index_gpu(&mut self) {
        self.gpu_by_process.clear();
        if let Some(readings) = &self.snapshot.gpu_processes {
            for row in &readings.rows {
                if let Some(load) = row.activity.value.get() {
                    self.gpu_by_process
                        .entry(row.process)
                        .and_modify(|v| *v = v.max(load))
                        .or_insert(load);
                }
            }
        }
    }

    pub(crate) fn process_gpu(&self, key: ProcessKey) -> Option<f64> {
        self.gpu_by_process.get(&key).copied()
    }

    pub fn select_process(&mut self, key: Option<ProcessKey>) {
        self.selected = key;
        if let Some(key) = key {
            self.table.select(row_id(key));
        } else {
            self.table.clear_selection();
        }
        self.sample_inspector();
    }

    fn sample_inspector(&mut self) {
        if self.selected != self.inspector_key {
            self.inspector_key = self.selected;
            self.inspector_history.clear();
        }
        let Some(key) = self.selected else { return };
        if self.replay || self.snapshot.utc_ms == 0 {
            return;
        }
        let elapsed_ms = self.snapshot.elapsed_ms;
        if self
            .inspector_history
            .back()
            .is_some_and(|p| p.elapsed_ms >= elapsed_ms)
        {
            return;
        }
        let p = self.snapshot.processes.iter().find(|p| p.key == key);
        if self.inspector_history.len() >= 61 {
            self.inspector_history.pop_front();
        }
        while self
            .inspector_history
            .front()
            .is_some_and(|p| elapsed_ms.saturating_sub(p.elapsed_ms) > 60_000)
        {
            self.inspector_history.pop_front();
        }
        self.inspector_history.push_back(Point {
            utc_ms: self.snapshot.utc_ms,
            elapsed_ms,
            values: [
                p.and_then(|p| p.cpu.value.get()),
                p.and_then(|p| p.private_bytes.value.get())
                    .map(|b| b as f64),
                p.and(self.process_gpu(key)),
            ],
            rates: [None; 2],
        });
    }

    fn record(&mut self) {
        let s = &self.snapshot;
        if let Some(entries) = &s.contributors {
            if self.contributor_history.len() >= pulse_history::LIVE_CAPACITY {
                self.contributor_history.pop_front();
            }
            self.contributor_history.push_back(ContributorPoint {
                elapsed_ms: s.elapsed_ms,
                entries: entries.clone(),
            });
        }
        ring_push(
            &mut self.history,
            Point {
                utc_ms: s.utc_ms,
                elapsed_ms: s.elapsed_ms,
                values: [s.cpu.value.get(), s.ram.value.get(), s.gpu.value.get()],
                rates: [s.disk.value.get(), s.network.value.get()],
            },
        );
        if let Some(first) = self.history.front() {
            while self
                .contributor_history
                .front()
                .is_some_and(|p| p.elapsed_ms < first.elapsed_ms)
            {
                self.contributor_history.pop_front();
            }
        }
        ring_push(
            &mut self.io,
            Point {
                utc_ms: s.utc_ms,
                elapsed_ms: s.elapsed_ms,
                values: [s.disk.value.get(), s.network.value.get(), None],
                rates: [None; 2],
            },
        );
    }

    pub fn project(&mut self) {
        let q = self.query.text.trim().to_lowercase();
        let pid = q.parse::<u32>().ok();
        self.shown.clear();
        let rows = &self.snapshot.processes;
        self.shown.extend(
            rows.iter()
                .enumerate()
                .filter(|(_, p)| {
                    (q.is_empty() || pid == Some(p.key.pid) || p.name.to_lowercase().contains(&q))
                        && (!self.active_only || p.cpu.value.get().is_some_and(|v| v >= 1.0))
                })
                .map(|(i, _)| i),
        );
        let (column, dir) = self.table.sort().unwrap_or(("cpu", SortDir::Descending));
        let gpu = &self.gpu_by_process;
        self.shown.sort_unstable_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            let order = match column {
                "name" => dir.apply(a.name.cmp(&b.name)),
                "pid" => dir.apply(a.key.pid.cmp(&b.key.pid)),
                "ram" => optional(
                    a.working_set.value.get(),
                    b.working_set.value.get(),
                    dir,
                    u64::cmp,
                ),
                "private" => optional(
                    a.private_bytes.value.get(),
                    b.private_bytes.value.get(),
                    dir,
                    u64::cmp,
                ),
                "threads" => optional(a.threads.value.get(), b.threads.value.get(), dir, u32::cmp),
                "gpu" => optional(
                    gpu.get(&a.key).copied(),
                    gpu.get(&b.key).copied(),
                    dir,
                    f64::total_cmp,
                ),
                _ => optional(a.cpu.value.get(), b.cpu.value.get(), dir, f64::total_cmp),
            };
            order.then_with(|| a.key.cmp(&b.key))
        });
        self.matched = self.shown.len();
        self.tree_rows.clear();
        if self.tree {
            let (indices, tree_rows) = hierarchy(
                rows,
                &self.shown,
                &mut self.collapsed,
                q.is_empty() && !self.active_only,
            );
            self.shown = indices;
            self.tree_rows = tree_rows;
        }
    }

    pub fn toggle_branch(&mut self, key: ProcessKey) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.project();
    }

    pub fn process(&self, index: usize) -> &Process {
        &self.snapshot.processes[self.shown[index]]
    }
    pub fn detail(&self) -> Option<&Process> {
        self.snapshot
            .processes
            .iter()
            .find(|p| Some(p.key) == self.selected)
    }
}

fn hierarchy(
    rows: &[Process],
    sorted: &[usize],
    collapsed: &mut HashSet<ProcessKey>,
    allow_collapse: bool,
) -> (Vec<usize>, Vec<TreeRow>) {
    let mut by_pid = HashMap::with_capacity(sorted.len());
    for &index in sorted {
        let pid = rows[index].key.pid;
        // An ambiguous PID cannot name a reliable parent.
        by_pid
            .entry(pid)
            .and_modify(|v| *v = None)
            .or_insert(Some(index));
    }
    if !collapsed.is_empty() {
        let present: HashSet<_> = rows.iter().map(|p| p.key).collect();
        collapsed.retain(|key| present.contains(key));
    }
    let mut children = HashMap::<usize, Vec<usize>>::new();
    let mut roots = Vec::new();
    for &i in sorted {
        let child = &rows[i];
        let parent = by_pid
            .get(&child.parent_pid)
            .copied()
            .flatten()
            .filter(|&j| rows[j].key.created_at_ticks < child.key.created_at_ticks);
        if let Some(parent) = parent {
            children.entry(parent).or_default().push(i);
        } else {
            roots.push(i);
        }
    }
    let mut indices = Vec::with_capacity(sorted.len());
    let mut meta = Vec::with_capacity(sorted.len());
    let mut stack: Vec<_> = roots.into_iter().rev().map(|i| (i, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        let descendants = children.get(&index);
        indices.push(index);
        meta.push(TreeRow {
            depth,
            has_children: descendants.is_some(),
        });
        if !(allow_collapse && collapsed.contains(&rows[index].key))
            && let Some(next) = descendants
        {
            stack.extend(next.iter().rev().map(|&i| (i, depth + 1)));
        }
    }
    (indices, meta)
}

pub fn row_id(k: ProcessKey) -> String {
    format!("{}-{}", k.pid, k.created_at_ticks)
}

fn optional<T>(
    a: Option<T>,
    b: Option<T>,
    dir: SortDir,
    cmp: impl Fn(&T, &T) -> Ordering,
) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => dir.apply(cmp(&a, &b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod inspector_tests {
    use super::*;

    #[test]
    fn replacement_disk_does_not_inherit_prior_history() {
        let mut state = State::demo();
        for tick in 0..70 {
            state.publish_storage(Arc::new(pulse_testkit::storage(tick)));
        }
        assert_eq!(state.storage_history[&0].1.len(), 61);
        let mut replaced = pulse_testkit::storage(70);
        replaced.drives[0].identity = "replacement".into();
        state.publish_storage(Arc::new(replaced));
        assert_eq!(state.storage_history[&0].1.len(), 1);
        assert_eq!(state.storage_history[&1].1.len(), 61);
    }

    #[test]
    fn trends_are_bounded_and_never_mix_process_identities() {
        let snapshot = pulse_testkit::snapshot(120);
        let key = snapshot.processes[0].key;
        let mut state = State::new(false, snapshot);
        state.select_process(Some(key));
        for i in 121..190 {
            state.publish(Arc::new(pulse_testkit::snapshot(i)));
        }
        assert_eq!(state.inspector_history.len(), 61);
        assert!(
            state
                .inspector_history
                .iter()
                .all(|p| p.values[0].is_some())
        );
        let mut replacement = pulse_testkit::snapshot(190);
        replacement.processes[0].key.created_at_ticks += 1;
        let new_key = replacement.processes[0].key;
        state.publish(Arc::new(replacement));
        assert!(state.inspector_history.back().unwrap().values[0].is_none());
        state.select_process(Some(new_key));
        assert_eq!(state.inspector_history.len(), 1);
        assert!(state.inspector_history.back().unwrap().values[0].is_some());
        state.select_process(None);
        assert!(state.inspector_history.is_empty());
    }
}
