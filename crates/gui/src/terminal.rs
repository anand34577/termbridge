//! Terminal renderer for the GUI. The line model lives in `serial_mcp_core::view`.

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Vec2};
use serial_mcp_core::types::Direction;
pub use serial_mcp_core::view::{dir_tag, LineFilter, TermLine, TerminalBuffer};

use crate::theme::Palette;
use crate::widgets::strong_font;

/// Render the terminal. `visible` holds indices of lines that pass the filter
/// (computed by the caller once per frame).
pub fn show(
    ui: &mut egui::Ui,
    buffer: &TerminalBuffer,
    visible: &[usize],
    palette: &Palette,
    follow: bool,
    wrap_lines: bool,
) {
    let p = palette;
    egui::Frame::new()
        .fill(p.term_bg)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin {
            left: 4,
            right: 4,
            top: 6,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    let text = if buffer.lines.is_empty() {
                        "Waiting for data. Anything received or sent appears here."
                    } else {
                        "No lines match the current filter."
                    };
                    ui.label(RichText::new(text).size(12.5).color(p.faint));
                });
                return;
            }
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
            let scroll = egui::ScrollArea::both()
                .id_salt("terminal_scroll")
                .stick_to_bottom(follow)
                .auto_shrink([false, false]);
            if wrap_lines {
                // Wrapped rows have variable height, so render them all.
                scroll.show(ui, |ui| {
                    for &index in visible {
                        row(ui, p, &buffer.lines[index], buffer.show_ts, true);
                    }
                });
            } else {
                scroll.show_rows(ui, row_height, visible.len(), |ui, range| {
                    for &index in &visible[range] {
                        row(ui, p, &buffer.lines[index], buffer.show_ts, false);
                    }
                });
            }
        });
}

fn row(ui: &mut egui::Ui, p: &Palette, line: &TermLine, show_ts: bool, wrap: bool) {
    let color = match line.dir {
        Direction::Rx => p.rx,
        Direction::Tx => p.tx,
        Direction::Info => p.muted,
        Direction::Error => p.error,
        Direction::Nvt => p.nvt,
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(4.0);
        if show_ts {
            ui.label(
                RichText::new(&line.ts)
                    .monospace()
                    .size(12.0)
                    .color(p.faint),
            );
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 16.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.tint(color));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            dir_tag(line.dir),
            strong_font(9.5),
            color,
        );
        let label = egui::Label::new(RichText::new(&line.text).monospace().color(
            if matches!(line.dir, Direction::Rx | Direction::Tx) {
                p.text
            } else {
                color
            },
        ))
        .selectable(true);
        if wrap {
            ui.add(label.wrap());
        } else {
            ui.add(label.extend());
        }
    });
}
