use super::{action, c, filled_segment, format_sample, glyph, label, material_nav, provenance};
use crate::State;
use crate::i18n::{tr, trf};
use pulse_domain::{Availability, GpuAdapter, Sample, Unit};
use rune::{El, LineCap, Shape, canvas, col, el, row};

pub fn view(s: &State, width: f32) -> El {
    let snap = &s.snapshot;
    let adapter = s
        .gpu_selection
        .as_ref()
        .and_then(|id| snap.adapters.iter().find(|a| &a.id == id))
        .or_else(|| snap.adapters.first());
    let uptime = snap
        .monitor
        .uptime_ms
        .map_or_else(|| "—".into(), display_duration);
    let duration = snap.elapsed_ms;
    let context = if s.replay {
        tr("Lecture seule · seuls les agrégats enregistrés sont disponibles.").into()
    } else if s.paused {
        tr("Mesures figées · affichage en pause").into()
    } else {
        trf!(
            (
                "PC allumé depuis {uptime}  ·  PULSE observe depuis {}",
                "PC uptime: {uptime}  ·  PULSE monitoring for {}"
            ),
            display_duration(duration)
        )
    };
    let cpu_max = snap
        .logical_cpus
        .iter()
        .filter_map(|c| c.frequency.value.get())
        .reduce(f64::max)
        .map_or_else(|| "—".into(), |mhz| format!("{mhz:.0} MHz"));
    let cpu = tile(
        "monitor-cpu",
        tr("Processeur"),
        "diagnostic",
        "cpu",
        format_sample(snap.cpu),
    )
    .help(provenance(&snap.cpu))
    .child(trend(s, false, 0, "cpu", Some(100.)))
    .child(reading(tr("Fréquence reportée max."), cpu_max).help(tr(
        "Maximum des fréquences exposées par Windows ; ce n’est pas une fréquence effective.",
    )))
    .child(cpu_temperature(s));
    let ram = tile(
        "monitor-ram",
        tr("Mémoire vive"),
        "memory",
        "memory",
        format_sample(snap.ram),
    )
    .help(provenance(&snap.ram))
    .child(trend(s, false, 1, "memory", Some(100.)))
    .child(reading(
        tr("Utilisée / totale"),
        snap.monitor.ram_bytes.get().map_or_else(
            || "—".into(),
            |[used, total]| {
                trf!(
                    ("{:.1} / {:.1} Gio", "{:.1} / {:.1} GiB"),
                    gib(used as f64),
                    gib(total as f64)
                )
            },
        ),
    ))
    .child(reading(
        tr("Disponible"),
        snap.monitor.ram_bytes.get().map_or_else(
            || "—".into(),
            |[used, total]| {
                trf!(
                    ("{:.1} Gio", "{:.1} GiB"),
                    gib(total.saturating_sub(used) as f64)
                )
            },
        ),
    ));
    let gpu = tile(
        "monitor-gpu",
        tr("Carte graphique"),
        "gpu",
        "gpu",
        format_sample(adapter.map_or(snap.gpu, |a| a.utilization)),
    )
    .child(reading(
        tr("Température"),
        gpu_read(adapter, |a| a.temperature),
    ))
    .child(reading(tr("Puissance"), gpu_read(adapter, |a| a.power)))
    .child(reading(tr("Fréquence"), gpu_read(adapter, |a| a.clock)))
    .child(reading(tr("Ventilateur"), gpu_read(adapter, |a| a.fan)));
    let vram_pair = adapter
        .and_then(|a| a.vram_used.value.get().zip(a.vram_total.value.get()))
        .filter(|&(used, total)| total > 0. && used <= total);
    let vram_percent = vram_pair.map(|(used, total)| used / total * 100.);
    let mut vram = tile(
        "monitor-vram",
        tr("Mémoire vidéo"),
        "memory",
        "vram",
        vram_percent.map_or_else(|| "—".into(), |p| format!("{p:.1} %")),
    )
    .child(bar(vram_percent, "vram"))
    .child(reading(
        tr("Utilisée / totale"),
        vram_pair.map_or_else(
            || "—".into(),
            |(used, total)| {
                trf!(
                    ("{:.1} / {:.1} Gio", "{:.1} / {:.1} GiB"),
                    gib(used),
                    gib(total)
                )
            },
        ),
    ))
    .child(reading(
        tr("Libre"),
        vram_pair.map_or_else(
            || "—".into(),
            |(used, total)| trf!(("{:.1} Gio", "{:.1} GiB"), gib(total - used)),
        ),
    ));
    if adapter.is_none() && !s.replay {
        vram=vram.child(label(tr("Capteurs NVML indisponibles"), "small","muted").wrap()
            .help(tr("La capacité, la VRAM et les capteurs agrégés nécessitent actuellement un adaptateur NVIDIA exposé par NVML. L’onglet GPU par processus utilise WDDM.")));
    }
    let [read, write] = snap.monitor.disk_rates.map(rate);
    let disk = tile(
        "monitor-disk",
        tr("Disques physiques"),
        "io",
        "disk",
        format_sample(snap.disk),
    )
    .help(provenance(&snap.disk))
    .child(trend(s, true, 0, "disk", None))
    .child(reading(tr("Lecture"), read))
    .child(reading(tr("Écriture"), write))
    .child(reading(
        tr("Latence lecture / écriture"),
        format!(
            "{} / {}",
            format_sample(snap.disk_read_latency),
            format_sample(snap.disk_write_latency)
        ),
    ));
    let [rx, tx] = snap.monitor.network_rates.map(rate);
    let network = tile(
        "monitor-network",
        tr("Réseau"),
        "io",
        "network",
        format_sample(snap.network),
    )
    .help(provenance(&snap.network))
    .child(trend(s, true, 1, "network", None))
    .child(reading(tr("Réception"), rx))
    .child(reading(tr("Émission"), tx))
    .child(label(tr("Ethernet + Wi-Fi actifs"), "small", "muted"));
    let cols = if width < 430. {
        1
    } else if width < 850. {
        2
    } else {
        3
    };
    let mut grid = col().gap(12.);
    let mut line = row().gap(12.);
    for (i, tile) in [cpu, ram, gpu, vram, disk, network].into_iter().enumerate() {
        line = line.child(tile);
        if (i + 1) % cols == 0 {
            grid = grid.child(line);
            line = row().gap(12.);
        }
    }
    col()
        .key("monitor-overview")
        .gap(14.)
        .child(
            col()
                .gap(4.)
                .child(label(tr("Moniteur"), "title", "text").role("heading"))
                .child(label(context, "body", "muted").wrap()),
        )
        .child(material_nav(s))
        .child(grid)
        .child(adapter.map_or_else(el, |a| {
            let name = a.name.clone();
            let id = a.id.clone();
            let next = snap
                .adapters
                .iter()
                .position(|x| x.id == id)
                .map(|i| snap.adapters[(i + 1) % snap.adapters.len()].id.clone());
            let info = row().gap(10.).center_y().child(
                label(format!("GPU / VRAM : {name} · NVML"), "small", "muted")
                    .min_w(0.)
                    .truncate(),
            );
            if let Some(next) = next.filter(|_| snap.adapters.len() > 1) {
                info.child(action(
                    "monitor-next-gpu",
                    tr("GPU suivant"),
                    false,
                    move |s| s.gpu_selection = Some(next.clone()),
                ))
            } else {
                info
            }
        }))
        .child(
            label(
                tr("Courbes : 60 dernières secondes · — : mesure indisponible"),
                "small",
                "muted",
            )
            .wrap(),
        )
}

