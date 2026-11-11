// colors + fonts. palette is lifted from ampersand (github.com/ZIZIES/ampersand):
// warm near-black surfaces, cream text, and one swappable accent.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

#[derive(Clone, Copy)]
pub struct Pal {
    pub bg: Color32,   // main surface. translucent when opacity < 1 so the compositor's blur shows through
    pub side: Color32, // sidebar surface, same deal
    pub panel: Color32, // cards and popups: always solid
    pub inset: Color32,
    pub raised: Color32,
    pub raised_hover: Color32,
    pub line: Color32,
    pub line_soft: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub err: Color32,
    pub good: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub cream: Color32,
}

// (name, accent, accent-hover, text-on-accent, light square). same six as ampersand's board themes.
pub const ACCENTS: [(&str, u32, u32, u32, u32); 6] = [
    ("coral", 0xe58f6f, 0xec9f82, 0x1f120c, 0xf2e6d9),
    ("green", 0x8cb35c, 0x9cc46b, 0x14170f, 0xebecd0),
    ("brown", 0xcfa072, 0xdab084, 0x1c140c, 0xf0d9b5),
    ("blue", 0x72aed0, 0x86bbd9, 0x0d171d, 0xdee3e6),
    ("slate", 0x95a3b8, 0xa5b2c5, 0x12161c, 0xd3d8de),
    ("purple", 0xa98fd6, 0xb8a1de, 0x17121f, 0xe9e3f2),
];

// same rgb, see-through. egui wants premultiplied alpha, from_rgba_unmultiplied does that for us.
fn glass(c: u32, opacity: f32) -> Color32 {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

pub fn pal(accent: usize, opacity: f32) -> Pal {
    let a = ACCENTS[accent % ACCENTS.len()];
    Pal {
        bg: glass(0x161512, opacity),
        side: glass(0x201e1b, opacity),
        panel: hex(0x201e1b),
        inset: hex(0x181714),
        raised: hex(0x2b2925),
        raised_hover: hex(0x35322d),
        line: hex(0x34312c),
        line_soft: hex(0x2a2824),
        text: hex(0xece9e3),
        dim: hex(0x9b968e),
        faint: hex(0x6c6861),
        err: hex(0xe0705f),
        good: hex(0x8cb35c),
        accent: hex(a.1),
        accent_hover: hex(a.2),
        on_accent: hex(a.3),
        cream: hex(a.4),
    }
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        f.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("inter", include_bytes!("../assets/Inter-Regular.ttf"));
    add("inter-medium", include_bytes!("../assets/Inter-Medium.ttf"));
    add("inter-semibold", include_bytes!("../assets/Inter-SemiBold.ttf"));
    add("jbmono", include_bytes!("../assets/JetBrainsMono-Regular.ttf"));

    // egui's bundled fonts stay behind ours as fallbacks (emoji, symbols).
    let fallback = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &str| {
        let mut v = vec![first.to_owned()];
        v.extend(fallback.iter().cloned());
        v
    };
    f.families.insert(FontFamily::Proportional, with("inter"));
    f.families.insert(medium(), with("inter-medium"));
    f.families.insert(semibold(), with("inter-semibold"));
    f.families.entry(FontFamily::Monospace).or_default().insert(0, "jbmono".to_owned());

    // egui can't draw color emoji, so borrow a monochrome symbols font from the system if there is
    // one. it goes last in every family: only used for glyphs nothing else has.
    for (i, path) in ["/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf", "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf"].iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("symbols{i}");
            f.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            for list in f.families.values_mut() {
                list.push(name.clone());
            }
        }
    }
    ctx.set_fonts(f);
}

pub fn apply(ctx: &egui::Context, p: &Pal) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.extreme_bg_color = p.inset;
    v.faint_bg_color = p.inset;
    v.code_bg_color = p.raised;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);

    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.raised_hover;
    w.hovered.weak_bg_fill = p.raised_hover;
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = p.raised_hover;
    w.active.weak_bg_fill = p.raised_hover;
    w.active.bg_stroke = Stroke::NONE;
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open.bg_fill = p.raised;
    w.open.weak_bg_fill = p.raised;
    w.open.bg_stroke = Stroke::NONE;
    w.open.corner_radius = r;
    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Body, sans(15.0)),
            (TextStyle::Button, sans(14.0)),
            (TextStyle::Small, sans(12.0)),
            (TextStyle::Heading, FontId::new(22.0, semibold())),
            (TextStyle::Monospace, mono(13.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 6.0);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.interaction.selectable_labels = true;
    });
}
