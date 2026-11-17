//! Colour themes, terminal capability detection and glyph sets.
//!
//! Themes are authored in 24-bit colour and adapted to what the terminal can
//! show: truecolor, the xterm 256-colour palette, the 16 ANSI colours, or no
//! colour at all (`NO_COLOR`). Glyphs fall back to plain ASCII on consoles
//! without good Unicode fonts (Linux VT, legacy Windows console).

use std::sync::RwLock;

use ratatui::style::Color;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

impl Depth {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "truecolor" | "24bit" | "true" | "rgb" => Some(Depth::TrueColor),
            "256" | "ansi256" => Some(Depth::Ansi256),
            "16" | "ansi" | "ansi16" | "basic" => Some(Depth::Ansi16),
            "none" | "mono" | "no" | "off" => Some(Depth::Mono),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Depth::TrueColor => "truecolor",
            Depth::Ansi256 => "256 colours",
            Depth::Ansi16 => "16 colours",
            Depth::Mono => "no colour",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GlyphLevel {
    /// Braille spinners, sparklines, rounded borders.
    Full,
    /// Common symbols only (dots, arrows), rounded borders.
    Basic,
    /// 7-bit ASCII.
    Ascii,
}

/// Detect the colour depth from the environment.
pub fn detect_depth() -> Depth {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
    if let Some(d) = Depth::parse(&var("TERMBRIDGE_COLORS")) {
        return d;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return Depth::Mono;
    }
    let colorterm = var("COLORTERM");
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return Depth::TrueColor;
    }
    if std::env::var_os("WT_SESSION").is_some() {
        return Depth::TrueColor;
    }
    let program = var("TERM_PROGRAM");
    if [
        "iterm.app",
        "wezterm",
        "vscode",
        "ghostty",
        "rio",
        "tabby",
        "hyper",
    ]
    .contains(&program.as_str())
    {
        return Depth::TrueColor;
    }
    let term = var("TERM");
    if term.contains("kitty")
        || term.contains("alacritty")
        || term.contains("foot")
        || term.contains("direct")
    {
        return Depth::TrueColor;
    }
    if term.contains("256color") || program == "apple_terminal" {
        return Depth::Ansi256;
    }
    if term == "linux" || term == "vt100" || term == "vt220" || term == "ansi" || term == "cons25" {
        return Depth::Ansi16;
    }
    if cfg!(windows) {
        // Windows 10+ consoles understand 24-bit escape sequences.
        return Depth::TrueColor;
    }
    if term.is_empty() || term == "dumb" {
        return Depth::Mono;
    }
    Depth::Ansi256
}

/// Detect how rich the glyph set may be.
pub fn detect_glyphs() -> GlyphLevel {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
    match var("TERMBRIDGE_GLYPHS").as_str() {
        "ascii" => return GlyphLevel::Ascii,
        "basic" => return GlyphLevel::Basic,
        "full" | "unicode" => return GlyphLevel::Full,
        _ => {}
    }
    let term = var("TERM");
    if term == "linux" || term == "vt100" || term == "vt220" || term == "dumb" || term == "cons25" {
        return GlyphLevel::Ascii;
    }
    if cfg!(windows) {
        // Windows Terminal ships Cascadia (braille, blocks); conhost fonts often don't.
        return if std::env::var_os("WT_SESSION").is_some() || !var("TERM_PROGRAM").is_empty() {
            GlyphLevel::Full
        } else {
            GlyphLevel::Basic
        };
    }
    let lang = format!("{}{}{}", var("LC_ALL"), var("LC_CTYPE"), var("LANG"));
    if !lang.is_empty() && !lang.contains("utf-8") && !lang.contains("utf8") {
        return GlyphLevel::Ascii;
    }
    GlyphLevel::Full
}

#[derive(Clone, Copy)]
pub struct Glyphs {
    pub level: GlyphLevel,
    pub on: &'static str,
    pub off: &'static str,
    pub pointer: &'static str,
    pub choice_l: &'static str,
    pub choice_r: &'static str,
    pub ellipsis: &'static str,
    pub caret: &'static str,
    pub sep: &'static str,
    pub vert: &'static str,
    pub horiz: &'static str,
    pub ok: &'static str,
    pub fail: &'static str,
    pub spinner: &'static [&'static str],
}