pub(crate) fn cpu_temperature(s: &State) -> El {
    let (value,hint)=match s.snapshot.monitor.cpu_thermal {
        Availability::Available(t)=>(format!("{:.1} °C",t.celsius),format!("HWiNFO · {}",t.label)),
        Availability::PermissionRequired=>(tr("Accès refusé").into(),tr("HWiNFO : partage de capteurs inaccessible à ce compte.").into()),
        _ if s.replay=>("—".into(),tr("La température CPU n’est pas enregistrée dans les sessions.").into()),
        _=>(tr("Non mesurée").into(),tr("Ouvrir les capteurs HWiNFO et activer Shared Memory Support. Le partage doit être actif et fournir une sonde CPU récente.").into()),
    };
    reading(tr("Température CPU"), value)
        .key("cpu-temperature")
        .help(hint)
}

fn tile(id: &str, title: &str, icon: &str, ink: &str, value: String) -> El {
    col()
        .key(id)
        .grow(1.)
        .min_w(0.)
        .padding(14.)
        .gap(7.)
        .radius(8.)
        .border(1., c("border"))
        .fill(c("surface"))
        .child(
            row()
                .gap(8.)
                .center_y()
                .child(glyph(icon, ink))
                .child(label(title, "section", "text").truncate()),
        )
        .child(label(value, "value", "text").truncate())
}

