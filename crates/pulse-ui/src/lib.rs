#![forbid(unsafe_code)]
mod model;
mod monitor;
mod storage;
pub use model::{Page, State};

use pulse_design::{c, card, kit, label, n, table_style};
use pulse_domain::{Availability, MIB, Sample, Unit};
use rune::{Align, Cx, El, LineCap, ScrollSpec, Shape, canvas, col, el, row};
use rune_kit::{Cell, Table, TableColumn, TextInput};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Weak},
};

const NAV: [(Page, &str, &str, &str); 4] = [
    (Page::Processes, "processes", "Processus", "process"),
    (Page::Monitor, "monitor", "Moniteur", "overview"),
    (Page::Gaming, "gaming", "Jeux", "game"),
    (Page::History, "history", "Sessions", "history"),
];

fn section(page: Page) -> Page {
    match page {
        Page::Overview | Page::Performance | Page::Io | Page::Gpu => Page::Monitor,
        Page::Memory => Page::Processes,
        Page::Diagnostics => Page::History,
        other => other,
    }
}

fn glyph(id: &str, ink: &str) -> El {
    let lines: &[&[[f32; 2]]] = match id {
        "logo" => &[&[
            [1., 12.],
            [5., 12.],
            [8., 5.],
            [12., 18.],
            [15., 10.],
            [21., 10.],
        ]],
        "overview" => &[
            &[[3., 17.], [3., 5.], [17., 5.], [17., 17.], [3., 17.]],
            &[[5., 13.], [8., 10.], [11., 13.], [15., 8.]],
        ],
        "process" => &[
            &[[5., 6.], [17., 6.]],
            &[[5., 11.], [17., 11.]],
            &[[5., 16.], [17., 16.]],
        ],
        "io" => &[
            &[[6., 3.], [6., 16.], [3., 13.]],
            &[[6., 16.], [9., 13.]],
            &[[15., 17.], [15., 4.], [12., 7.]],
            &[[15., 4.], [18., 7.]],
        ],
        "gpu" => &[
            &[[3., 6.], [17., 6.], [17., 16.], [3., 16.], [3., 6.]],
            &[[7., 9.], [12., 9.], [12., 13.], [7., 13.], [7., 9.]],
        ],
        "memory" => &[
            &[[4., 7.], [17., 7.], [17., 15.], [4., 15.], [4., 7.]],
            &[[7., 10.], [7., 13.]],
            &[[10., 10.], [10., 13.]],
            &[[14., 10.], [14., 13.]],
        ],
        "game" => &[
            &[
                [3., 15.],
                [5., 7.],
                [16., 7.],
                [19., 15.],
                [17., 17.],
                [14., 14.],
                [8., 14.],
                [5., 17.],
                [3., 15.],
            ],
            &[[6., 10.], [10., 10.]],
            &[[8., 8.], [8., 12.]],
            &[[14., 10.], [16., 10.]],
        ],
        "history" => &[
            &[[4., 4.], [18., 4.], [18., 18.], [4., 18.], [4., 4.]],
            &[[8., 9.], [14., 9.]],
            &[[8., 13.], [14., 13.]],
        ],
        _ => &[&[
            [3., 11.],
            [7., 11.],
            [9., 6.],
            [12., 16.],
            [14., 11.],
            [18., 11.],
        ]],
    };
    let shape = lines.iter().fold(Shape::new(), |s, line| {
        s.stroke(line.to_vec(), 1.45, c(ink))
    });
    canvas(shape.view_box(22., 22.))
        .size(20., 20.)
        .a11y_hidden()
}

fn action(id: &str, title: &str, active: bool, f: impl Fn(&mut State) + 'static) -> El {
    row()
        .key(id)
        .h(n("row"))
        .center_y()
        .px(n("gapInner"))
        .fill(c(if active { "primary" } else { "surface" }))
        .hover(c("info").alpha(0.13))
        .border(
            n("borderWidth"),
            c(if active { "primary" } else { "borderStrong" }),
        )
        .radius(n("controlRadius"))
        .role("button")
        .label(title)
        .focusable(true)
        .focus_ring(2., 1., c("primary"))
        .motion(rune::Spring::new(n("motion"), 0.))
        .on_press(f)
        .child(label(
            title,
            "small",
            if active { "inverse" } else { "text" },
        ))
}

fn navigation(s: &State) -> El {
    let mut nav = col()
        .key("navigation")
        .gap(6.)
        .w(n("sidebarWidth"))
        .padding(8.)
        .fill(c("surface"))
        .border_sides([0., 1., 0., 0.], c("border"));
    for (page, id, title, icon) in NAV {
        let active = page == section(s.page);
        nav = nav.child(
            row()
                .key(id)
                .h(42.)
                .center()
                .fill(c(if active { "primaryTint" } else { "surface" }))
                .hover(c("primaryTint"))
                .radius(n("controlRadius"))
                .role("button")
                .label(title)
                .help(title)
                .focusable(true)
                .focus_ring(2., 1., c("primary"))
                .on_press(move |s: &mut State| s.navigate(page))
                .child(glyph(icon, if active { "primary" } else { "muted" })),
        );
    }
    nav
}

fn format_sample(s: Sample<f64>) -> String {
    let Some(v) = s.value.get() else {
        return "—".into();
    };
    match s.unit {
        Unit::Percent => format!("{v:.1} %"),
        Unit::BytesPerSecond if v >= MIB as f64 => format!("{:.1} Mio/s", v / MIB as f64),
        Unit::BytesPerSecond if v >= 1024. => format!("{:.1} Kio/s", v / 1024.),
        Unit::BytesPerSecond => format!("{v:.1} o/s"),
        Unit::Celsius => format!("{v:.0} °C"),
        Unit::Watts => format!("{v:.0} W"),
        Unit::Megahertz => format!("{v:.0} MHz"),
        Unit::Milliseconds => format!("{v:.2} ms"),
        Unit::Bytes => format!("{:.1} Mio", v / MIB as f64),
        Unit::Count => format!("{v:.0}"),
        Unit::Path | Unit::PriorityClass => "—".into(),
    }
}

fn provenance<T>(s: &Sample<T>) -> String {
    let status = match s.value {
        Availability::Available(_) => "Disponible".into(),
        Availability::Unsupported => "Non pris en charge".into(),
        Availability::PermissionRequired => "Accès refusé".into(),
        Availability::Pending => "En attente de mesure".into(),
        Availability::Error(code) => format!("Erreur {code:#010x}"),
    };
    format!(
        "{status} · {} · UTC {} ms · {:?}",
        s.source, s.utc_ms, s.unit
    )
}

fn metric(title: &str, sample: Sample<f64>, ink: &str, note: &str) -> El {
    card()
        .grow(1.)
        .min_w(0.)
        .h(n("kpi"))
        .gap(n("gapSmall"))
        .help(provenance(&sample))
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(el().size(7., 7.).radius(4.).fill(c(ink)))
                .child(label(title, "body", "text").truncate()),
        )
        .child(label(format_sample(sample), "value", "text").truncate())
        .child(
            label(
                if sample.value.get().is_some() {
                    note
                } else {
                    "Non disponible"
                },
                "small",
                "muted",
            )
            .truncate(),
        )
}

fn metrics(items: &[(&str, Sample<f64>, &str, &str)]) -> El {
    items.iter().fold(
        row().gap(n("gapInner")).w_full(),
        |r, (title, value, ink, note)| r.child(metric(title, *value, ink, note)),
    )
}

fn filled_segment(mut shape: Shape, points: Vec<[f32; 2]>, ink: &str, baseline: f32) -> Shape {
    match points.as_slice() {
        [] => shape,
        [p] => shape.line(*p, [p[0] + 0.1, p[1]], 2.6, LineCap::Round, c(ink)),
        [first, .., last] => {
            if points.len() <= 240 && last[0] > first[0] && points.iter().any(|p| p[1] < baseline) {
                let mut area = points.clone();
                area.extend([[last[0], baseline], [first[0], baseline]]);
                shape = shape.fill(area, c(ink).alpha(0.13));
            }
            shape.stroke(points, 2.0, c(ink))
        }
    }
}

fn dashboard_metric(title: &str, icon: &str, sample: Sample<f64>, ink: &str, note: &str) -> El {
    card()
        .grow(1.)
        .min_w(0.)
        .min_h(110.)
        .gap(n("space"))
        .help(provenance(&sample))
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(glyph(icon, ink))
                .child(label(title, "body", "text").truncate()),
        )
        .child(
            col()
                .child(label(format_sample(sample), "value", "text").truncate())
                .child(
                    label(
                        if sample.value.get().is_some() {
                            note
                        } else {
                            "Non disponible"
                        },
                        "small",
                        "muted",
                    )
                    .truncate(),
                ),
        )
}

fn periods(s: &State) -> El {
    [(30, "30 s"), (60, "1 min"), (120, "2 min"), (900, "15 min")]
        .into_iter()
        .fold(row().gap(n("space")), |r, (period, text)| {
            r.child(action(
                &format!("period-{period}"),
                text,
                s.period == period,
                move |s| {
                    s.period = period;
                    s.cursor = None;
                },
            ))
        })
}

fn chart(
    s: &State,
    title: &str,
    unit: &str,
    max: f64,
    series: &[(&str, &str, Vec<Option<f64>>)],
) -> El {
    let grid = (0..=4).fold(Shape::new(), |sh, i| {
        let y = 8. + i as f32 * 46.;
        sh.line([0., y], [1000., y], 0.8, LineCap::Butt, c("border"))
    });
    let total = series.first().map_or(0, |(_, _, v)| v.len());
    let timeline = s.page != Page::Memory || !s.demo;
    let history = match s.page {
        Page::Memory => &s.memory,
        Page::Io => &s.io,
        _ => &s.history,
    };
    let start = history.len().saturating_sub(total);
    let axis_t = |i: usize| history.get(start + i).map(|p| p.elapsed_ms);
    let bounds = axis_t(0)
        .zip(axis_t(total.saturating_sub(1)))
        .filter(|(a, b)| a < b);
    let x = |i| {
        if timeline {
            match bounds.zip(axis_t(i)) {
                Some(((a, b), t)) => (t.saturating_sub(a) as f64 / (b - a) as f64 * 1000.) as f32,
                None => i as f32 * 1000. / total.saturating_sub(1).max(1) as f32,
            }
        } else {
            i as f32 * 1000. / total.saturating_sub(1).max(1) as f32
        }
    };
    let mut shape = series.iter().fold(grid, |mut sh, (_, ink, values)| {
        let mut segment = Vec::new();
        for (i, value) in values.iter().enumerate() {
            if let Some(v) = value {
                segment.push([x(i), 192. - (v / max).clamp(0., 1.) as f32 * 184.]);
            } else if !segment.is_empty() {
                sh = filled_segment(sh, std::mem::take(&mut segment), ink, 192.);
            }
        }
        filled_segment(sh, segment, ink, 192.)
    });
    let inspection = s
        .cursor
        .and_then(|time| (0..total).find(|&i| axis_t(i) == Some(time)))
        .map(|i| {
            shape = std::mem::take(&mut shape).line(
                [x(i), 8.],
                [x(i), 192.],
                1.,
                LineCap::Butt,
                c("muted"),
            );
            series
                .iter()
                .map(|(name, _, v)| {
                    format!(
                        "{name} : {}",
                        v.get(i)
                            .and_then(|v| *v)
                            .map_or("—".into(), |v| format!("{v:.1} {unit}"))
                    )
                })
                .collect::<Vec<_>>()
                .join("   ·   ")
        });
    let legend = series
        .iter()
        .fold(row().gap(n("gap")).center_y(), |r, (name, ink, _)| {
            r.child(
                row()
                    .gap(n("space"))
                    .center_y()
                    .child(el().size(12., 3.).fill(c(ink)))
                    .child(label(*name, "small", "muted")),
            )
        });
    let height = if matches!(
        s.page,
        Page::Overview | Page::Memory | Page::Diagnostics | Page::History
    ) {
        252.
    } else {
        220.
    };
    let axis = (0..=4).fold(col().h(height - 4.).justify_between(), |a, i| {
        a.child(label(
            format!("{:.0}", max * f64::from(4 - i) / 4.),
            "small",
            "muted",
        ))
    });
    let elapsed = if timeline {
        bounds.map_or(0, |(a, b)| b.saturating_sub(a) / 1000)
    } else {
        total.saturating_sub(1) as u64 * 60
    };
    let tick = |i: u64| {
        let delta = elapsed * (4 - i) / 4;
        if i == 4 {
            "Dernier point".to_owned()
        } else if delta >= 60 {
            format!("−{} min {:02}", delta / 60, delta % 60)
        } else {
            format!("−{delta} s")
        }
    };
    let mut plot = canvas(shape.view_box(1000., 200.))
        .h(height)
        .grow(1.)
        .key("timeline-plot");
    let interactive = matches!(s.page, Page::Diagnostics | Page::History);
    if interactive && total > 0 {
        let ticks = (0..total)
            .filter_map(|i| axis_t(i).map(|t| (x(i) / 1000., t)))
            .collect::<Vec<_>>();
        plot = plot.help("Cliquer ou glisser pour inspecter un échantillon. Les boutons Précédent et Suivant permettent aussi de se déplacer.")
            .on_drag(move |s: &mut State, drag, tx| {
                if drag.cancelled() || drag.w <= 0. { return; }
                let fraction = (drag.x/drag.w).clamp(0.,1.);
                let chosen = ticks.iter().min_by(|a,b| (a.0-fraction).abs().total_cmp(&(b.0-fraction).abs())).map(|p|p.1);
                if s.cursor == chosen { tx.live_only(); }
                else { s.cursor=chosen; tx.instant(); }
            });
    }
    card()
        .grow(1.)
        .min_w(0.)
        .key("chart")
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(label(title, "section", "text").grow(1.))
                .child(legend)
                .child(label(unit, "small", "muted")),
        )
        .child(row().gap(n("gapSmall")).child(axis).child(plot))
        .child((0..=4).fold(row().justify_between(), |r, i| {
            r.child(label(tick(i), "small", "muted"))
        }))
        .child(
            label(
                inspection.unwrap_or_else(|| {
                    if total < 2 {
                        "Historique en attente de mesures".into()
                    } else {
                        "Fenêtre glissante · données manquantes non interpolées".into()
                    }
                }),
                "small",
                "muted",
            )
            .truncate(),
        )
}

