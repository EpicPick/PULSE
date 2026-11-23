#![forbid(unsafe_code)]
use rune::{Color, El, Theme, col, text, tok::type_};
use rune_kit::{Kit, PaletteSeed, TableStyle};
use std::sync::OnceLock;

pub fn tokens() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        Theme::from_json(include_str!("../../../design/tokens.json"), false)
            .expect("valid embedded PULSE tokens")
    })
}

pub fn c(name: &str) -> Color {
    tokens().color(name)
}
pub fn n(name: &str) -> f32 {
    tokens().num(name)
}

pub fn label(value: impl Into<String>, size: &str, ink: &str) -> El {
    let base = match size {
        "hero" | "title" | "value" | "section" => type_::TITLE,
        _ => type_::BODY,
    };
    text(value.into())
        .font(rune::TextStyle {
            size: n(size),
            ..base
        })
        .color(c(ink))
}

pub fn card() -> El {
    col()
        .padding(n("cardPadding"))
        .gap(n("gapInner"))
        .radius(n("cardRadius"))
        .fill(c("surface"))
        .border(n("borderWidth"), c("border"))
}

pub fn kit() -> Kit {
    let mut kit = Kit::from_palette(PaletteSeed {
        bg: c("surface"),
        fg: c("text"),
        accent: c("primary"),
    });
    kit.c.border_focus = c("primary");
    kit.c.border_base = c("borderStrong");
    kit.radius.sm = n("controlRadius");
    kit
}

pub fn table_style(kit: &Kit) -> TableStyle {
    TableStyle {
        row_height: n("row"),
        header_height: n("row"),
        muted: c("muted"),
        rule: c("border"),
        selected: c("primaryTint"),
        stripe: c("surface"),
        ..TableStyle::from_kit(kit)
    }
}
