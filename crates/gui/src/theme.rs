//! Visual theme: palette, typography (system UI fonts) and egui style tuning.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Visuals,
};
use termbridge_core::types::SessionKind;

/// Named font family used for headings and emphasised labels.
pub const STRONG: &str = "strong";

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Window background behind every panel.
    pub bg: Color32,
    /// Side/top panels.
    pub surface: Color32,
    /// Cards inside panels.
    pub card: Color32,
    /// Inputs, hovered rows, inactive widgets.
    pub raised: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub term_bg: Color32,
    pub rx: Color32,
    pub tx: Color32,
    pub error: Color32,
    pub warn: Color32,
    pub nvt: Color32,
    pub ok: Color32,
}

impl Palette {
    pub fn kind_color(&self, kind: SessionKind) -> Color32 {
        match kind {
            SessionKind::Serial => self.accent,
            SessionKind::TcpClient => self.ok,
            SessionKind::TcpServer => self.warn,
            SessionKind::Udp => self.nvt,
        }
    }

    /// Translucent tint of `color` suitable for a badge or chip background.
    pub fn tint(&self, color: Color32) -> Color32 {
        let alpha = if self.dark { 38 } else { 28 };
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            dark,
            bg: Color32::from_rgb(0x0C, 0x0F, 0x14),
            surface: Color32::from_rgb(0x12, 0x16, 0x1D),
            card: Color32::from_rgb(0x18, 0x1D, 0x26),
            raised: Color32::from_rgb(0x21, 0x28, 0x33),
            border: Color32::from_rgb(0x26, 0x2E, 0x3A),
            border_strong: Color32::from_rgb(0x36, 0x41, 0x51),
            text: Color32::from_rgb(0xD9, 0xE0, 0xEA),
            text_strong: Color32::from_rgb(0xF4, 0xF7, 0xFB),
            muted: Color32::from_rgb(0x8A, 0x95, 0xA7),
            faint: Color32::from_rgb(0x5A, 0x65, 0x76),
            accent: Color32::from_rgb(0x4C, 0x9A, 0xFF),
            accent_soft: Color32::from_rgb(0x1A, 0x2D, 0x4A),
            on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            term_bg: Color32::from_rgb(0x09, 0x0C, 0x10),
            rx: Color32::from_rgb(0x5E, 0xD6, 0xC8),
            tx: Color32::from_rgb(0xF4, 0xB8, 0x5E),
            error: Color32::from_rgb(0xFF, 0x6E, 0x6E),
            warn: Color32::from_rgb(0xF5, 0xA5, 0x24),
            nvt: Color32::from_rgb(0xC0, 0x8C, 0xF8),
            ok: Color32::from_rgb(0x3E, 0xCF, 0x8E),
        }
    } else {
        Palette {
            dark,
            bg: Color32::from_rgb(0xEE, 0xF1, 0xF5),
            surface: Color32::from_rgb(0xF7, 0xF8, 0xFA),
            card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            raised: Color32::from_rgb(0xEC, 0xEF, 0xF4),
            border: Color32::from_rgb(0xDD, 0xE2, 0xE9),
            border_strong: Color32::from_rgb(0xC5, 0xCD, 0xD8),
            text: Color32::from_rgb(0x25, 0x2E, 0x3B),
            text_strong: Color32::from_rgb(0x0F, 0x16, 0x22),
            muted: Color32::from_rgb(0x5D, 0x69, 0x7B),
            faint: Color32::from_rgb(0x94, 0x9E, 0xAD),
            accent: Color32::from_rgb(0x1F, 0x6F, 0xEB),
            accent_soft: Color32::from_rgb(0xDF, 0xEA, 0xFD),
            on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            term_bg: Color32::from_rgb(0xFB, 0xFC, 0xFD),
            rx: Color32::from_rgb(0x0B, 0x7F, 0x74),
            tx: Color32::from_rgb(0xA8, 0x55, 0x05),
            error: Color32::from_rgb(0xD3, 0x2F, 0x2F),
            warn: Color32::from_rgb(0xB8, 0x6E, 0x00),
            nvt: Color32::from_rgb(0x7A, 0x3E, 0xD6),
            ok: Color32::from_rgb(0x0A, 0x8A, 0x55),
        }
    }
}