fn series(s: &State, i: usize) -> Vec<Option<f64>> {
    let history = if s.page == Page::Io {
        &s.io
    } else {
        &s.history
    };
    let last = history.back().map_or(0, |p| p.elapsed_ms);
    history
        .iter()
        .skip_while(|p| last.saturating_sub(p.elapsed_ms) >= s.period as u64 * 1000)
        .map(|p| p.values[i])
        .collect()
}

fn heading(title: &str, subtitle: &str) -> El {
    col()
        .gap(n("space"))
        .child(label(title, "title", "text").role("heading"))
        .child(label(subtitle, "body", "muted"))
}

fn panel(title: &str, subtitle: &str, color: &str) -> El {
    card()
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(el().size(7., 7.).radius(4.).fill(c(color)))
                .child(label(title, "section", "text")),
        )
        .child(label(subtitle, "body", "muted").wrap())
}

fn detail_metric<T: Copy>(title: &str, metric: Sample<T>, render: impl Fn(T) -> String) -> El {
    row()
        .gap(n("space"))
        .help(provenance(&metric))
        .child(label(title, "small", "muted").grow(1.))
        .child(label(
            metric.value.get().map_or("—".into(), render),
            "body",
            "text",
        ))
}

fn mib(value: u64) -> String {
    format!("{:.1} Mio", value as f64 / MIB as f64)
}

type IconTextures = HashMap<usize, (Weak<[u8]>, rune::Pixels)>;
thread_local! {
    static PROCESS_TEXTURES: RefCell<IconTextures> = RefCell::new(HashMap::new());
}

fn process_icon(p: &pulse_domain::Process, size: f32) -> El {
    let Some(icon) = p
        .icon
        .as_ref()
        .filter(|b| b.len() == pulse_domain::PROCESS_ICON_SIZE.pow(2) * 4)
    else {
        return row()
            .size(size, size)
            .radius(4.)
            .center()
            .fill(c("primaryTint"))
            .a11y_hidden()
            .child(label(
                p.name.chars().next().map_or_else(
                    || "·".to_owned(),
                    |ch| ch.to_uppercase().collect::<String>(),
                ),
                "small",
                "primary",
            ));
    };
    let pixels = PROCESS_TEXTURES.with_borrow_mut(|cache| {
        let key = icon.as_ptr() as usize;
        if let Some((owner, pixels)) = cache.get(&key)
            && owner.upgrade().is_some_and(|old| Arc::ptr_eq(&old, icon))
        {
            return pixels.clone();
        }
        if cache.len() >= 512 {
            cache.retain(|_, (owner, _)| owner.strong_count() > 0);
            if cache.len() >= 512 {
                cache.clear();
            }
        }
        let pixels = rune::Pixels::new();
        let size = pulse_domain::PROCESS_ICON_SIZE as u32;
        pixels.set(size, size, icon.to_vec());
        cache.insert(key, (Arc::downgrade(icon), pixels.clone()));
        pixels
    });
    el().size(size, size).pixels(&pixels).a11y_hidden()
}

fn process_name(s: &State, index: usize) -> El {
    let p = s.process(index);
    let mut line = row().gap(n("gapSmall")).center_y().min_w(0.);
    if let Some(meta) = s.tree_rows.get(index) {
        let key = p.key;
        let can_toggle = s.query.text.trim().is_empty() && !s.active_only;
        let expanded = !can_toggle || !s.collapsed.contains(&key);
        line = line.child(el().w((meta.depth.min(9) * 12) as f32).h(1.));
        line = line.child(if meta.has_children && can_toggle {
            let title = if expanded {
                "Replier la branche"
            } else {
                "Déplier la branche"
            };
            let id = format!("branch-{}", model::row_id(key));
            row()
                .key(id)
                .size(22., 24.)
                .center()
                .radius(n("tagRadius"))
                .hover(c("info").alpha(0.12))
                .role("button")
                .label(format!("{title} : {}", p.name))
                .focusable(true)
                .focus_ring(2., 1., c("primary"))
                .on_press(move |s: &mut State| s.toggle_branch(key))
                .child(label(if expanded { "▾" } else { "▸" }, "body", "muted"))
        } else {
            el().size(22., 24.)
        });
    }
    line.child(process_icon(p, 18.)).child(
        label(&p.name, "small", "text")
            .truncate()
            .min_w(0.)
            .shrink()
            .help(&p.name),
    )
}