impl Glyphs {
    pub fn for_level(level: GlyphLevel) -> Self {
        match level {
            GlyphLevel::Ascii => Glyphs {
                level,
                on: "*",
                off: "o",
                pointer: "> ",
                choice_l: "<",
                choice_r: ">",
                ellipsis: "...",
                caret: "_",
                sep: "|",
                vert: "Up/Down",
                horiz: "Left/Right",
                ok: "OK",
                fail: "!!",
                spinner: &["|", "/", "-", "\\"],
            },
            GlyphLevel::Basic => Glyphs {
                level,
                on: "â—",
                off: "â—‹",
                pointer: "â€º ",
                choice_l: "â€¹",
                choice_r: "â€º",
                ellipsis: "â€¦",
                caret: "_",
                sep: "â”‚",
                vert: "â†‘/â†“",
                horiz: "â†/â†’",
                ok: "âœ“",
                fail: "âœ—",
                spinner: &["|", "/", "-", "\\"],
            },
            GlyphLevel::Full => Glyphs {
                level,
                on: "â—",
                off: "â—‹",
                pointer: "â€º ",
                choice_l: "â€¹",
                choice_r: "â€º",
                ellipsis: "â€¦",
                caret: "â–",
                sep: "â”‚",
                vert: "â†‘/â†“",
                horiz: "â†/â†’",
                ok: "âœ“",
                fail: "âœ—",
                spinner: &[
                    "â ‹", "â ™", "â ¹", "â ¸", "â ¼", "â ´", "â ¦", "â §", "â ‡", "â ",
                ],
            },
        }
    }
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub accent: Color,
    pub on_accent: Color,
    pub text: Color,
    pub strong: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub sel_bg: Color,
    pub rx: Color,
    pub tx: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub nvt: Color,
    /// Selection must use reverse video because there is no colour.
    pub mono: bool,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const THEMES: [Theme; 5] = [
    Theme {
        name: "Midnight",
        accent: rgb(0x4C9AFF),
        on_accent: rgb(0x0B0E14),
        text: rgb(0xD9E0EA),
        strong: rgb(0xF4F7FB),
        muted: rgb(0x8A95A7),
        faint: rgb(0x5A6576),
        border: rgb(0x364151),
        sel_bg: rgb(0x212833),
        rx: rgb(0x5ED6C8),
        tx: rgb(0xF4B85E),
        ok: rgb(0x3ECF8E),
        warn: rgb(0xF5A524),
        err: rgb(0xFF6E6E),
        nvt: rgb(0xC08CF8),
        mono: false,
    },
    Theme {
        name: "Arctic",
        accent: rgb(0x88C0D0),
        on_accent: rgb(0x2E3440),
        text: rgb(0xD8DEE9),
        strong: rgb(0xECEFF4),
        muted: rgb(0x9AA5B8),
        faint: rgb(0x616E88),
        border: rgb(0x4C566A),
        sel_bg: rgb(0x3B4252),
        rx: rgb(0x8FBCBB),
        tx: rgb(0xEBCB8B),
        ok: rgb(0xA3BE8C),
        warn: rgb(0xD08770),
        err: rgb(0xBF616A),
        nvt: rgb(0xB48EAD),
        mono: false,
    },
    Theme {
        name: "Ember",
        accent: rgb(0xFF9E64),
        on_accent: rgb(0x1A1B26),
        text: rgb(0xC0CAF5),
        strong: rgb(0xE8ECFF),
        muted: rgb(0x8B92B5),
        faint: rgb(0x565F89),
        border: rgb(0x3B4261),
        sel_bg: rgb(0x292E42),
        rx: rgb(0x7DCFFF),
        tx: rgb(0xE0AF68),
        ok: rgb(0x9ECE6A),
        warn: rgb(0xFF9E64),
        err: rgb(0xF7768E),
        nvt: rgb(0xBB9AF7),
        mono: false,
    },
    Theme {
        name: "Light",
        accent: rgb(0x1F6FEB),
        on_accent: rgb(0xFFFFFF),
        text: rgb(0x25303D),
        strong: rgb(0x0F1622),
        muted: rgb(0x5D697B),
        faint: rgb(0x8F99A8),
        border: rgb(0xC5CDD8),
        sel_bg: rgb(0xE3EAF4),
        rx: rgb(0x0B7F74),
        tx: rgb(0xA85505),
        ok: rgb(0x0A8A55),
        warn: rgb(0xB86E00),
        err: rgb(0xD32F2F),
        nvt: rgb(0x7A3ED6),
        mono: false,
    },
    Theme {
        name: "High contrast",
        accent: rgb(0x00D7FF),
        on_accent: rgb(0x000000),
        text: rgb(0xFFFFFF),
        strong: rgb(0xFFFFFF),
        muted: rgb(0xD0D0D0),
        faint: rgb(0xA8A8A8),
        border: rgb(0xFFFFFF),
        sel_bg: rgb(0x303030),
        rx: rgb(0x00FFAF),
        tx: rgb(0xFFD700),
        ok: rgb(0x00FF5F),
        warn: rgb(0xFFAF00),
        err: rgb(0xFF5F5F),
        nvt: rgb(0xD787FF),
        mono: false,
    },
];

pub fn theme_index(name: &str) -> usize {
    THEMES
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(name.trim()))
        .unwrap_or(0)
}

