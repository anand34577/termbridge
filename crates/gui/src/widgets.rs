//! Small reusable widgets: cards, badges, status dots, segmented controls, stat tiles.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke,
    StrokeKind, Vec2,
};

use crate::theme::{Palette, STRONG};

pub fn strong_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Bordered card with comfortable padding.
pub fn card<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, add_contents)
}

/// Card with tighter padding, used for toolbars.
pub fn bar<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, add_contents)
}

/// Heading for a card section.
pub fn section_title(ui: &mut egui::Ui, p: &Palette, title: &str) {
    ui.label(
        RichText::new(title)
            .font(strong_font(14.5))
            .color(p.text_strong),
    );
}

/// Muted helper text.
pub fn hint(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.label(RichText::new(text).size(12.5).color(p.muted))
}

/// Small rounded pill with a tinted background.
pub fn badge(ui: &mut egui::Ui, p: &Palette, text: &str, color: Color32) -> Response {
    let font = strong_font(10.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + Vec2::new(12.0, 5.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), p.tint(color));
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// Filled status dot with an optional soft halo.
pub fn dot(ui: &mut egui::Ui, color: Color32, halo: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    if halo {
        ui.painter().circle_filled(
            rect.center(),
            5.5,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 50),
        );
    }
    ui.painter().circle_filled(rect.center(), 3.5, color);
    response
}

/// 0..1 sine pulse at `hz`, driven by the frame clock.
pub fn pulse(ui: &egui::Ui, hz: f64) -> f32 {
    let t = ui.input(|i| i.time);
    (0.5 + 0.5 * (t * hz * std::f64::consts::TAU).sin()) as f32
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Status pill whose dot animates: `Busy` shows a spinner, `Pulse` breathes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PillAnim {
    None,
    Busy,
    Pulse,
}

pub fn status_pill_anim(
    ui: &mut egui::Ui,
    p: &Palette,
    text: &str,
    color: Color32,
    anim: PillAnim,
) -> Response {
    egui::Frame::new()
        .fill(p.tint(color))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin {
            left: 6,
            right: 10,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                match anim {
                    PillAnim::Busy => {
                        ui.add(egui::Spinner::new().size(10.0).color(color));
                    }
                    PillAnim::Pulse => {
                        let a = 0.35 + 0.65 * pulse(ui, 1.0);
                        dot(ui, with_alpha(color, a), true);
                    }
                    PillAnim::None => {
                        dot(ui, color, false);
                    }
                }
                ui.label(RichText::new(text).font(strong_font(12.0)).color(color));
            });
        })
        .response
}

/// Activity light: bright while `lit`, dim otherwise.
pub fn activity_led(ui: &mut egui::Ui, p: &Palette, label: &str, color: Color32, lit: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        dot(ui, if lit { color } else { p.faint }, lit);
        ui.label(
            RichText::new(label)
                .size(12.0)
                .color(if lit { color } else { p.muted }),
        );
    });
}

/// Small filled sparkline painted into `rect`.
pub fn sparkline(ui: &egui::Ui, rect: egui::Rect, data: &[f32], color: Color32) {
    if data.len() < 2 {
        return;
    }
    let max = data.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let step = rect.width() / (data.len() - 1) as f32;
    let points: Vec<egui::Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, v)| {
            egui::pos2(
                rect.left() + i as f32 * step,
                rect.bottom() - (v / max) * rect.height(),
            )
        })
        .collect();
    let painter = ui.painter_at(rect.expand(1.0));
    // Soft area under the line, as vertical strips.
    for w in points.windows(2) {
        let quad = vec![
            w[0],
            w[1],
            egui::pos2(w[1].x, rect.bottom()),
            egui::pos2(w[0].x, rect.bottom()),
        ];
        painter.add(egui::Shape::convex_polygon(
            quad,
            with_alpha(color, 0.12),
            Stroke::NONE,
        ));
    }
    painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
}

/// Status pill: dot + label.
pub fn status_pill(ui: &mut egui::Ui, p: &Palette, text: &str, color: Color32) -> Response {
    egui::Frame::new()
        .fill(p.tint(color))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin {
            left: 6,
            right: 10,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                dot(ui, color, false);
                ui.label(RichText::new(text).font(strong_font(12.0)).color(color));
            });
        })
        .response
}

/// Indicator chip for an RS-232 input line.
pub fn led(ui: &mut egui::Ui, p: &Palette, label: &str, on: bool) -> Response {
    let color = if on { p.ok } else { p.faint };
    egui::Frame::new()
        .fill(if on { p.tint(p.ok) } else { p.raised })
        .stroke(Stroke::new(1.0, if on { p.tint(p.ok) } else { p.border }))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.horizontal(|ui| {
                dot(ui, color, on);
                ui.label(RichText::new(label).font(strong_font(12.0)).color(if on {
                    p.text_strong
                } else {
                    p.muted
                }));
            });
        })
        .response
}