fn processes(s: &State, width: f32) -> El {
    let k = kit();
    let compact = width < 1040.;
    let stack_details = width < 850.;
    let table = Table::new(
        |s: &State| &s.table,
        |s: &mut State| &mut s.table,
        |s: &State| s.shown.len(),
        |s: &State, i| Cow::Owned(model::row_id(s.process(i).key)),
    )
    .name("Processus")
    .column(
        TableColumn::new("name", "Nom")
            .flex(1.)
            .min(115.)
            .sortable(),
    )
    .column(
        TableColumn::new("pid", "PID")
            .width(63.)
            .align(Align::End)
            .sortable(),
    )
    .column(
        TableColumn::new("cpu", "CPU")
            .width(72.)
            .align(Align::End)
            .sortable(),
    )
    .column(
        TableColumn::new("private", "Mémoire privée")
            .width(126.)
            .align(Align::End)
            .sortable(),
    )
    .column(
        TableColumn::new("gpu", "GPU")
            .width(78.)
            .align(Align::End)
            .sortable(),
    );
    let table = if compact {
        table
    } else {
        table.column(
            TableColumn::new("ram", "RAM résid.")
                .width(105.)
                .align(Align::End)
                .sortable(),
        )
    };
    let table = table
        .cell(|s: &State, i, column| {
            let p = s.process(i);
            match column {
                "name" => Cell::el(process_name(s, i)),
                "pid" => Cell::text(p.key.pid.to_string()).mono(),
                "cpu" => load_cell(p.cpu.value.get(), "cpu"),
                "gpu" => load_cell(s.process_gpu(p.key), "gpu"),
                "ram" => Cell::text(p.working_set.value.get().map_or("—".into(), mib)).mono(),
                "private" => Cell::text(p.private_bytes.value.get().map_or("—".into(), mib)).mono(),
                _ => Cell::text(p.threads.value.get().map_or("—".into(), |v| v.to_string())).mono(),
            }
        })
        .on_select(|s, id, _| {
            let key = s.shown.iter().find_map(|&i| {
                let key = s.snapshot.processes[i].key;
                (model::row_id(key) == id).then_some(key)
            });
            if s.selected != key {
                s.tab = 0;
            }
            s.select_process(key);
        })
        .on_open(|s, _, _| s.tab = 1)
        .on_sort(|s, _, _, _| s.project())
        .style(table_style(&k))
        .zebra(true)
        .build(s, &k)
        .grow(1.)
        .min_h(0.);
    let mut details = col().padding(14.).gap(9.);
    if let Some(p) = s.detail() {
        details = details
            .child(
                row()
                    .gap(n("gapSmall"))
                    .center_y()
                    .child(process_icon(p, 28.))
                    .child(
                        label(&p.name, "section", "text")
                            .truncate()
                            .min_w(0.)
                            .shrink(),
                    ),
            )
            .child(
                row()
                    .center_y()
                    .gap(n("gapSmall"))
                    .child(label(format!("PID {}", p.key.pid), "small", "muted").grow(1.))
                    .child(action("close-process-detail", "Fermer", false, |s| {
                        s.select_process(None);
                        s.tab = 0;
                    })),
            )
            .child(["CPU", "RAM", "E/S", "GPU"].into_iter().enumerate().fold(
                row().gap(4.),
                |ui, (index, name)| {
                    ui.child(
                        action(&format!("tab-{index}"), name, s.tab == index, move |s| {
                            s.tab = index
                        })
                        .px(6.)
                        .grow(1.)
                        .center(),
                    )
                },
            ));
        if s.tab != 2 {
            details = details.child(inspector_trend(s));
        }
        if s.tab == 0 {
            details = details
                .child(detail_metric("CPU", p.cpu, |v| format!("{v:.1} %")))
                .child(detail_metric("Threads", p.threads, |v| v.to_string()))
                .child(detail_metric("Handles", p.handles, |v| v.to_string()))
                .child(detail_metric("Priorité", p.priority, |priority| {
                    priority.label().into()
                }))
                .child(label(
                    format!("PID parent déclaré : {}", p.parent_pid),
                    "small",
                    "muted",
                ));
            let path = match &p.image_path.value {
                Availability::Available(path) => path.as_ref(),
                _ => "Chemin non disponible",
            };
            details = details.child(
                label(path, "small", "muted")
                    .truncate()
                    .help(format!("{path}\n{}", provenance(&p.image_path))),
            );
        } else if s.tab == 1 {
            details = details
                .child(detail_metric("Working Set", p.working_set, mib))
                .child(detail_metric("Private Bytes", p.private_bytes, mib))
                .child(
                    label(
                        "Résidente et privée : deux mesures distinctes.",
                        "small",
                        "muted",
                    )
                    .wrap(),
                )
                .child(action(
                    "open-memory",
                    "Ouvrir le suivi mémoire",
                    true,
                    |s| s.navigate(Page::Memory),
                ));
        } else if s.tab == 2 {
            details = details
                .child(detail_metric("Lecture", p.io_read, |_| format_sample(p.io_read)))
                .child(detail_metric("Écriture", p.io_write, |_| format_sample(p.io_write)))
                .child(label("E/S totales Windows : fichiers, réseau et périphériques ne sont pas séparés par ce compteur.", "small", "muted").wrap());
        } else {
            let data = s.snapshot.gpu_processes.as_ref();
            let rows = data
                .into_iter()
                .flat_map(|d| d.rows.iter())
                .filter(|r| r.process == p.key);
            let mut count = 0;
            for r in rows.take(3) {
                count += 1;
                details = details
                    .child(label(
                        format!("GPU {} · {:08X}", r.adapter.physical, r.adapter.luid_low),
                        "small",
                        "muted",
                    ))
                    .child(detail_metric("Moteur le plus actif", r.activity, |x| {
                        format!("{x:.1} %")
                    }))
                    .child(detail_metric("VRAM dédiée", r.dedicated, gpu_memory))
                    .child(detail_metric("Mémoire partagée", r.shared, gpu_memory));
            }
            if count == 0 {
                let text = if data.is_none() && !s.demo {
                    "En attente des compteurs Windows."
                } else {
                    "Pas de mesure GPU disponible pour cette instance."
                };
                details = details.child(label(text, "small", "muted").wrap());
            }
        }
        details = details.child(
            action("copy-pid", "Copier le PID", false, |_| {}).on_press_tx(|s: &mut State, tx| {
                if let Some(p) = s.detail() {
                    tx.copy(p.key.pid.to_string());
                }
            }),
        );
        if !s.paused
            && p.key.pid > 4
            && p.key.pid != std::process::id()
            && s.gaming.commands.is_some()
            && !s.gaming.busy
            && !s.session_busy
        {
            details = details.child(action("measure-frames", "Mesurer les FPS…", false, |s| {
                if let Some(p) = s.detail() {
                    s.gaming.target = Some((p.key, p.name.clone()));
                    s.navigate(Page::Gaming);
                }
            }));
        }
        if s.command.is_some()
            && !s.action_pending
            && !s.paused
            && p.key.pid > 4
            && p.key.pid != std::process::id()
        {
            details = details.child(action(
                "ask-priority",
                "Modifier la priorité…",
                false,
                |s| {
                    if let Some(p) = s.detail() {
                        s.priority_picker = Some((p.key, p.name.clone()));
                    }
                },
            ));
        }
    } else {
        details = details
            .child(label("Détail du processus", "section", "text"))
            .child(
                label(
                    if s.selected.is_some() {
                        "Cette instance n’est plus observée."
                    } else {
                        "Sélectionnez une ligne pour inspecter ses métriques."
                    },
                    "body",
                    "muted",
                )
                .wrap(),
            );
    }
    let status = if let Some(code) = s.snapshot.process_error {
        format!("Énumération incomplète · erreur {code:#010x}")
    } else {
        format!(
            "{} affichés · {} correspondent · {} sans identité accessible",
            s.shown.len(),
            s.matched,
            s.snapshot.skipped
        )
    };
    let mut tools = row().center_y().gap(6.)
                .child(action("open-monitor","Moniteur",false,|s|s.navigate(Page::Monitor)))
                .child(action("process-tree", if compact {"Arbre"} else {"Arborescence"}, s.tree, |s| {
                    s.tree = !s.tree;
                    s.project();
                }).help("Tri entre frères ; l’heure de création doit être antérieure pour relier un parent. Les filtres n’affichent que les correspondances."))
                .child(action("active-filter", "CPU ≥ 1 %", s.active_only, |s| {
                    s.active_only = !s.active_only;
                    s.project();
                }));
    if !compact && s.tracked.is_some() {
        tools = tools.child(action("active-memory", "Suivi mémoire", false, |s| {
            s.navigate(Page::Memory)
        }));
    }
    if s.command.is_some()
        && !s.action_pending
        && !s.paused
        && let Some(p) = s
            .detail()
            .filter(|p| p.key.pid > 4 && p.key.pid != std::process::id())
    {
        let command = pulse_actions::Command {
            key: p.key,
            change: pulse_actions::Change::Terminate,
        };
        let name = p.name.clone();
        tools = tools.child(
            action("ask-terminate", "Terminer…", false, move |s| {
                if s.selected == Some(command.key) {
                    s.confirmation = Some((command, name.clone()));
                }
            })
            .border(1., c("danger").alpha(0.55)),
        );
    }
    let mut workspace = if stack_details && s.selected.is_some() {
        col()
    } else {
        row()
    };
    workspace = workspace
        .grow(1.)
        .min_h(0.)
        .child(
            col()
                .grow(1.)
                .min_w(0.)
                .min_h(0.)
                .child(if s.shown.is_empty() {
                    col().grow(1.).center().child(label(
                        if s.snapshot.utc_ms == 0 {
                            "Lecture des processus Windows…"
                        } else {
                            "Aucun processus à afficher"
                        },
                        "body",
                        "muted",
                    ))
                } else {
                    table
                }),
        );
    if s.selected.is_some() {
        let mut pane = col()
            .key("process-detail")
            .min_h(0.)
            .fill(c("surface"))
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(details);
        pane = if stack_details {
            pane.h(174.).border_sides([1., 0., 0., 0.], c("border"))
        } else {
            pane.w(if compact { 239. } else { 285. })
                .border_sides([0., 0., 0., 1.], c("border"))
        };
        workspace = workspace.child(pane);
    }
    let snap = &s.snapshot;
    let readout = [
        ("CPU", snap.cpu),
        ("Mémoire", snap.ram),
        ("GPU", snap.gpu),
        ("Disque", snap.disk),
        ("Réseau", snap.network),
    ];
    let readout = readout
        .into_iter()
        .take(if width < 760. { 3 } else { 5 })
        .fold(
            row()
                .key("system-readout")
                .h(34.)
                .center_y()
                .px(14.)
                .gap(15.)
                .fill(c("surface"))
                .border_sides([0., 0., 1., 0.], c("border")),
            |ui, (title, sample)| {
                ui.child(
                    row()
                        .center_y()
                        .gap(5.)
                        .child(label(title, "small", "muted"))
                        .child(label(format_sample(sample), "small", "text"))
                        .help(provenance(&sample)),
                )
            },
        );
    col()
        .grow(1.)
        .min_w(0.)
        .min_h(0.)
        .child(
            row()
                .center_y()
                .justify_between()
                .px(14.)
                .py(9.)
                .gap(n("gapSmall"))
                .border_sides([0., 0., 1., 0.], c("border"))
                .child(label("Processus", "title", "text").role("heading"))
                .child(tools),
        )
        .child(readout)
        .child(workspace)
        .child(
            label(status, "small", "muted")
                .key("process-count")
                .px(14.)
                .py(5.),
        )
        .child(if let Some(text) = &s.action_status {
            label(text, "small", "text").key("action-status")
        } else {
            el().h(0.)
        })
}

fn load_cell(value: Option<f64>, ink: &str) -> Cell {
    let Some(value) = value else {
        return Cell::text("—").mono();
    };
    Cell::el(
        row()
            .w_full()
            .center_y()
            .justify_end()
            .px(5.)
            .py(3.)
            .radius(3.)
            .fill(c(ink).alpha((value / 55.).clamp(0., 0.28) as f32))
            .child(label(format!("{value:.1} %"), "small", "text")),
    )
}

fn inspector_trend(s: &State) -> El {
    let index = if s.tab == 3 { 2 } else { s.tab.min(1) };
    let ink = ["cpu", "memory", "gpu"][index];
    let pts = &s.inspector_history;
    let max = if index == 1 {
        pts.iter()
            .filter_map(|p| p.values[index])
            .fold(1., f64::max)
            * 1.05
    } else {
        100.
    };
    let now = pts.back().map_or(0, |p| p.elapsed_ms);
    let mut shape = (1..=3).fold(Shape::new(), |shape, n| {
        let y = n as f32 * 20.;
        shape.line([0., y], [240., y], 1., LineCap::Butt, c("border"))
    });
    let mut segment = Vec::new();
    let mut previous = None;
    for p in pts {
        if previous.is_some_and(|last| p.elapsed_ms.saturating_sub(last) > 5_000)
            || p.values[index].is_none()
        {
            shape = filled_segment(shape, std::mem::take(&mut segment), ink, 76.);
        }
        if let Some(v) = p.values[index] {
            let x = 240. - now.saturating_sub(p.elapsed_ms) as f32 / 60_000. * 240.;
            segment.push([x, 76. - (v / max).clamp(0., 1.) as f32 * 71.]);
        }
        previous = Some(p.elapsed_ms);
    }
    shape = filled_segment(shape, segment, ink, 76.);
    col()
        .key("inspector-trend")
        .gap(4.)
        .py(8.)
        .border_sides([1., 0., 1., 0.], c("border"))
        .child(label(
            [
                "Charge du processus",
                "Mémoire privée",
                "Moteur GPU le plus actif",
            ][index],
            "small",
            "muted",
        ))
        .child(if pts.iter().any(|p| p.values[index].is_some()) {
            canvas(shape.view_box(240., 80.)).w_full().h(74.)
        } else {
            col()
                .h(58.)
                .center()
                .child(label("En attente de mesures", "small", "muted"))
        })
        .child(
            row()
                .justify_between()
                .child(label("−60 s", "small", "muted"))
                .child(label("Dernier relevé", "small", "muted")),
        )
}

fn tabs(s: &State, names: &[&str]) -> El {
    names
        .iter()
        .enumerate()
        .fold(row().gap(n("space")), |r, (i, name)| {
            r.child(action(&format!("tab-{i}"), name, s.tab == i, move |s| {
                s.tab = i;
                s.cursor = None;
            }))
        })
}

fn overview(s: &State, compact: bool) -> El {
    let snap = &s.snapshot;
    let metrics = [
        ("CPU", "diagnostic", snap.cpu, "cpu", "Charge globale"),
        ("Mémoire", "memory", snap.ram, "memory", "RAM physique"),
        ("GPU", "gpu", snap.gpu, "gpu", "Charge graphique"),
    ];
    let selected = s.tab.min(metrics.len() - 1);
    let cards = metrics.iter().enumerate().fold(
        row().gap(n("gapInner")),
        |ui, (i, &(title, icon, sample, ink, note))| {
            ui.child(
                dashboard_metric(title, icon, sample, ink, note)
                    .key(format!("overview-metric-{i}"))
                    .fill(c(if selected == i {
                        "primaryTint"
                    } else {
                        "surface"
                    }))
                    .border(1., c(if selected == i { "primary" } else { "border" }))
                    .hover(c("primaryTint"))
                    .role("button")
                    .label(format!("Afficher la courbe {title}"))
                    .focusable(true)
                    .focus_ring(2., 1., c("primary"))
                    .on_press(move |s: &mut State| s.tab = i),
            )
        },
    );
    let (title, _, _, ink, _) = metrics[selected];
    let trend = col()
        .grow(1.)
        .min_w(0.)
        .gap(n("gapSmall"))
        .child(label("En ce moment", "hero", "text").role("heading"))
        .child(
            label(
                "Choisis une mesure pour afficher son évolution.",
                "body",
                "muted",
            )
            .wrap(),
        )
        .child(cards)
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(label("Évolution récente", "section", "text").grow(1.))
                .child(periods(s)),
        )
        .child(chart(
            s,
            &format!("{title} · utilisation"),
            "%",
            100.,
            &[(title, ink, series(s, selected))],
        ));
    let mut top = card()
        .key("overview-top")
        .gap(n("gapSmall"))
        .child(label("Qui consomme ?", "section", "text"))
        .child(label(
            if selected == 1 {
                "Par mémoire privée"
            } else {
                "Par charge CPU"
            },
            "small",
            "muted",
        ));
    let ranked = snap.contributors.as_deref().map_or_else(
        || Cow::Owned(pulse_domain::top_contributors(&snap.processes)),
        Cow::Borrowed,
    );
    let mut ranking: Vec<_> = ranked
        .iter()
        .filter_map(|p| {
            Some((
                if selected == 1 {
                    p.private_bytes? as f64
                } else {
                    p.cpu_percent?
                },
                p,
            ))
        })
        .collect();
    ranking.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
    if ranking.is_empty() {
        top = top.child(label("Aucune mesure disponible pour le moment.", "small", "muted").wrap());
    }
    for (rank, p) in ranking.into_iter().take(4) {
        let key = p.key;
        let measure = if selected == 1 {
            mib(p.private_bytes.unwrap_or_default())
        } else {
            format!("{rank:.1} %")
        };
        top = top.child(
            row()
                .h(37.)
                .center_y()
                .gap(n("gapSmall"))
                .border_sides([0., 0., 1., 0.], c("border"))
                .hover(c("primaryTint"))
                .role("button")
                .label(format!("Inspecter {}", p.name))
                .focusable(true)
                .focus_ring(2., 1., c("primary"))
                .on_press(move |s: &mut State| {
                    s.navigate(Page::Processes);
                    s.select_process(Some(key));
                    s.tab = usize::from(selected == 1);
                })
                .child(glyph("process", "muted").size(16., 16.))
                .child(label(&p.name, "body", "text").min_w(0.).truncate().grow(1.))
                .child(label(measure, "small", "text")),
        );
    }
    top = top.child(row().child(action(
        "overview-processes",
        "Tous les processus",
        false,
        |s| s.navigate(Page::Processes),
    )));
    if selected == 2 {
        top = card()
            .key("overview-top")
            .child(label("GPU par application", "section", "text"))
            .child(
                label(
                    "Consultez le moteur actif et la mémoire vidéo utilisée par chaque processus.",
                    "body",
                    "muted",
                )
                .wrap(),
            )
            .child(row().child(action(
                "overview-gpu-processes",
                "Voir les processus GPU",
                true,
                |s| {
                    s.navigate(Page::Gpu);
                    s.tab = 3;
                },
            )));
    }
    let analysis = if compact {
        col().gap(n("gap")).child(trend).child(top)
    } else {
        row()
            .gap(n("gap"))
            .child(trend)
            .child(col().w(278.).justify_center().child(top))
    };
    col()
        .gap(n("gap"))
        .child(material_nav(s))
        .child(analysis)
        .child(
            row()
                .gap(n("gapInner"))
                .child(entry_point(
                    "overview-history",
                    "history",
                    "Explorer un pic",
                    "Chronologie et sessions",
                    Page::History,
                ))
                .child(entry_point(
                    "overview-gaming",
                    "game",
                    "Mesurer les FPS",
                    "Capture pour un jeu",
                    Page::Gaming,
                )),
        )
}

