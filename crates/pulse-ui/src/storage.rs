use super::{action, c, filled_segment, label, panel};
use crate::State;
use crate::{
    i18n::{tr, trf},
    translate_message,
};
use pulse_domain::{DriveSample, MIB};
use rune::{El, LineCap, Shape, canvas, col, el, row};

const PER_PAGE: usize = 6;
pub fn view(s: &State, compact: bool) -> El {
    let Some(data) = &s.storage else {
        return panel(
            tr("Inventaire des disques"),
            tr("Lecture des disques physiques Windows…"),
            "info",
        );
    };
    if data.drives.is_empty() {
        return panel(
            tr("Aucun disque à afficher"),
            &data.note.as_deref().map_or_else(
                || tr("Windows n’a renvoyé aucun disque physique.").into(),
                translate_message,
            ),
            "info",
        );
    }
    let pages = data.drives.len().div_ceil(PER_PAGE);
    let page = s.disk_page.min(pages - 1);
    let selected = data
        .drives
        .iter()
        .find(|d| Some(d.index) == s.disk_selected)
        .unwrap_or(&data.drives[page * PER_PAGE]);
    let mut listing = col()
        .key("storage-inventory")
        .radius(7.)
        .fill(c("surface"))
        .border(1., c("border"))
        .child(columns(
            [
                tr("Disque physique"),
                tr("Capacité"),
                tr("Activité"),
                tr("Lecture"),
                tr("Écriture"),
                tr("Latence L / É"),
                "Temp.",
                tr("Santé"),
            ],
            compact,
            true,
        ));
    for d in data.drives.iter().skip(page * PER_PAGE).take(PER_PAGE) {
        let index = d.index;
        let letters = d
            .volumes
            .iter()
            .map(|v| v.letter.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let model = trf!(
            ("{} · Disque {} · {}{}", "{} · Drive {} · {}{}"),
            d.model,
            d.index,
            tr(&d.bus),
            if letters.is_empty() {
                String::new()
            } else {
                format!(" · {letters}")
            }
        );
        let size = d.bytes.map_or_else(dash, capacity);
        let activity = d.busy_percent.map_or_else(dash, |v| format!("{v:.0} %"));
        let read = rate(d.read_bps);
        let write = rate(d.write_bps);
        let latency = latency(d);
        let temperature = d.temperature.map_or_else(dash, |v| format!("{v} °C"));
        listing = listing.child(
            columns(
                [
                    &model,
                    &size,
                    &activity,
                    &read,
                    &write,
                    &latency,
                    &temperature,
                    health(d.health),
                ],
                compact,
                false,
            )
            .key(format!("storage-disk-{index}"))
            .fill(c(if selected.index == index {
                "primaryTint"
            } else {
                "surface"
            }))
            .hover(c("primaryTint"))
            .role("button")
            .label(model)
            .focusable(true)
            .focus_ring(2., 1., c("primary"))
            .on_press(move |s: &mut State| s.disk_selected = Some(index)),
        );
    }
    let mut header = row().center_y().gap(8.).child(
        label(
            trf!(
                ("{} disques physiques", "{} physical drives"),
                data.drives.len()
            ),
            "section",
            "text",
        )
        .grow(1.),
    );
    if pages > 1 {
        header = header
            .child(action("storage-prev", "←", false, move |s| {
                s.disk_page = page.saturating_sub(1);
                s.disk_selected = None;
            }))
            .child(label(format!("{} / {pages}", page + 1), "small", "muted"))
            .child(action("storage-next", "→", false, move |s| {
                s.disk_page = (page + 1).min(pages - 1);
                s.disk_selected = None;
            }));
    }
    let stats = col()
        .grow(1.)
        .min_w(0.)
        .gap(9.)
        .padding(13.)
        .fill(c("surface"))
        .radius(7.)
        .border(1., c("border"))
        .child(label(tr("Débits de ce disque"), "section", "text"))
        .child(
            row()
                .gap(10.)
                .center_y()
                .child(label(tr("● Lecture"), "small", "cpu"))
                .child(label(tr("● Écriture"), "small", "gpu"))
                .child(label(tr("60 dernières secondes"), "small", "muted")),
        )
        .child(trend(s, selected.index))
        .child(detail(
            tr("Lecture / écriture"),
            format!("{} / {}", rate(selected.read_bps), rate(selected.write_bps)),
        ))
        .child(detail(
            tr("Temps actif"),
            selected
                .busy_percent
                .map_or_else(dash, |v| format!("{v:.0} %")),
        ))
        .child(detail(tr("Latence lecture / écriture"), latency(selected)));
    let mut volumes = col()
        .gap(8.)
        .padding(13.)
        .fill(c("surface"))
        .radius(7.)
        .border(1., c("border"))
        .child(label(tr("Volumes et espace"), "section", "text"));
    if selected.volumes.is_empty() {
        volumes = volumes.child(label(
            tr("Aucun volume avec lettre associé."),
            "small",
            "muted",
        ));
    }
    for volume in &selected.volumes {
        let used = volume
            .bytes
            .zip(volume.free_bytes)
            .and_then(|(total, free)| total.checked_sub(free).map(|used| (used, total)));
        volumes = volumes
            .child(
                label(
                    format!(
                        "{}  {}{}",
                        volume.letter,
                        volume.name,
                        if volume.spans_multiple_disks {
                            tr(" · multidisque")
                        } else {
                            ""
                        }
                    ),
                    "body",
                    "text",
                )
                .truncate(),
            )
            .child(detail(
                tr("Utilisé / total"),
                used.map_or_else(dash, |(used, total)| {
                    format!("{} / {}", capacity(used), capacity(total))
                }),
            ));
        if let Some((used, total)) = used.filter(|&(_, total)| total > 0) {
            volumes = volumes.child(
                row().h(5.).w_full().radius(2.).fill(c("border")).child(
                    el().w_frac(used as f32 / total as f32)
                        .h_full()
                        .fill(c("primary"))
                        .radius(2.),
                ),
            );
        }
    }
    let wear = selected
        .wear
        .map_or_else(dash, |v| trf!(("{v} % consommée", "{v}% used")));
    let reliability = col()
        .gap(8.)
        .padding(13.)
        .fill(c("surface"))
        .radius(7.)
        .border(1., c("border"))
        .child(label(tr("Santé et température"), "section", "text"))
        .child(detail(
            if selected.nvme_smart {
                tr("État Windows / NVMe")
            } else {
                tr("État Windows")
            },
            health(selected.health),
        ))
        .child(detail(
            tr("Température"),
            selected
                .temperature
                .map_or_else(dash, |v| format!("{v} °C")),
        ))
        .child(detail(tr("Endurance"), wear))
        .child(detail(
            tr("Temps allumé"),
            selected
                .power_on_hours
                .map_or_else(dash, |v| format!("{v} h")),
        ))
        .child(detail(
            tr("Total écrit (hôte)"),
            selected
                .written_bytes
                .map_or_else(dash, |n| trf!(("{:.2} To", "{:.2} TB"), n as f64 / 1e12)),
        ))
        .child(
            label(
                tr("— : le périphérique, le pilote ou Windows ne publie pas la mesure."),
                "small",
                "muted",
            )
            .wrap(),
        );
    let side = col().gap(10.).child(volumes).child(reliability);
    let detail_layout = if compact {
        col().gap(10.).child(stats).child(side)
    } else {
        row().gap(10.).child(stats).child(side.w(350.))
    };
    col()
        .gap(11.)
        .child(header)
        .child(listing)
        .child(if data.omitted {
            label(
                tr("Inventaire limité aux 64 premiers disques."),
                "small",
                "muted",
            )
        } else {
            el().h(0.)
        })
        .child(
            label(
                trf!(
                    ("Disque {} · {}", "Drive {} · {}"),
                    selected.index,
                    selected.model
                ),
                "section",
                "text",
            )
            .truncate(),
        )
        .child(detail_layout)
}

fn columns(values: [&str; 8], compact: bool, header: bool) -> El {
    let base = row()
        .h(if header { 30. } else { 40. })
        .px(10.)
        .gap(8.)
        .center_y()
        .fill(c("surfaceSecondary"))
        .border_sides([0., 0., 1., 0.], c("border"));
    let ink = if header { "muted" } else { "text" };
    values.iter().enumerate().fold(base, |row, (i, text)| {
        if compact && matches!(i, 5 | 7) {
            return row;
        }
        let entry = label(*text, "small", ink).truncate().help(*text);
        if i == 0 {
            row.child(entry.grow(1.).min_w(0.))
        } else {
            row.child(entry.w([0., 69., 55., 88., 88., 123., 53., 87.][i]))
        }
    })
}
fn detail(label_text: &str, value: impl Into<String>) -> El {
    row()
        .gap(5.)
        .center_y()
        .child(
            label(label_text, "small", "muted")
                .grow(1.)
                .min_w(0.)
                .truncate(),
        )
        .child(label(value, "small", "text"))
}
fn health(value: Option<u16>) -> &'static str {
    match value {
        Some(0) => tr("Pas d’alerte"),
        Some(1) => tr("Avertissement"),
        Some(2) => tr("Dégradé"),
        _ => tr("Non exposée"),
    }
}
fn dash() -> String {
    "—".into()
}
fn capacity(n: u64) -> String {
    if n >= 1024_u64.pow(4) {
        trf!(("{:.2} Tio", "{:.2} TiB"), n as f64 / 1024_f64.powi(4))
    } else {
        trf!(("{:.1} Gio", "{:.1} GiB"), n as f64 / 1024_f64.powi(3))
    }
}
fn rate(n: Option<f64>) -> String {
    n.map_or_else(dash, |v| {
        if v >= MIB as f64 {
            trf!(("{:.1} Mio/s", "{:.1} MiB/s"), v / MIB as f64)
        } else if v >= 1024. {
            trf!(("{:.1} Kio/s", "{:.1} KiB/s"), v / 1024.)
        } else {
            trf!(("{v:.0} o/s", "{v:.0} B/s"))
        }
    })
}
fn latency(d: &DriveSample) -> String {
    let f = |v: Option<f64>| v.map_or_else(dash, |v| format!("{v:.2} ms"));
    format!("{} / {}", f(d.read_ms), f(d.write_ms))
}
fn trend(s: &State, id: u32) -> El {
    let Some((_, points)) = s.storage_history.get(&id) else {
        return label(tr("Courbes en attente de relevés"), "small", "muted");
    };
    let now = points.back().map_or(0, |p| p.elapsed_ms);
    let maximum = points
        .iter()
        .flat_map(|p| p.values[..2].iter().flatten())
        .copied()
        .fold(1., f64::max);
    let mut grid = (1..=4).fold(Shape::new(), |sh, i| {
        sh.line(
            [0., i as f32 * 23.],
            [640., i as f32 * 23.],
            1.,
            LineCap::Butt,
            c("border"),
        )
    });
    for (i, ink) in ["cpu", "gpu"].into_iter().enumerate() {
        let mut run = Vec::new();
        let mut prev = None;
        for p in points {
            if p.values[i].is_none() || prev.is_some_and(|t| p.elapsed_ms.saturating_sub(t) > 5000)
            {
                grid = filled_segment(grid, std::mem::take(&mut run), ink, 92.);
            }
            if let Some(v) = p.values[i] {
                run.push([
                    640. - now.saturating_sub(p.elapsed_ms) as f32 * 640. / 60_000.,
                    92. - (v / maximum).clamp(0., 1.) as f32 * 87.,
                ]);
            }
            prev = Some(p.elapsed_ms);
        }
        grid = filled_segment(grid, run, ink, 92.);
    }
    col()
        .gap(4.)
        .child(label(
            trf!(("Échelle : 0 → {}", "Scale: 0 → {}"), rate(Some(maximum))),
            "small",
            "muted",
        ))
        .child(canvas(grid.view_box(640., 100.)).w_full().h(126.))
        .child(
            row()
                .justify_between()
                .child(label("−60 s", "small", "muted"))
                .child(label(tr("Dernier relevé"), "small", "muted")),
        )
}