/// Toggle chip for an output line (RTS / DTR / Break / Echo).
pub fn toggle_chip(ui: &mut egui::Ui, p: &Palette, label: &str, on: &mut bool) -> Response {
    let color = if *on { p.accent } else { p.muted };
    let button = egui::Button::new(
        RichText::new(format!("{label}  {}", if *on { "ON" } else { "OFF" }))
            .font(strong_font(12.0))
            .color(if *on { p.accent } else { p.muted }),
    )
    .fill(if *on { p.tint(color) } else { p.raised })
    .stroke(Stroke::new(
        1.0,
        if *on { p.tint(p.accent) } else { p.border },
    ))
    .corner_radius(CornerRadius::same(6))
    .min_size(Vec2::new(0.0, 26.0));
    let mut response = ui.add(button);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
}

/// Segmented control. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    p: &Palette,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label) in options {
                    let selected = *value == *option;
                    let text = RichText::new(*label).size(12.5).color(if selected {
                        p.text_strong
                    } else {
                        p.muted
                    });
                    let text = if selected {
                        text.font(strong_font(12.5))
                    } else {
                        text
                    };
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            p.card
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(if selected {
                            Stroke::new(1.0, p.border_strong)
                        } else {
                            Stroke::NONE
                        })
                        .corner_radius(CornerRadius::same(5))
                        .min_size(Vec2::new(0.0, 22.0));
                    if ui.add(button).clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Filled primary action button.
pub fn primary(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.add(primary_button(p, text))
}

pub fn primary_button<'a>(p: &Palette, text: &str) -> egui::Button<'a> {
    egui::Button::new(
        RichText::new(text.to_string())
            .font(strong_font(13.5))
            .color(p.on_accent),
    )
    .fill(p.accent)
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(7))
    .min_size(Vec2::new(0.0, 30.0))
}

/// Outline / secondary button.
pub fn secondary(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(p.text))
            .fill(p.raised)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// Quiet text-only button for toolbars.
pub fn ghost(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.5).color(p.muted))
            .frame_when_inactive(false)
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(0.0, 24.0)),
    )
}

/// Destructive-styled button.
pub fn danger(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(p.error))
            .fill(p.tint(p.error))
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// Metric tile: caption, large value, sub-line and a coloured accent bar.
pub fn stat_tile(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    caption: &str,
    value: &str,
    sub: &str,
    color: Color32,
) {
    stat_tile_spark(ui, p, width, caption, value, sub, color, None);
}

#[allow(clippy::too_many_arguments)]
pub fn stat_tile_spark(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    caption: &str,
    value: &str,
    sub: &str,
    color: Color32,
    spark: Option<&[f32]>,
) {
    let response = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin {
            left: 16,
            right: 12,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width((width - 30.0).max(60.0));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(caption).size(11.5).color(p.muted));
                ui.label(
                    RichText::new(value)
                        .font(strong_font(19.0))
                        .color(p.text_strong),
                );
                ui.add(egui::Label::new(RichText::new(sub).size(11.0).color(p.faint)).truncate());
            });
        })
        .response;
    let r = response.rect;
    if let Some(data) = spark {
        let w = (r.width() * 0.42).min(140.0);
        let area = egui::Rect::from_min_max(
            egui::pos2(r.right() - w - 12.0, r.top() + 16.0),
            egui::pos2(r.right() - 12.0, r.bottom() - 14.0),
        );
        sparkline(ui, area, data, color);
    }
    ui.painter().rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(r.left() + 6.0, r.top() + 12.0),
            Vec2::new(3.0, r.height() - 24.0),
        ),
        CornerRadius::same(2),
        color,
    );
}

/// Form row label with a fixed width so inputs line up.
pub fn field_label(ui: &mut egui::Ui, p: &Palette, text: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.5),
        p.muted,
    );
}

/// Vertical divider used in toolbars.
pub fn vdivider(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, p.border),
    );
}

/// Draws the app mark: a rounded square with a small waveform.
pub fn logo(ui: &mut egui::Ui, p: &Palette, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((size * 0.28) as u8), p.accent);
    let inner = rect.shrink(size * 0.22);
    let w = inner.width();
    let h = inner.height();
    let pts = [
        (0.0, 0.5),
        (0.18, 0.5),
        (0.18, 0.15),
        (0.42, 0.15),
        (0.42, 0.85),
        (0.66, 0.85),
        (0.66, 0.5),
        (1.0, 0.5),
    ]
    .map(|(x, y)| inner.left_top() + Vec2::new(x * w, y * h));
    painter.add(egui::Shape::line(
        pts.to_vec(),
        Stroke::new(size * 0.09, p.on_accent),
    ));
    response
}

/// Thin outline around a rect.
pub fn outline(ui: &egui::Ui, rect: egui::Rect, color: Color32, radius: u8) {
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, color),
        StrokeKind::Inside,
    );
}

pub fn human_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        "0 B/s".into()
    } else if bytes_per_sec < 1024.0 {
        format!("{bytes_per_sec:.0} B/s")
    } else if bytes_per_sec < 1024.0 * 1024.0 {
        format!("{:.1} KiB/s", bytes_per_sec / 1024.0)
    } else {
        format!("{:.2} MiB/s", bytes_per_sec / (1024.0 * 1024.0))
    }
}

pub fn human_count(bytes: u64) -> String {
    serial_mcp_core::util::human_bytes(bytes as usize)
}

pub fn human_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}