fn material_nav(s: &State) -> El {
    [
        (Page::Monitor, "direct-monitor", "Moniteur"),
        (Page::Overview, "direct-home", "Courbes"),
        (Page::Performance, "direct-cpu", "Détail CPU"),
        (Page::Gpu, "direct-gpu", "Détail GPU"),
        (Page::Io, "direct-io", "Disque et réseau"),
    ]
    .into_iter()
    .fold(
        row().key("direct-details").gap(6.),
        |ui, (page, id, title)| {
            ui.child(action(id, title, s.page == page, move |s| s.navigate(page)).px(8.))
        },
    )
}

fn entry_point(id: &str, icon: &str, title: &str, subtitle: &str, page: Page) -> El {
    row()
        .key(id)
        .grow(1.)
        .min_w(0.)
        .center_y()
        .gap(n("gapInner"))
        .padding(n("gapInner"))
        .radius(n("cardRadius"))
        .fill(c("surface"))
        .border(1., c("border"))
        .hover(c("primaryTint"))
        .role("button")
        .label(title)
        .focusable(true)
        .focus_ring(2., 1., c("primary"))
        .on_press(move |s: &mut State| s.navigate(page))
        .child(
            row()
                .size(34., 34.)
                .center()
                .radius(8.)
                .fill(c("primaryTint"))
                .child(glyph(icon, "primary")),
        )
        .child(
            col()
                .min_w(0.)
                .grow(1.)
                .child(label(title, "body", "text").truncate())
                .child(label(subtitle, "small", "muted").truncate()),
        )
        .child(label("›", "section", "primary"))
}

fn performance(s: &State) -> El {
    let mut cores = card().child(label("Processeurs logiques", "section", "text"));
    if s.snapshot.logical_cpus.is_empty() {
        let reason = if s.replay {
            "La session ne stocke pas le détail par processeur."
        } else if matches!(s.snapshot.logical_cpu_status, Availability::Pending) {
            "En attente des compteurs Windows."
        } else {
            "Compteurs détaillés non disponibles."
        };
        cores = cores.child(label(reason, "body", "muted"));
    } else {
        for chunk in s.snapshot.logical_cpus.chunks(4) {
            let line = chunk.iter().fold(row().gap(n("gapSmall")), |r, cpu| {
                let name = format!("CPU {}:{}", cpu.group, cpu.number);
                let frequency = if cpu.frequency.value.get().is_some() {
                    format_sample(cpu.frequency)
                } else {
                    "— MHz".into()
                };
                let help = format!("{}\n{}", provenance(&cpu.usage), provenance(&cpu.frequency));
                let mut tile = col()
                    .grow(1.)
                    .min_w(0.)
                    .padding(n("gapSmall"))
                    .gap(n("space"))
                    .radius(n("tagRadius"))
                    .fill(c("surfaceSecondary"))
                    .help(help)
                    .child(
                        row()
                            .child(label(name, "small", "text").grow(1.))
                            .child(label(format_sample(cpu.usage), "small", "text")),
                    )
                    .child(label(frequency, "small", "muted"));
                if let Some(v) = cpu.usage.value.get() {
                    tile = tile.child(
                        row().h(4.).w_full().radius(2.).fill(c("border")).child(
                            el().w_frac((v / 100.) as f32)
                                .h_full()
                                .radius(2.)
                                .fill(c("cpu")),
                        ),
                    );
                }
                r.child(tile)
            });
            cores = cores.child(line);
        }
        cores = cores.child(label("La fréquence est celle exposée par le compteur Windows, pas une mesure de fréquence effective.","small","muted"));
    }
    col()
        .gap(n("gap"))
        .child(
            row()
                .center_y()
                .child(
                    heading(
                        "Détail CPU",
                        "Charge globale, fréquence et activité par processeur logique.",
                    )
                    .grow(1.),
                )
                .child(periods(s)),
        )
        .child(metrics(&[("CPU", s.snapshot.cpu, "cpu", "Charge globale")]))
        .child(monitor::cpu_temperature(s))
        .child(chart(
            s,
            "Charge CPU",
            "%",
            100.,
            &[("CPU", "cpu", series(s, 0))],
        ))
        .child(cores)
}

fn io(s: &State, compact: bool) -> El {
    let shell = col()
        .gap(n("gap"))
        .child(heading(
            "Stockage et réseau",
            "Débits agrégés et historique de la session en cours.",
        ))
        .child(tabs(s, &["Stockage", "Réseau", "Connexions"]));
    if s.tab == 2 {
        return shell.child(network_connections(s));
    }
    if s.tab == 0 && !s.replay {
        return shell.child(storage::view(s, compact));
    }
    let disk = s.tab == 0;
    let mut values = series(s, s.tab.min(1));
    let peak = values.iter().flatten().copied().fold(0., f64::max);
    let (unit, divisor) = if peak >= MIB as f64 {
        ("Mio/s", MIB as f64)
    } else if peak >= 1024. {
        ("Kio/s", 1024.)
    } else {
        ("o/s", 1.)
    };
    for value in values.iter_mut().flatten() {
        *value /= divisor;
    }
    let limit = (peak / divisor / 4.).ceil().max(1.) * 4.;
    let (metric, color, scope) = if disk {
        (
            "Lecture + écriture",
            "disk",
            "Le compteur PhysicalDisk agrège les octets lus et écrits. Il ne mesure ni la latence, ni le débit d’un processus.",
        )
    } else {
        (
            "Réception + émission",
            "network",
            "Somme des interfaces matérielles Ethernet et Wi-Fi actives. Boucles locales, tunnels et interfaces virtuelles ne sont pas inclus.",
        )
    };
    let current = if disk {
        s.snapshot.disk
    } else {
        s.snapshot.network
    };
    let shell = if disk && !s.replay {
        shell.child(metrics(&[
            (
                "Latence lecture",
                s.snapshot.disk_read_latency,
                "disk",
                "Moyenne · disques physiques",
            ),
            (
                "Latence écriture",
                s.snapshot.disk_write_latency,
                "warning",
                "Moyenne · disques physiques",
            ),
        ]))
    } else {
        shell
    };
    shell
        .child(metrics(&[
            ("Disque", s.snapshot.disk, "disk", "Lecture et écriture"),
            ("Réseau", s.snapshot.network, "network", "Ethernet et Wi-Fi"),
        ]))
        .child(periods(s))
        .child(if peak > 0. || values.iter().any(Option::is_some) {
            chart(
                s,
                "Historique des débits",
                unit,
                limit,
                &[(metric, color, values)],
            )
        } else {
            panel(
                "Aucun débit mesuré sur cette période",
                &provenance(&current),
                "info",
            )
        })
        .child(panel("Périmètre de la mesure", scope, "info"))
}

fn network_connections(s: &State) -> El {
    use pulse_domain::Transport;
    let Some(data) = &s.snapshot.endpoints else {
        return if s.demo || s.replay {
            panel(
                "Connexions non enregistrées",
                "L’inventaire est disponible uniquement pendant la collecte locale. Aucune adresse ou port n’est conservé.",
                "info",
            )
        } else if s.paused {
            panel(
                "Affichage en pause",
                "Reprenez la collecte pour actualiser la liste.",
                "info",
            )
        } else {
            panel(
                "Lecture des connexions Windows",
                "La liste s’affichera après le prochain échantillon.",
                "info",
            )
        };
    };
    let query = s.network_query.text.trim();
    let number = query.parse::<u32>().ok();
    let protocol = if query.eq_ignore_ascii_case("tcp") {
        Some(Transport::Tcp)
    } else if query.eq_ignore_ascii_case("udp") {
        Some(Transport::Udp)
    } else {
        None
    };
    let filtered = data
        .entries
        .iter()
        .filter(|e| {
            query.is_empty()
                || protocol == Some(e.transport)
                || number.is_some_and(|n| {
                    n == e.pid
                        || n == u32::from(e.local.port())
                        || e.remote.is_some_and(|addr| n == u32::from(addr.port()))
                })
        })
        .collect::<Vec<_>>();
    let page_size = 13;
    let pages = filtered.len().div_ceil(page_size).max(1);
    let page = s.network_page.min(pages - 1);
    let edit = TextInput::new(&s.network_query)
        .placeholder("Filtrer par PID, port, TCP ou UDP…")
        .on_edit(|s: &mut State| &mut s.network_query)
        .on_change(|s: &mut State, _| s.network_page = 0)
        .build(&kit())
        .key("connections-search")
        .grow(1.);
    let columns = |texts: [&str; 5], header: bool| {
        let ink = if header { "muted" } else { "text" };
        row()
            .h(32.)
            .w_full()
            .gap(n("gapSmall"))
            .center_y()
            .px(n("gapSmall"))
            .fill(c(if header {
                "surfaceSecondary"
            } else {
                "surface"
            }))
            .border_sides([0., 0., 1., 0.], c("border"))
            .child(label(texts[0], "small", ink).w(68.))
            .child(
                label(texts[1], "small", ink)
                    .grow(1.)
                    .min_w(0.)
                    .truncate()
                    .help(texts[1]),
            )
            .child(
                label(texts[2], "small", ink)
                    .grow(1.)
                    .min_w(0.)
                    .truncate()
                    .help(texts[2]),
            )
            .child(label(texts[3], "small", ink).w(132.))
            .child(label(texts[4], "small", ink).w(70.))
    };
    let mut table = card().key("connections-table").gap(0.).child(columns(
        ["Proto", "Adresse locale", "Adresse distante", "État", "PID"],
        true,
    ));
    for endpoint in filtered.iter().skip(page * page_size).take(page_size) {
        let protocol = format!(
            "{}{}",
            if endpoint.transport == Transport::Tcp {
                "TCP"
            } else {
                "UDP"
            },
            if endpoint.local.is_ipv6() { "v6" } else { "v4" }
        );
        let local = endpoint.local.to_string();
        let remote = endpoint.remote.map_or("—".into(), |a| a.to_string());
        let state = match endpoint.tcp_state {
            None => "Port lié",
            Some(1) => "Fermé",
            Some(2) => "En écoute",
            Some(3 | 4) => "Négociation",
            Some(5) => "Établie",
            Some(6..=10) => "Fermeture",
            Some(11) => "TIME_WAIT",
            Some(12) => "Supprimée",
            Some(_) => "État inconnu",
        };
        table = table.child(columns(
            [&protocol, &local, &remote, state, &endpoint.pid.to_string()],
            false,
        ));
    }
    if filtered.is_empty() {
        table = table.child(
            label(
                "Aucun résultat parmi les tables accessibles",
                "body",
                "muted",
            )
            .padding(n("gapSmall")),
        );
    }
    let missing: Vec<_> = data
        .tables
        .iter()
        .zip(["TCPv4", "TCPv6", "UDPv4", "UDPv6"])
        .filter_map(|(status, name)| status.get().is_none().then_some(name))
        .collect();
    let details = format!(
        "IP Helper · UTC {} ms · {} correspondance(s) · {} hors limite{}",
        data.utc_ms,
        filtered.len(),
        data.omitted,
        if missing.is_empty() {
            String::new()
        } else {
            format!(" · indisponible : {}", missing.join(", "))
        }
    );
    col().gap(n("gapSmall"))
        .child(row().gap(n("gapSmall")).center_y().child(edit)
            .child(action("connections-prev","Précédent",false,move |s|s.network_page=page.saturating_sub(1)))
            .child(label(format!("{} / {pages}",page+1),"small","muted"))
            .child(action("connections-next","Suivant",false,move |s|s.network_page=(page+1).min(pages-1))))
        .child(table)
        .child(label(details,"small","muted").wrap().key("connections-count"))
        .child(label("PID fourni par Windows au moment de l’inventaire. Aucun nom de processus ni débit par connexion n’est déduit. Adresses non enregistrées.","small","muted").wrap())
}