fn reading(name: &str, value: impl Into<String>) -> El {
    row()
        .gap(6.)
        .center_y()
        .child(label(name, "small", "muted").grow(1.).min_w(0.).truncate())
        .child(label(value, "small", "text"))
}

fn gpu_read(adapter: Option<&GpuAdapter>, sample: impl Fn(&GpuAdapter) -> Sample<f64>) -> String {
    adapter.map_or_else(|| "—".into(), |a| format_sample(sample(a)))
}

fn rate(rate: Availability<f64>) -> String {
    format_sample(Sample {
        utc_ms: 0,
        unit: Unit::BytesPerSecond,
        source: "",
        value: rate,
    })
}

fn gib(bytes: f64) -> f64 {
    bytes / 1_073_741_824.
}

fn display_duration(ms: u64) -> String {
    let sec = ms / 1000;
    if sec >= 86_400 {
        trf!(
            ("{} j {:02} h", "{} d {:02} h"),
            sec / 86_400,
            sec / 3600 % 24
        )
    } else if sec >= 3600 {
        format!("{} h {:02} min", sec / 3600, sec / 60 % 60)
    } else if sec >= 60 {
        format!("{} min {:02} s", sec / 60, sec % 60)
    } else {
        format!("{sec} s")
    }
}

fn bar(value: Option<f64>, ink: &str) -> El {
    row().h(9.).w_full().radius(4.).fill(c("border")).child(
        el().h_full()
            .radius(4.)
            .fill(c(ink))
            .w_frac(value.unwrap_or(0.).clamp(0., 100.) as f32 / 100.),
    )
}

fn trend(s: &State, io: bool, index: usize, ink: &str, max: Option<f64>) -> El {
    let history = if io { &s.io } else { &s.history };
    let now = history.back().map_or(0, |p| p.elapsed_ms);
    let start = now.saturating_sub(60_000);
    let points = history.iter().skip_while(|p| p.elapsed_ms < start);
    let max = max.unwrap_or_else(|| {
        points
            .clone()
            .filter_map(|p| p.values[index])
            .fold(1., f64::max)
    });
    let mut shape = Shape::new().line([0., 36.], [320., 36.], 1., LineCap::Butt, c("border"));
    let mut run = Vec::new();
    let mut previous = None;
    for p in points {
        if p.values[index].is_none()
            || previous.is_some_and(|t| p.elapsed_ms.saturating_sub(t) > 6_000)
        {
            shape = filled_segment(shape, std::mem::take(&mut run), ink, 36.);
        }
        if let Some(v) = p.values[index] {
            let x = 320. - now.saturating_sub(p.elapsed_ms) as f32 * 320. / 60_000.;
            run.push([x, 36. - (v / max).clamp(0., 1.) as f32 * 33.]);
        }
        previous = Some(p.elapsed_ms);
    }
    canvas(filled_segment(shape, run, ink, 36.).view_box(320., 38.))
        .w_full()
        .h(38.)
}