pub fn apply(ctx: &egui::Context, dark: bool) -> Palette {
    let p = palette(dark);
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r6 = CornerRadius::same(6);
    v.panel_fill = p.surface;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 140 } else { 40 }),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 120 } else { 30 }),
    };
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.term_bg;
    v.text_edit_bg_color = Some(p.raised);
    v.faint_bg_color = p.raised;
    v.code_bg_color = p.raised;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(
        p.accent.r(),
        p.accent.g(),
        p.accent.b(),
        if dark { 90 } else { 60 },
    );
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.striped = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r6;

    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r6;

    w.hovered.bg_fill = p.accent_soft;
    w.hovered.weak_bg_fill = blend(p.raised, p.text, 0.06);
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.hovered.corner_radius = r6;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = blend(p.raised, p.text, 0.10);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = r6;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.raised;
    w.open.weak_bg_fill = p.raised;
    w.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.open.corner_radius = r6;

    ctx.set_visuals(v);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.spacing.interact_size = egui::vec2(36.0, 28.0);
        style.spacing.indent = 16.0;
        style.spacing.combo_width = 120.0;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.window_margin = Margin::same(16);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(19.0, FontFamily::Name(STRONG.into())),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
    p
}

/// Linear blend of two colours (`t` = share of `b`).
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Load the platform's UI and monospace fonts so the app looks native.
/// Falls back to egui's bundled fonts when nothing is found.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let regular = first_font(UI_REGULAR);
    let strong = first_font(UI_STRONG);
    let mono = first_font(MONO);

    if let Some((name, data)) = regular {
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(data)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, name);
    }
    if let Some((name, data)) = mono {
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(data)));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, name);
    }
    let mut strong_family = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if let Some((name, data)) = strong {
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(data)));
        strong_family.insert(0, name);
    }
    fonts
        .families
        .insert(FontFamily::Name(STRONG.into()), strong_family);
    ctx.set_fonts(fonts);
}

#[cfg(target_os = "windows")]
const UI_REGULAR: &[&str] = &["segoeui.ttf"];
#[cfg(target_os = "windows")]
const UI_STRONG: &[&str] = &["seguisb.ttf", "segoeuib.ttf"];
#[cfg(target_os = "windows")]
const MONO: &[&str] = &["CascadiaMono.ttf", "consola.ttf"];

#[cfg(target_os = "macos")]
const UI_REGULAR: &[&str] = &["SFNS.ttf", "Helvetica.ttc", "HelveticaNeue.ttc"];
#[cfg(target_os = "macos")]
const UI_STRONG: &[&str] = &["SFNS.ttf", "HelveticaNeue.ttc"];
#[cfg(target_os = "macos")]
const MONO: &[&str] = &["SFNSMono.ttf", "Menlo.ttc", "Monaco.ttf"];

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UI_REGULAR: &[&str] = &[
    "Inter-Regular.ttf",
    "InterVariable.ttf",
    "NotoSans-Regular.ttf",
    "Cantarell-Regular.otf",
    "Ubuntu-R.ttf",
    "DejaVuSans.ttf",
    "LiberationSans-Regular.ttf",
];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UI_STRONG: &[&str] = &[
    "Inter-SemiBold.ttf",
    "NotoSans-SemiBold.ttf",
    "NotoSans-Bold.ttf",
    "Ubuntu-M.ttf",
    "DejaVuSans-Bold.ttf",
    "LiberationSans-Bold.ttf",
];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const MONO: &[&str] = &[
    "JetBrainsMono-Regular.ttf",
    "CascadiaMono.ttf",
    "FiraCode-Regular.ttf",
    "UbuntuMono-R.ttf",
    "DejaVuSansMono.ttf",
    "NotoSansMono-Regular.ttf",
    "LiberationMono-Regular.ttf",
];

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(PathBuf::from(windir).join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Fonts"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
            dirs.push(PathBuf::from(home).join(".fonts"));
        }
    }
    dirs
}

/// Find the first candidate file name in the font directories (searched up
/// to three levels deep, which covers the common Linux layouts).
fn first_font(candidates: &[&str]) -> Option<(String, Vec<u8>)> {
    let dirs = font_dirs();
    for name in candidates {
        for dir in &dirs {
            if let Some(path) = find_file(dir, name, 3) {
                if let Ok(data) = std::fs::read(&path) {
                    return Some((name.to_string(), data));
                }
            }
        }
    }
    None
}

fn find_file(dir: &std::path::Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}