fn gpu(s: &State) -> El {
    let title = heading(
        if s.demo {
            "GPU — Adaptateur de démonstration"
        } else {
            "GPU Inspector"
        },
        if s.demo {
            "Adaptateur de démonstration · scénario de rendu 3D"
        } else {
            "Adaptateurs et moteurs graphiques"
        },
    );
    if !s.demo {
        return gpu_live(s);
    }
    let vram = || {
        series(s, 2)
            .into_iter()
            .map(|v| v.map(|v| 27. + v * 0.45))
            .collect()
    };
    let mut result = col().gap(n("gap")).child(title).child(tabs(
        s,
        &["Aperçu", "Par processus", "Moteurs", "VRAM", "Températures"],
    ));
    let load = s.snapshot.gpu.value.get().unwrap_or_default();
    let engines = card()
        .grow(1.)
        .min_w(0.)
        .child(label("Moteurs GPU · fixture", "section", "text"))
        .child(bar("3D / Graphics", load, "cpu"))
        .child(bar("Compute", 18., "cpu"))
        .child(bar("Copy", 6., "cpu"))
        .child(bar("Video", 2., "cpu"));
    let consumers = card()
        .child(label("Processus utilisant le GPU", "section", "text"))
        .child(info_row("Studio Render.exe", "64 % · 10,4 Gio"))
        .child(info_row("Media Player.exe", "6 % · 1,1 Gio"))
        .child(info_row("Navigateur Atlas.exe", "3 % · 324 Mio"))
        .child(info_row("Desktop Shell.exe", "1 % · 128 Mio"))
        .child(label(
            "Répartition du scénario de démonstration",
            "small",
            "muted",
        ));
    if s.tab == 0 {
        result = result
            .child(
                row()
                    .gap(n("gapInner"))
                    .child(card().w(160.).center().child(gauge(load)))
                    .child(
                        card()
                            .grow(1.)
                            .min_w(0.)
                            .child(info_row(
                                "VRAM dédiée",
                                &format!("{:.1} %", 27. + load * 0.45),
                            ))
                            .child(info_row("Température", "64 °C"))
                            .child(info_row("Fréquence", "1 830 MHz"))
                            .child(info_row("Consommation", "124 W")),
                    )
                    .child(engines),
            )
            .child(periods(s))
            .child(
                row()
                    .gap(n("gapInner"))
                    .child(chart(
                        s,
                        "Utilisation & VRAM",
                        "%",
                        100.,
                        &[("GPU", "gpu", series(s, 2)), ("VRAM", "cpu", vram())],
                    ))
                    .child(consumers.w(280.)),
            );
    } else {
        result = match s.tab {
            1 => result.child(consumers),
            2 => result.child(engines).child(panel(
                "Lecture des compteurs",
                "Les pourcentages de moteurs indépendants ne s’additionnent pas.",
                "info",
            )),
            3 => result.child(periods(s)).child(chart(
                s,
                "VRAM dédiée",
                "%",
                100.,
                &[("VRAM", "cpu", vram())],
            )),
            _ => result.child(
                card()
                    .child(info_row("Température GPU · simulée", "64 °C"))
                    .child(info_row("Puissance · simulée", "124 W")),
            ),
        };
    }
    result
}

fn gpu_live(s: &State) -> El {
    let adapters = &s.snapshot.adapters;
    let menu = |s: &State| tabs(s, &["Aperçu", "Vidéo", "Capteurs", "Par processus"]);
    if s.tab == 3 {
        return col()
            .gap(n("gap"))
            .child(heading(
                "GPU · Par processus",
                "Mesures locales Windows WDDM",
            ))
            .child(menu(s))
            .child(gpu_process_table(s));
    }
    let Some(adapter) = s
        .gpu_selection
        .as_ref()
        .and_then(|id| adapters.iter().find(|a| &a.id == id))
        .or_else(|| adapters.first())
    else {
        let reason = if s.replay {
            "La session ne contient pas d’inventaire matériel."
        } else {
            "Aucun capteur NVIDIA accessible. L’onglet « Par processus » utilise les compteurs Windows indépendamment de la marque du GPU."
        };
        return col()
            .gap(n("gap"))
            .child(heading(
                "GPU",
                "Activité et capteurs de l’adaptateur graphique",
            ))
            .child(menu(s))
            .child(
                panel("Adaptateur indisponible", reason, "info").child(label(
                    provenance(&s.snapshot.gpu),
                    "small",
                    "muted",
                )),
            );
    };
    let mut shell = col().gap(n("gap")).child(heading(
        &format!("GPU — {}", adapter.name),
        "Mesures locales NVIDIA NVML · par adaptateur",
    ));
    if adapters.len() > 1 {
        let selector = adapters
            .iter()
            .enumerate()
            .fold(row().gap(n("space")), |r, (i, a)| {
                let id = a.id.clone();
                r.child(
                    action(
                        &format!("adapter-{i}"),
                        &format!("GPU {}", i + 1),
                        a.id == adapter.id,
                        move |s| s.gpu_selection = Some(id.clone()),
                    )
                    .help(&a.name),
                )
            });
        shell = shell.child(selector);
    }
    shell = shell.child(menu(s));
    let sensors = || {
        card()
            .grow(1.)
            .min_w(0.)
            .child(detail_metric(
                "Température GPU",
                adapter.temperature,
                |v| format!("{v:.0} °C"),
            ))
            .child(detail_metric("Fréquence graphique", adapter.clock, |v| {
                format!("{v:.0} MHz")
            }))
            .child(detail_metric("Puissance GPU", adapter.power, |v| {
                format!("{v:.1} W")
            }))
            .child(detail_metric(
                "Ventilateur 1 · consigne",
                adapter.fan,
                |v| format!("{v:.0} %"),
            ))
    };
    let video = || {
        card()
            .grow(1.)
            .min_w(0.)
            .child(label("Moteurs vidéo", "section", "text"))
            .child(detail_metric("Encodage", adapter.encode, |v| {
                format!("{v:.0} %")
            }))
            .child(detail_metric("Décodage", adapter.decode, |v| {
                format!("{v:.0} %")
            }))
            .child(
                label(
                    "Le détail WDDM 3D, Compute, Copy et par processus n’est pas collecté.",
                    "small",
                    "muted",
                )
                .wrap(),
            )
    };
    if s.tab == 1 {
        return shell.child(video());
    }
    if s.tab == 2 {
        return shell.child(sensors());
    }
    shell = shell.child(
        row()
            .gap(n("gapInner"))
            .child(
                card()
                    .w(174.)
                    .center()
                    .help(provenance(&adapter.utilization))
                    .child(
                        adapter
                            .utilization
                            .value
                            .get()
                            .map_or_else(|| label("Non mesuré", "body", "muted"), gauge),
                    ),
            )
            .child(sensors())
            .child(
                card()
                    .grow(1.)
                    .min_w(0.)
                    .child(label("Mémoire vidéo", "section", "text"))
                    .child(detail_metric(
                        "VRAM occupée / réservée",
                        adapter.vram_used,
                        |v| format!("{:.2} Gio", v / 1_073_741_824.),
                    ))
                    .child(detail_metric(
                        "Capacité physique",
                        adapter.vram_total,
                        |v| format!("{:.2} Gio", v / 1_073_741_824.),
                    ))
                    .child(
                        label(
                            "La VRAM partagée et le détail par processus ne sont pas mesurés.",
                            "small",
                            "muted",
                        )
                        .wrap(),
                    ),
            ),
    );
    if adapters.first().is_some_and(|a| a.id == adapter.id) {
        shell.child(periods(s)).child(
            row()
                .gap(n("gapInner"))
                .child(chart(
                    s,
                    "Historique de l’activité GPU",
                    "%",
                    100.,
                    &[("GPU", "gpu", series(s, 2))],
                ))
                .child(video().grow(0.5)),
        )
    } else {
        shell.child(panel(
            "Historique non enregistré",
            "Le graphique de session suit uniquement le premier adaptateur NVIDIA détecté.",
            "info",
        ))
    }
}

fn gpu_memory(bytes: f64) -> String {
    if bytes >= 1_073_741_824. {
        format!("{:.2} Gio", bytes / 1_073_741_824.)
    } else {
        format!("{:.1} Mio", bytes / MIB as f64)
    }
}

fn gpu_process_table(s: &State) -> El {
    let Some(data) = &s.snapshot.gpu_processes else {
        return if s.replay {
            panel(
                "Non enregistré",
                "Le détail GPU par processus est disponible en direct, pas dans les sessions enregistrées par cette version.",
                "info",
            )
        } else if s.paused {
            panel(
                "Affichage en pause",
                "Reprenez la collecte pour obtenir les compteurs GPU.",
                "info",
            )
        } else {
            panel(
                "Lecture des compteurs WDDM",
                "Patientez le temps de réunir deux échantillons du pilote.",
                "info",
            )
        };
    };
    let mut rows = data.rows.iter().collect::<Vec<_>>();
    if s.gpu_sort_memory {
        rows.sort_by(|a, b| {
            b.dedicated
                .value
                .get()
                .unwrap_or(-1.)
                .total_cmp(&a.dedicated.value.get().unwrap_or(-1.))
                .then_with(|| {
                    b.shared
                        .value
                        .get()
                        .unwrap_or(-1.)
                        .total_cmp(&a.shared.value.get().unwrap_or(-1.))
                })
        });
    }
    let per_page = 12;
    let pages = rows.len().div_ceil(per_page).max(1);
    let page = s.gpu_process_page.min(pages - 1);
    let columns = |titles: [&str; 6], header| {
        let ink = if header { "muted" } else { "text" };
        row()
            .h(32.)
            .gap(n("gapSmall"))
            .center_y()
            .px(n("gapSmall"))
            .border_sides([0., 0., 1., 0.], c("border"))
            .fill(c(if header {
                "surfaceSecondary"
            } else {
                "surface"
            }))
            .child(
                label(titles[0], "small", ink)
                    .grow(1.)
                    .min_w(0.)
                    .truncate()
                    .help(titles[0]),
            )
            .child(label(titles[1], "small", ink).w(75.))
            .child(
                label(titles[2], "small", ink)
                    .w(120.)
                    .truncate()
                    .help(titles[2]),
            )
            .child(
                label(titles[3], "small", ink)
                    .w(155.)
                    .truncate()
                    .help(titles[3]),
            )
            .child(label(titles[4], "small", ink).w(105.))
            .child(label(titles[5], "small", ink).w(115.))
    };
    let mut table = card().key("gpu-process-table").gap(0.).child(columns(
        [
            "Processus",
            "PID",
            "Adaptateur",
            "Moteur actif max.",
            "VRAM dédiée",
            "Partagée",
        ],
        true,
    ));
    for p in rows.iter().skip(page * per_page).take(per_page) {
        let adapter = format!("{} · {:08X}", p.adapter.physical, p.adapter.luid_low);
        let engine = p.activity.value.get().map_or_else(
            || "—".into(),
            |x| {
                format!(
                    "{} · {x:.1} %",
                    p.busiest_engine.as_deref().unwrap_or("Moteur")
                )
            },
        );
        let dedicated = p
            .dedicated
            .value
            .get()
            .map_or_else(|| "—".into(), gpu_memory);
        let shared = p.shared.value.get().map_or_else(|| "—".into(), gpu_memory);
        table = table.child(columns(
            [
                &p.name,
                &p.process.pid.to_string(),
                &adapter,
                &engine,
                &dedicated,
                &shared,
            ],
            false,
        ));
    }
    if rows.is_empty() {
        table = table.child(
            label("Aucune ligne GPU mesurable pour le moment", "body", "muted")
                .padding(n("gapSmall")),
        );
    }
    let summary = data
        .status
        .iter()
        .zip(["activité", "dédiée", "partagée"])
        .map(|(status, name)| {
            let status = match status {
                Availability::Available(_) => "disponible",
                Availability::Pending => "en attente",
                Availability::Unsupported => "non pris en charge",
                Availability::PermissionRequired => "accès refusé",
                Availability::Error(_) => "erreur",
            };
            format!("{name} : {status}")
        })
        .collect::<Vec<_>>()
        .join(" · ");
    col().gap(n("gapSmall"))
        .child(row().gap(n("gapSmall")).center_y()
            .child(action("gpu-sort-activity","Activité",!s.gpu_sort_memory,|s| {s.gpu_sort_memory=false;s.gpu_process_page=0;}))
            .child(action("gpu-sort-vram","VRAM dédiée",s.gpu_sort_memory,|s| {s.gpu_sort_memory=true;s.gpu_process_page=0;}))
            .child(el().grow(1.))
            .child(action("gpu-process-prev","Précédent",false,move |s|s.gpu_process_page=page.saturating_sub(1)))
            .child(label(format!("{} / {pages}",page+1),"small","muted"))
            .child(action("gpu-process-next","Suivant",false,move |s|s.gpu_process_page=(page+1).min(pages-1))))
        .child(table)
        .child(label(format!("WDDM · UTC {} ms · {summary} · {} mesures hors limite",data.utc_ms,data.excluded),"small","muted").wrap())
        .child(label("Maximum par moteur, pas une somme ni la charge globale de l’adaptateur. Les PID non vérifiables entre deux relevés ne sont pas affichés.","small","muted").wrap())
}