struct Active {
    theme: Theme,
    depth: Depth,
    glyphs: Glyphs,
    index: usize,
}

static ACTIVE: RwLock<Option<Active>> = RwLock::new(None);

/// Install a theme adapted to the colour depth.
pub fn set(index: usize, depth: Depth, glyphs: GlyphLevel) {
    let index = index % THEMES.len();
    let theme = adapt(THEMES[index], depth);
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Some(Active {
        theme,
        depth,
        glyphs: Glyphs::for_level(glyphs),
        index,
    });
}

fn with<R>(f: impl FnOnce(&Active) -> R) -> R {
    {
        let guard = ACTIVE.read().unwrap_or_else(|e| e.into_inner());
        if let Some(active) = guard.as_ref() {
            return f(active);
        }
    }
    set(0, Depth::TrueColor, GlyphLevel::Full);
    f(ACTIVE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .unwrap())
}

pub fn th() -> Theme {
    with(|a| a.theme)
}

pub fn glyphs() -> Glyphs {
    with(|a| a.glyphs)
}

pub fn depth() -> Depth {
    with(|a| a.depth)
}

pub fn index() -> usize {
    with(|a| a.index)
}

fn adapt(theme: Theme, depth: Depth) -> Theme {
    let map = |c: Color| match (depth, c) {
        (Depth::TrueColor, c) => c,
        (Depth::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
        (Depth::Ansi16, Color::Rgb(r, g, b)) => nearest_16(r, g, b),
        (Depth::Mono, _) => Color::Reset,
        (_, c) => c,
    };
    let mut t = Theme {
        name: theme.name,
        accent: map(theme.accent),
        on_accent: map(theme.on_accent),
        text: map(theme.text),
        strong: map(theme.strong),
        muted: map(theme.muted),
        faint: map(theme.faint),
        border: map(theme.border),
        sel_bg: map(theme.sel_bg),
        rx: map(theme.rx),
        tx: map(theme.tx),
        ok: map(theme.ok),
        warn: map(theme.warn),
        err: map(theme.err),
        nvt: map(theme.nvt),
        mono: depth == Depth::Mono,
    };
    if depth == Depth::Ansi16 {
        // Dim greys collapse to black on 16 colours; keep them readable.
        if t.faint == Color::Black {
            t.faint = Color::DarkGray;
        }
        if t.sel_bg == t.border || t.sel_bg == Color::Black {
            t.sel_bg = Color::DarkGray;
        }
    }
    t
}

/// Nearest colour in the xterm 256 palette (6x6x6 cube or grey ramp).
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let idx = |v: u8| {
        STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| (**s as i32 - v as i32).abs())
            .map(|(i, _)| i)
            .unwrap()
    };
    let (ri, gi, bi) = (idx(r), idx(g), idx(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let grey_i = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let grey_v = 8 + 10 * grey_i;
    let dist = |(a, b2, c): (u8, u8, u8)| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a, r) + d(b2, g) + d(c, b)
    };
    if dist((grey_v, grey_v, grey_v)) < dist(cube_rgb) {
        232 + grey_i
    } else {
        cube as u8
    }
}

/// Nearest of the 16 standard ANSI colours.
pub fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    const PALETTE: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (170, 0, 0)),
        (Color::Green, (0, 170, 0)),
        (Color::Yellow, (170, 85, 0)),
        (Color::Blue, (0, 0, 170)),
        (Color::Magenta, (170, 0, 170)),
        (Color::Cyan, (0, 170, 170)),
        (Color::Gray, (170, 170, 170)),
        (Color::DarkGray, (85, 85, 85)),
        (Color::LightRed, (255, 85, 85)),
        (Color::LightGreen, (85, 255, 85)),
        (Color::LightYellow, (255, 255, 85)),
        (Color::LightBlue, (85, 85, 255)),
        (Color::LightMagenta, (255, 85, 255)),
        (Color::LightCyan, (85, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    PALETTE
        .iter()
        .min_by_key(|(_, (pr, pg, pb))| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(*pr, r) + d(*pg, g) + d(*pb, b)
        })
        .map(|(c, _)| *c)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_mapping() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(128, 128, 128), 244);
        assert_eq!(nearest_16(250, 80, 80), Color::LightRed);
        assert_eq!(nearest_16(10, 10, 10), Color::Black);
    }

    #[test]
    fn mono_resets_colours() {
        let t = adapt(THEMES[0], Depth::Mono);
        assert_eq!(t.accent, Color::Reset);
        assert!(t.mono);
    }

    #[test]
    fn depth_parsing() {
        assert_eq!(Depth::parse("256"), Some(Depth::Ansi256));
        assert_eq!(Depth::parse("none"), Some(Depth::Mono));
        assert_eq!(Depth::parse("x"), None);
    }
}