fn info_row(title: &str, value: &str) -> El {
    row()
        .min_h(32.)
        .center_y()
        .gap(n("gapSmall"))
        .border_sides([0., 0., 1., 0.], c("border"))
        .child(label(title, "small", "muted").grow(1.).truncate())
        .child(label(value, "body", "text").truncate())
}

fn bar(title: &str, value: f64, ink: &str) -> El {
    row()
        .center_y()
        .gap(n("gapSmall"))
        .child(label(title, "small", "text").w(100.).truncate())
        .child(
            row().grow(1.).h(8.).radius(4.).fill(c("border")).child(
                el().w_frac(value.clamp(0., 100.) as f32 / 100.)
                    .h_full()
                    .radius(4.)
                    .fill(c(ink)),
            ),
        )
        .child(label(format!("{value:.0} %"), "small", "muted").w(36.))
}

fn gauge(value: f64) -> El {
    let arc = |fraction: f64| {
        (0..=72)
            .map(|i| {
                let t = std::f64::consts::TAU * f64::from(i) / 72. * fraction
                    - std::f64::consts::FRAC_PI_2;
                [64. + 49. * t.cos() as f32, 64. + 49. * t.sin() as f32]
            })
            .collect::<Vec<_>>()
    };
    let shape = Shape::new().stroke(arc(1.), 10., c("border")).stroke(
        arc(value.clamp(0., 100.) / 100.),
        10.,
        c("gpu"),
    );
    col()
        .size(128., 128.)
        .center()
        .child(
            canvas(shape.view_box(128., 128.))
                .size(128., 128.)
                .absolute(0., 0.),
        )
        .child(label(format!("{value:.0} %"), "value", "text"))
        .child(label("Utilisation", "small", "muted"))
}

fn memory(s: &State) -> El {
    let mut result = col()
        .gap(n("gap"))
        .child(tabs(s, &["Fuite mémoire", "Interprétation"]))
        .child(panel(
            "Détection de fuite mémoire",
            "Surveille l’évolution de la mémoire pour repérer les croissances anormales.",
            "warning",
        ));
    if s.tab == 1 {
        return result.child(panel("Une tendance n’est pas une preuve","Un cache, un chargement ou un changement de charge peuvent expliquer une croissance de Private Bytes. Comparez des charges reproductibles et inspectez les allocations avec un profileur.","warning"));
    }
    if !s.demo {
        if s.replay {
            return result.child(panel(
                "Mesures individuelles non enregistrées",
                "Cette session contient uniquement des agrégats système.",
                "info",
            ));
        }
        let mut controls = row().gap(n("gapSmall")).child(action(
            "go-processes",
            "Choisir un processus",
            false,
            |s| s.navigate(Page::Processes),
        ));
        if let Some(p) = s.detail().filter(|p| Some(p.key) != s.tracked) {
            let key = p.key;
            controls = controls.child(action(
                "track-selected",
                "Suivre la sélection",
                false,
                move |s| {
                    s.tracked = Some(key);
                    s.memory.clear();
                },
            ));
        }
        result = result.child(controls);
        let Some(key) = s.tracked else {
            return result.child(panel("Aucun suivi actif","Sélectionnez un processus, puis démarrez le suivi. Un seul processus est suivi à la fois.","info"));
        };
        let observed = s.snapshot.processes.iter().find(|p| p.key == key);
        let data = |i: usize| {
            s.memory
                .iter()
                .map(|p| p.values[i].map(|b| b / MIB as f64))
                .collect::<Vec<_>>()
        };
        let values = [data(0), data(1)];
        let max = values
            .iter()
            .flatten()
            .filter_map(|v| *v)
            .fold(0., f64::max);
        let max = ((max / 128.).ceil() * 128.).max(128.);
        let [private, working] = values;
        let finding = pulse_insights::private_growth(
            s.memory
                .iter()
                .filter_map(|p| p.values[0].map(|v| (p.elapsed_ms, v as u64))),
        );
        return result
            .child(panel(
                "Instance surveillée",
                &observed.map_or_else(
                    || format!("PID {} · cette instance n’est plus observée.", key.pid),
                    |p| {
                        format!(
                            "{} · PID {} · échantillon toutes les 6 s environ",
                            p.name, key.pid
                        )
                    },
                ),
                "info",
            ))
            .child(
                row()
                    .gap(n("gapInner"))
                    .child(chart(
                        s,
                        "Évolution de la mémoire",
                        "Mio",
                        max,
                        &[
                            ("Private Bytes", "memory", private),
                            ("Working Set", "cpu", working),
                        ],
                    ))
                    .child(finding_panel(finding).w(250.)),
            );
    }
    // A separate fixed one-minute fixture, not inferred from the CPU chart.
    let test = |offset| {
        (0..31)
            .map(|i| Some(512. + offset + f64::from(i) * 9. + (f64::from(i) * 0.7).sin() * 14.))
            .collect()
    };
    let private: Vec<Option<f64>> = test(45.);
    let finding = pulse_insights::private_growth(
        private
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.map(|v| (i as u64 * 60_000, (v * MIB as f64) as u64))),
    );
    result
        .child(panel(
            "Studio Render.exe  ·  PID 4200",
            "Scénario indépendant · 31 échantillons espacés d’une minute",
            "info",
        ))
        .child(
            row()
                .gap(n("gapInner"))
                .child(chart(
                    s,
                    "Évolution de la mémoire",
                    "Mio",
                    1000.,
                    &[
                        ("Mémoire privée", "cpu", private),
                        ("Working Set", "network", test(-60.)),
                        ("Engagée · fixture", "memory", test(120.)),
                    ],
                ))
                .child(finding_panel(finding).w(250.)),
        )
        .child(s.snapshot.processes.first().map_or_else(el, memory_summary))
}

fn memory_summary(p: &pulse_domain::Process) -> El {
    let optional = |bytes: Option<u64>| bytes.map_or("—".into(), mib);
    [
        ("Mémoire privée", optional(p.private_bytes.value.get())),
        ("Working Set", optional(p.working_set.value.get())),
        (
            "Handles",
            p.handles.value.get().map_or("—".into(), |v| v.to_string()),
        ),
        (
            "Threads",
            p.threads.value.get().map_or("—".into(), |v| v.to_string()),
        ),
    ]
    .into_iter()
    .fold(row().gap(n("gapSmall")), |r, (title, value)| {
        r.child(
            card()
                .grow(1.)
                .min_w(0.)
                .gap(n("space"))
                .child(label(title, "small", "muted"))
                .child(label(value, "section", "text")),
        )
    })
}

fn finding_panel(finding: pulse_insights::Finding) -> El {
    use pulse_insights::Finding;
    match finding {
        Finding::Insufficient => panel(
            "Données insuffisantes",
            "MEM-01 v1 · au moins 30 min et 20 points valides sont nécessaires.",
            "info",
        ),
        Finding::Stable => panel(
            "Pas de croissance nette",
            "MEM-01 v1 · résultat limité aux mesures disponibles sur la période.",
            "success",
        ),
        Finding::Growth {
            delta_mib,
            slope_mib_hour,
            duration_ms,
            sustained,
        } => panel(
            if sustained {
                "Fuite potentielle"
            } else {
                "Croissance observée"
            },
            &format!(
                "MEM-01 v1 · +{delta_mib:.0} Mio sur {:.1} min · pente {slope_mib_hour:.0} Mio/h. Seuils : 256 Mio et 64 Mio/h, sans plateau tardif ni retour significatif. Charge et caches non exclus ; ce signal ne confirme pas une fuite.",
                duration_ms as f64 / 60_000.
            ),
            if sustained { "warning" } else { "info" },
        ),
    }
}

fn diagnostics(s: &State) -> El {
    let values = series(s, 0);
    let count = values.len();
    let start = s.history.len().saturating_sub(count);
    let index = s
        .cursor
        .and_then(|time| {
            s.history
                .iter()
                .skip(start)
                .position(|p| p.elapsed_ms == time)
        })
        .unwrap_or_else(|| {
            if s.cursor.is_some() {
                0
            } else {
                count.saturating_sub(1)
            }
        });
    let shown = s.history.get(start + index);
    let prev = s
        .history
        .get(start + index.saturating_sub(1))
        .map(|p| p.elapsed_ms);
    let next = s
        .history
        .get(start + (index + 1).min(count.saturating_sub(1)))
        .map(|p| p.elapsed_ms);
    let gpu = series(s, 2);
    let ram = series(s, 1);
    let mean = |series: &[Option<f64>]| {
        let mut n = 0;
        let sum = series.iter().flatten().inspect(|_| n += 1).sum::<f64>();
        if n == 0 {
            "—".into()
        } else {
            format!("{:.1} %", sum / f64::from(n))
        }
    };
    let max_ram = ram
        .iter()
        .flatten()
        .copied()
        .reduce(f64::max)
        .map_or("—".into(), |v| format!("{v:.1} %"));
    let averages = format!(
        "CPU moyen {}   ·   RAM max. {}   ·   GPU moyen {}",
        mean(&values),
        max_ram,
        mean(&gpu)
    );
    let history = s.page == Page::History;
    let assessed = shown.map(|p| p.elapsed_ms);
    let finding = pulse_insights::sustained_cpu(
        s.history
            .iter()
            .rev()
            .filter(|p| assessed.is_some_and(|at| p.elapsed_ms <= at))
            .map(|p| (p.elapsed_ms, p.values[0])),
    );
    let alert = match finding {
        pulse_insights::CpuLoad::Insufficient => panel(
            "Charge CPU · données insuffisantes",
            "CPU-01 · au moins 15 s de mesures valides, sans interruption de plus de 6 s.",
            "info",
        ),
        pulse_insights::CpuLoad::BelowThreshold => panel(
            "Aucune charge CPU soutenue détectée",
            "CPU-01 · seuil de 90 % pendant au moins 15 s. Le résultat ne s’applique qu’aux mesures disponibles.",
            "info",
        ),
        pulse_insights::CpuLoad::Sustained {
            duration_ms,
            peak_percent,
            count,
        } => panel(
            "Charge CPU soutenue",
            &format!(
                "CPU-01 · au moins 90 % pendant {:.1} s · pic à {peak_percent:.1} % · {count} mesures. Cette règle n’attribue pas la cause à un processus.",
                duration_ms as f64 / 1000.
            ),
            "warning",
        ),
    };
    let mut tools = row().gap(n("gapSmall")).child(action(
        "session-diagnostics",
        if history {
            "Afficher le diagnostic CPU"
        } else {
            "Masquer le diagnostic CPU"
        },
        !history,
        |s| {
            s.page = if s.page == Page::History {
                Page::Diagnostics
            } else {
                Page::History
            }
        },
    ));
    if !s.demo && (s.replay || s.history.len() > 1) && count > 0 {
        tools = tools.child(action("copy-csv","Copier le CSV",false, |_| {})
            .help("Copie la période visible : dates et métriques agrégées uniquement, sans nom de machine ni processus.")
            .on_press_tx(move |s: &mut State, tx| {
                tx.copy(pulse_history::csv_text(s.history.iter().skip(start).copied()));
            }));
    }
    let mut layout = col()
        .gap(n("gap"))
        .child(heading(
            "Sessions",
            "Enregistre une mesure ou clique sur un pic pour l’explorer.",
        ))
        .child(session_tools(s))
        .child(tools)
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(periods(s))
                .child(el().grow(1.))
                .child(action("back-point", "← Échantillon", false, move |s| {
                    s.cursor = prev
                }))
                .child(action("next-point", "Échantillon →", false, move |s| {
                    s.cursor = next
                }))
                .child(action(
                    "return-live",
                    if s.replay { "Dernier point" } else { "Direct" },
                    s.cursor.is_none(),
                    |s| s.cursor = None,
                )),
        )
        .child(chart(
            s,
            "Chronologie système",
            "%",
            100.,
            &[
                ("CPU", "cpu", values),
                ("RAM", "memory", ram),
                ("GPU", "disk", gpu),
            ],
        ))
        .child(
            label(
                shown.map_or_else(
                    || "Aucun échantillon.".into(),
                    |p| {
                        format!(
                            "Point {} / {} · +{:.1} s   ·   {averages}",
                            index + 1,
                            count,
                            p.elapsed_ms as f64 / 1000.
                        )
                    },
                ),
                "small",
                "muted",
            )
            .wrap(),
        )
        .child(if history {
            el().h(0.)
        } else {
            alert.key("cpu-diagnostic")
        })
        .child(contributors_panel(s, assessed));
    if !history
        && !s.demo
        && !s.replay
        && let Some(p) = s
            .snapshot
            .processes
            .iter()
            .find(|p| p.key.pid == std::process::id())
    {
        layout = layout.child(
            card()
                .child(label("Empreinte de PULSE", "section", "text"))
                .child(detail_metric("CPU machine", p.cpu, |v| format!("{v:.1} %")))
                .child(detail_metric("Mémoire privée", p.private_bytes, mib)),
        );
    }
    layout
}

fn contributors_panel(s: &State, time: Option<u64>) -> El {
    let Some(at) = time else {
        return panel(
            "Processus à cet instant",
            "Sélectionnez un point pour voir ses mesures par processus.",
            "info",
        );
    };
    let Some(snapshot) = s.contributor_history.iter().find(|p| p.elapsed_ms == at) else {
        return panel(
            "Processus à cet instant",
            if s.replay {
                "Aucun détail n’a été enregistré pour ce point. Pour un prochain enregistrement, activez « Inclure les processus »."
            } else {
                "Aucun détail de processus n’est disponible pour cet échantillon."
            },
            "info",
        );
    };
    let memory = s.contributors_by_memory;
    let mut rows = snapshot
        .entries
        .iter()
        .filter(|p| {
            if memory {
                p.private_bytes.is_some()
            } else {
                p.cpu_percent.is_some()
            }
        })
        .collect::<Vec<_>>();
    rows.sort_unstable_by(|a, b| {
        if memory {
            b.private_bytes
                .cmp(&a.private_bytes)
                .then_with(|| a.key.cmp(&b.key))
        } else {
            b.cpu_percent
                .unwrap()
                .total_cmp(&a.cpu_percent.unwrap())
                .then_with(|| a.key.cmp(&b.key))
        }
    });
    let header = row()
        .center_y()
        .gap(n("gapSmall"))
        .child(label("Processus à cet instant", "section", "text").grow(1.))
        .child(action("contributors-cpu", "CPU", !memory, |s| {
            s.contributors_by_memory = false
        }))
        .child(action(
            "contributors-memory",
            "Mémoire privée",
            memory,
            |s| s.contributors_by_memory = true,
        ));
    let mut ui = card().key("contributors").child(header);
    if rows.is_empty() {
        ui = ui.child(label(
            "Aucune mesure disponible pour ce classement.",
            "body",
            "muted",
        ));
    }
    for p in rows.into_iter().take(6) {
        let cpu = p
            .cpu_percent
            .map_or_else(|| "—".into(), |v| format!("{v:.1} %"));
        let ram = p.private_bytes.map_or_else(|| "—".into(), mib);
        ui = ui.child(
            row()
                .min_h(30.)
                .gap(n("gapSmall"))
                .center_y()
                .px(n("gapSmall"))
                .border_sides([0., 0., 1., 0.], c("border"))
                .child(label(&p.name, "body", "text").min_w(0.).truncate().grow(1.))
                .child(label(format!("PID {}", p.key.pid), "small", "muted").w(90.))
                .child(label(format!("CPU {cpu}"), "small", "text").w(105.))
                .child(label(format!("Privée {ram}"), "small", "text").w(155.)),
        );
    }
    ui.child(label("Classement des mesures disponibles. La présence d’un processus à cet instant n’établit pas la cause du pic.","small","muted").wrap())
}

fn session_tools(s: &State) -> El {
    use pulse_history::SessionCommand;
    use rune::FileDialogOptions;
    if s.session_commands.is_none() {
        return el().h(0.);
    }
    let status = if s.session_busy {
        "Traitement de la session…"
    } else if s.recording {
        "Enregistrement en cours · rétention selon la session"
    } else if s.replay {
        "Lecture seule · aucune collecte pendant la relecture"
    } else {
        "L’enregistrement est manuel. Les adresses et chemins ne sont pas stockés."
    };
    let mut buttons = row().gap(n("gapSmall")).center_y();
    if !s.session_busy {
        if s.recording {
            buttons = buttons.child(action("session-stop", "Arrêter", true, |s| {
                s.request_session(SessionCommand::Stop)
            }));
        } else {
            if s.can_return_live() {
                buttons = buttons.child(action(
                    "session-live",
                    "Revenir au direct",
                    true,
                    State::return_live,
                ));
            } else if !s.replay {
                buttons = buttons.child(
                    action("session-start", "Enregistrer…", true, |_| {})
                        .on_press_tx(|s: &mut State, tx| {
                            if s.session_busy {
                                return;
                            }
                            tx.save_file(
                                FileDialogOptions {
                                    title: "Enregistrer une session PULSE".into(),
                                    file_name: Some("session-pulse.sqlite".into()),
                                    ..Default::default()
                                }
                                .filter("Session PULSE", &["sqlite"]),
                                |s: &mut State, result, _| match result {
                                    Ok(Some(path)) => s.request_session(SessionCommand::Start {
                                        path,
                                        retain: 3600,
                                        include_processes: s.include_processes,
                                    }),
                                    Ok(None) => {}
                                    Err(e) => {
                                        s.recording_status =
                                            Some(format!("Sélecteur indisponible : {e}"))
                                    }
                                },
                            );
                        })
                        .help("Créer un nouveau fichier, avec rétention de 3 600 points."),
                );
            }
            if !s.replay {
                buttons=buttons.child(action("session-processes","Inclure les processus",s.include_processes,|s|s.include_processes = !s.include_processes)
                    .help("Active le stockage local des noms et identités des principaux processus. Ces champs ne sont pas exportés dans le CSV."));
            }
            buttons = buttons.child(
                action("session-open", "Ouvrir une session…", false, |_| {}).on_press_tx(
                    |s: &mut State, tx| {
                        if s.session_busy {
                            return;
                        }
                        tx.open_file(
                            FileDialogOptions {
                                title: "Ouvrir une session PULSE".into(),
                                ..Default::default()
                            }
                            .filter("Session PULSE", &["sqlite", "db"]),
                            |s: &mut State, result, _| match result {
                                Ok(Some(path)) => s.request_session(SessionCommand::Open(path)),
                                Ok(None) => {}
                                Err(e) => {
                                    s.recording_status =
                                        Some(format!("Sélecteur indisponible : {e}"))
                                }
                            },
                        );
                    },
                ),
            );
        }
    }
    card()
        .gap(n("gapSmall"))
        .child(label("Sessions locales", "section", "text"))
        .child(label(status, "small", "muted").wrap())
        .child(buttons)
        .child(if let Some(message) = &s.recording_status {
            label(message, "small", "text").wrap()
        } else {
            el().h(0.)
        })
}

fn gaming(s: &State) -> El {
    use pulse_gaming::Request;
    let g = &s.gaming;
    let panel_base = col().gap(n("gap")).child(heading(
        "Jeux",
        "Mesurez la fluidité de votre jeu : FPS, 1 % low et temps entre les images.",
    ));
    if g.commands.is_none() || s.replay {
        return panel_base.child(panel(
            "Capture non disponible dans cette vue",
            "Revenez au direct pour utiliser PresentMon ou importer un CSV.",
            "info",
        ));
    }
    if s.session_busy {
        return panel_base.child(panel(
            "Session en cours de traitement",
            "Attendez la fin de l’opération pour utiliser PresentMon.",
            "info",
        ));
    }
    let chosen = g.target.as_ref().and_then(|(key, name)| {
        s.snapshot
            .processes
            .iter()
            .any(|p| p.key == *key)
            .then(|| (*key, name.clone()))
    });
    let tool = |text, primary| {
        action("gaming-tool", text, primary, |_| {}).on_press_tx(|_: &mut State, tx| {
            tx.open_file(
                rune::FileDialogOptions {
                    title: "Choisir le CLI officiel PresentMon".into(),
                    ..Default::default()
                }
                .filter("Exécutable Windows", &["exe"]),
                |s: &mut State, result, _| match result {
                    Ok(Some(path)) => {
                        s.gaming.executable = Some(path);
                        s.gaming.status = None;
                    }
                    Ok(None) => {}
                    Err(e) => s.gaming.status = Some(e.to_string()),
                },
            );
        })
    };
    let duration_options = [15, 30, 60]
        .into_iter()
        .fold(row().gap(n("space")), |r, seconds| {
            r.child(action(
                &format!("capture-{seconds}"),
                &format!("{seconds} s"),
                g.seconds == seconds,
                move |s| s.gaming.seconds = seconds,
            ))
        });
    let executable = g
        .executable
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or_else(
            || {
                if g.bundled_presentmon {
                    "PresentMon 2.3.1 intégré".into()
                } else {
                    "Non configuré".into()
                }
            },
            |name| name.to_string_lossy().into_owned(),
        );
    let mut settings = card().key("gaming-primary").fill(c("primaryTint")).child(
        row()
            .center_y()
            .gap(n("gapSmall"))
            .child(glyph("game", "primary"))
            .child(label("Capture automatique", "section", "text")),
    );
    if g.busy {
        settings = settings.child(row().child(action("gaming-cancel", "Annuler", false, |s| {
            s.gaming.stop()
        })));
    } else if g.bundled_presentmon || g.executable.is_some() {
        settings=settings.child(label("Démarrez la capture, puis retournez directement dans le jeu. Les images hors premier plan ne comptent pas.","body","muted").wrap());
        if s.paused {
            settings = settings.child(label("Reprenez le direct pour capturer.", "body", "text"));
        } else {
            let button = action("gaming-auto", "Capturer en auto", true, move |s| {
                s.gaming.request(Request::Auto {
                    executable: s.gaming.executable.clone(),
                    seconds: s.gaming.seconds,
                });
            });
            settings = settings.child(
                row()
                    .center_y()
                    .gap(n("gapInner"))
                    .child(duration_options)
                    .child(button),
            );
        }
    } else {
        settings=settings.child(label("Build de développement sans PresentMon intégré. Choisissez le CLI officiel 2.3.1 x64.","body","muted").wrap())
            .child(row().child(tool("Configurer PresentMon…",true)));
    }
    if let Some(status) = &g.status {
        settings = settings.child(label(status, "body", "text").wrap());
    }
    let mut view = panel_base.child(settings);
    if let Some(report) = &g.report {
        if report.foreground_only {
            view = view.child(
                label(
                    "Capture auto · intervalles au premier plan uniquement.",
                    "small",
                    "muted",
                )
                .wrap(),
            );
        }
        view = view.child(gaming_report(report));
    }
    if !g.busy {
        let mut options = card()
            .key("gaming-options")
            .child(label("Autres options", "section", "text"))
            .child(
                row()
                    .gap(n("gapSmall"))
                    .child(
                        action("gaming-import", "Importer un CSV…", false, |_| {}).on_press_tx(
                            |_: &mut State, tx| {
                                tx.open_file(
                                    rune::FileDialogOptions {
                                        title: "Importer un CSV PresentMon".into(),
                                        ..Default::default()
                                    }
                                    .filter("CSV PresentMon", &["csv"]),
                                    |s: &mut State, result, _| match result {
                                        Ok(Some(path)) => s.gaming.request(Request::Import(path)),
                                        Ok(None) => {}
                                        Err(e) => s.gaming.status = Some(e.to_string()),
                                    },
                                );
                            },
                        ),
                    )
                    .child(action(
                        "gaming-target",
                        "Choisir un processus",
                        false,
                        |s| s.navigate(Page::Processes),
                    )),
            );
        if g.bundled_presentmon || g.executable.is_some() {
            options = options.child(
                row()
                    .center_y()
                    .gap(n("gapSmall"))
                    .child(tool("Choisir un autre CLI…", false))
                    .child(label(&executable, "small", "muted").truncate())
                    .child(if g.bundled_presentmon && g.executable.is_some() {
                        action(
                            "gaming-default-cli",
                            "Utiliser le CLI intégré",
                            false,
                            |s| s.gaming.executable = None,
                        )
                    } else {
                        el().w(0.)
                    }),
            );
            if let Some((process, name)) = chosen
                && !s.paused
            {
                options = options.child(
                    row()
                        .center_y()
                        .gap(n("gapSmall"))
                        .child(action(
                            "gaming-start",
                            "Capturer la cible",
                            false,
                            move |s| {
                                s.gaming.request(Request::Capture {
                                    executable: s.gaming.executable.clone(),
                                    process,
                                    seconds: s.gaming.seconds,
                                });
                            },
                        ))
                        .child(
                            label(format!("{name} · PID {}", process.pid), "small", "muted")
                                .truncate(),
                        ),
                );
            }
        }
        view = view.child(options);
    }
    view
}

fn gaming_report(r: &pulse_gaming::Report) -> El {
    let limit = r.chart.iter().map(|p| p[1]).fold(0., f64::max).max(1.);
    let max = (limit / 4.).ceil() * 4.;
    let sh = (0..=4).fold(Shape::new(), |sh, i| {
        let y = 6. + i as f32 * 47.;
        sh.line([0., y], [1000., y], 1., LineCap::Butt, c("border"))
    });
    let sh = filled_segment(
        sh,
        r.chart
            .iter()
            .map(|p| {
                [
                    (p[0] / r.total_ms * 1000.) as f32,
                    194. - (p[1] / max * 188.) as f32,
                ]
            })
            .collect(),
        "cpu",
        194.,
    );
    let axis = (0..=4).fold(col().h(218.).justify_between(), |ui, i| {
        ui.child(label(
            format!("{:.0} ms", max * f64::from(4 - i) / 4.),
            "small",
            "muted",
        ))
    });
    let metrics = [
        ("FPS moyens", format!("{:.1}", r.fps)),
        (
            "1 % low",
            r.low_one.map_or("—".into(), |v| format!("{v:.1} FPS")),
        ),
        ("Frametime P99", format!("{:.2} ms", r.p99_ms)),
        ("Intervalles", r.intervals.to_string()),
    ]
    .into_iter()
    .fold(row().gap(n("gapSmall")), |ui, (label_text, value)| {
        ui.child(
            card()
                .grow(1.)
                .min_w(0.)
                .child(label(label_text, "small", "muted"))
                .child(label(value, "value", "text")),
        )
    });
    col().key("gaming-report").gap(n("gap")).child(heading(&format!("{} · PID {}",r.name,r.pid),r.metric.label()))
        .child(metrics)
        .child(card().child(label("Frametimes","section","text"))
            .child(row().gap(n("gapSmall")).child(axis).child(canvas(sh.view_box(1000.,200.)).h(220.).grow(1.)))
            .child(row().justify_between().child(label("0 s","small","muted"))
                .child(label(format!("{:.1} s d’intervalles mesurés",r.total_ms/1000.),"small","muted"))))
        .child(panel("Lecture des résultats",&format!("Chaîne de présentation 0x{:X} · {} flux détecté(s), non mélangés. Le 1 % low est l’inverse de la durée moyenne des 1 % d’intervalles les plus lents (100 intervalles minimum). Le graphique conserve le maximum par bloc.",r.swapchain,r.streams),"info"))
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let mode = if s.replay {
        "Relecture — session SQLite"
    } else if s.demo {
        "Démo — données simulées"
    } else if s.paused {
        "En pause"
    } else if s.snapshot.utc_ms == 0 {
        "Connexion aux compteurs"
    } else {
        "En direct"
    };
    let width = cx.layout_size("shell").map_or(1440., |[w, _]| w);
    let search = if s.page == Page::Processes {
        TextInput::new(&s.query)
            .placeholder("Rechercher un processus ou PID…")
            .on_edit(|s: &mut State| &mut s.query)
            .on_change(|s: &mut State, _| s.project())
            .build(&kit())
            .key("search")
    } else {
        TextInput::new(&s.global_query)
            .placeholder("Rechercher un processus ou PID…")
            .on_edit(|s: &mut State| &mut s.global_query)
            .on_submit(|s: &mut State, tx| {
                s.query = rune::TextEdit::new(s.global_query.text.clone());
                s.navigate(Page::Processes);
                tx.focus("search", true);
            })
            .build(&kit())
            .key("global-search")
    }
    .grow(1.)
    .min_w(0.);
    let top = row()
        .h(n("topbar"))
        .px(n("gap"))
        .center_y()
        .gap(n("gapSmall"))
        .fill(c("surface"))
        .border_sides([0., 0., n("borderWidth"), 0.], c("border"))
        .child(
            row()
                .center_y()
                .gap(n("gapSmall"))
                .child(glyph("logo", "logo").size(27., 27.))
                .child(label("PULSE", "section", "text")),
        )
        .child(el().grow(1.))
        .child(if s.replay {
            el().w(0.)
        } else {
            search.max_w(if width < 880. { 250. } else { 390. })
        })
        .child(el().grow(1.));
    let mut status_bar = row()
        .h(34.)
        .px(n("gap"))
        .gap(n("gapSmall"))
        .center_y()
        .fill(c("surface"))
        .border_sides([n("borderWidth"), 0., 0., 0.], c("border"))
        .child(
            row()
                .radius(n("tagRadius"))
                .padding(n("gapSmall"))
                .gap(n("gapSmall"))
                .center_y()
                .fill(c("surfaceSecondary"))
                .child(el().size(6., 6.).radius(3.).fill(c(if s.demo || s.paused {
                    "warning"
                } else if !s.replay && s.snapshot.utc_ms != 0 {
                    "success"
                } else {
                    "muted"
                })))
                .child(label(mode, "small", "text")),
        )
        .child(el().grow(1.));
    if s.recording {
        status_bar = status_bar.child(label("● Enregistrement", "small", "danger"));
    }
    if s.replay && s.page != Page::History && s.can_return_live() {
        status_bar = status_bar.child(action(
            "status-return-live",
            "Revenir au direct",
            true,
            State::return_live,
        ));
    }
    if !s.replay {
        status_bar = status_bar.child(action(
            "pause",
            if s.paused { "Reprendre" } else { "Pause" },
            s.paused,
            |s| s.paused = !s.paused,
        ));
    }
    let content = if s.page == Page::Processes {
        processes(s, width - n("sidebarWidth"))
    } else {
        let page = match s.page {
            Page::Monitor => monitor::view(s, width - n("sidebarWidth")),
            Page::Overview => overview(s, cx.layout_size("shell").is_some_and(|[w, _]| w < 1030.)),
            Page::Performance | Page::Gpu | Page::Io => col()
                .gap(n("gap"))
                .child(material_nav(s))
                .child(match s.page {
                    Page::Performance => performance(s),
                    Page::Gpu => gpu(s),
                    _ => io(s, width < 1030.),
                }),
            Page::Memory => col()
                .gap(n("gap"))
                .child(
                    row().child(action("memory-back", "← Processus", false, |s| {
                        s.navigate(Page::Processes);
                        s.tab = 1;
                    })),
                )
                .child(heading(
                    "Suivi mémoire",
                    "Observe l’évolution de la mémoire d’un seul processus.",
                ))
                .child(memory(s)),
            Page::Diagnostics | Page::History => diagnostics(s),
            Page::Gaming => gaming(s),
            _ => unreachable!(),
        };
        col()
            .grow(1.)
            .min_h(0.)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(page.padding(n("gap")).w_full())
    };
    let shell = col()
        .key("shell")
        .w_full()
        .h_full()
        .fill(c("canvas"))
        .child(top)
        .child(row().grow(1.).min_h(0.).child(navigation(s)).child(content))
        .child(status_bar);
    if let Some((command, name)) = &s.confirmation {
        let pid = command.key.pid;
        let (title, body, confirm, ink) = match command.change {
            pulse_actions::Change::Terminate => (
                "Confirmer la terminaison",
                format!(
                    "Terminer {name} (PID {pid}) ? Les données non enregistrées pourraient être perdues."
                ),
                "Terminer ce processus",
                "danger",
            ),
            pulse_actions::Change::SetPriority(target) => (
                "Confirmer la priorité",
                format!(
                    "Appliquer la priorité « {} » à {name} (PID {pid}) ? Windows peut refuser cette demande.",
                    target.priority().label()
                ),
                "Appliquer la priorité",
                "primary",
            ),
        };
        let modal = col()
            .w_full()
            .h_full()
            .center()
            .fill(c("scrim").alpha(0.70))
            .absolute(0., 0.)
            .modal(false, |s: &mut State| s.confirmation = None)
            .child(
                card()
                    .w(420.)
                    .role("dialog")
                    .label(title)
                    .child(label(title, "section", "text"))
                    .child(label(body, "body", "text").wrap())
                    .child(
                        row()
                            .gap(n("gapSmall"))
                            .child(action("cancel-terminate", "Annuler", false, |s| {
                                s.confirmation = None
                            }))
                            .child(
                                action("confirm-terminate", confirm, false, State::confirm_action)
                                    .border(n("borderWidth"), c(ink)),
                            ),
                    ),
            );
        col().w_full().h_full().child(shell).child(modal)
    } else if let Some((key, name)) = &s.priority_picker {
        use pulse_actions::{Change, Command, PriorityTarget};
        let current = s
            .snapshot
            .processes
            .iter()
            .find(|p| p.key == *key)
            .and_then(|p| p.priority.value.get());
        let panel = card()
            .w(380.)
            .role("dialog")
            .label("Modifier la priorité")
            .child(label("Modifier la priorité", "section", "text"))
            .child(label(format!("{name} · PID {}", key.pid), "body", "muted").truncate())
            .child(
                label(
                    "La haute priorité et le temps réel ne sont pas proposés.",
                    "small",
                    "muted",
                )
                .wrap(),
            );
        let panel = [
            ("idle", PriorityTarget::Idle),
            ("low", PriorityTarget::Low),
            ("normal", PriorityTarget::Normal),
            ("above", PriorityTarget::AboveNormal),
        ]
        .into_iter()
        .fold(panel, |p, (id, target)| {
            p.child(action(
                &format!("priority-{id}"),
                target.priority().label(),
                current == Some(target.priority()),
                move |s| {
                    if let Some((key, name)) = s.priority_picker.take() {
                        s.confirmation = Some((
                            Command {
                                key,
                                change: Change::SetPriority(target),
                            },
                            name,
                        ));
                    }
                },
            ))
        })
        .child(action("cancel-priority", "Annuler", false, |s| {
            s.priority_picker = None
        }));
        col().w_full().h_full().child(shell).child(
            col()
                .w_full()
                .h_full()
                .center()
                .fill(c("scrim").alpha(0.70))
                .absolute(0., 0.)
                .modal(false, |s: &mut State| s.priority_picker = None)
                .child(panel),
        )
    } else {
        shell
    }
}
