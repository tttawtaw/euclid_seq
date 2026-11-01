use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, Sense, Shape, Stroke, Ui,
};
use nih_plug_egui::{create_egui_editor, EguiState};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use crate::sequencer::{note_name, Scale, MAX_STEPS};
use crate::EuclidSeqParams;

const WIDTH: u32 = 720;
const HEIGHT: u32 = 300;

const BG: Color32 = Color32::from_rgb(0x17, 0x17, 0x19);
const PANEL: Color32 = Color32::from_rgb(0x22, 0x22, 0x26);
const CELL_OFF: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x34);
const CELL_OUT: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x20);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x1f);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x7a, 0x47, 0x18);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x88, 0x86);

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WIDTH, HEIGHT)
}

pub fn create(params: Arc<EuclidSeqParams>, playhead: Arc<AtomicI32>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        (),
        |ctx, _| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BG;
            visuals.selection.bg_fill = ACCENT_DIM;
            visuals.widgets.inactive.weak_bg_fill = PANEL;
            visuals.widgets.hovered.weak_bg_fill = CELL_OFF;
            ctx.set_visuals(visuals);
        },
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(BG).inner_margin(24.0))
                .show(ctx, |ui| {
                    header(ui, &params);
                    ui.add_space(14.0);
                    step_grid(ui, &params, setter, playhead.load(Ordering::Relaxed));
                    ui.add_space(18.0);
                    controls(ui, &params, setter);
                });
            // Keep the playhead animating while the transport runs.
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        },
    )
}

fn header(ui: &mut Ui, params: &EuclidSeqParams) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("EUCLID SEQ").color(TEXT).strong().size(16.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let summary = format!(
                "E({},{})  ·  {}  ·  {}",
                params.effective_pulses(),
                params.euclid_steps.value(),
                params.scale.value().name(),
                note_name(params.current_note() as i32),
            );
            ui.label(egui::RichText::new(summary).color(TEXT_DIM).monospace().size(12.0));
        });
    });
}

/// Two rows of 16 cells. Cells past the sequence length are dimmed; clicking a
/// cell sets the sequence length to end on it.
fn step_grid(ui: &mut Ui, params: &EuclidSeqParams, setter: &ParamSetter, playhead: i32) {
    let length = params.length.value() as usize;
    let e_steps = params.euclid_steps.value() as usize;
    let pattern = params.current_pattern();

    let gap = 6.0;
    let cols = 16;
    let cell = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let grid_h = cell * 2.0 + gap;
    let (grid_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), grid_h), Sense::hover());
    let painter = ui.painter_at(grid_rect.expand(4.0));

    for i in 0..MAX_STEPS {
        let (row, col) = (i / cols, i % cols);
        let min = grid_rect.min + vec2(col as f32 * (cell + gap), row as f32 * (cell + gap));
        let rect = Rect::from_min_size(min, vec2(cell, cell));
        let id = Id::new(("step", i));
        let resp = ui.interact(rect, id, Sense::click());

        let active = i < length;
        let hit = active && pattern[i];
        let is_playhead = active && playhead == i as i32;

        let fill = match (active, hit, is_playhead) {
            (false, _, _) => CELL_OUT,
            (true, true, true) => Color32::WHITE,
            (true, true, false) => ACCENT,
            (true, false, true) => Color32::from_rgb(0x55, 0x55, 0x5e),
            (true, false, false) => CELL_OFF,
        };
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        if resp.hovered() {
            painter.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(1.0_f32, TEXT_DIM),
                egui::StrokeKind::Inside,
            );
        }

        // Small marker at the start of each euclidean cycle.
        if active && e_steps > 0 && i % e_steps == 0 {
            let c = pos2(rect.center().x, rect.bottom() - 5.0);
            let dot = if hit { BG } else { ACCENT_DIM };
            painter.circle_filled(c, 2.0, dot);
        }

        if resp.clicked() {
            setter.begin_set_parameter(&params.length);
            setter.set_parameter(&params.length, i as i32 + 1);
            setter.end_set_parameter(&params.length);
        }
        resp.on_hover_text(format!("Step {} — click to end the sequence here", i + 1));
    }
}

fn controls(ui: &mut Ui, params: &EuclidSeqParams, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        knob(ui, setter, &params.length, "STEPS", false, None);
        ui.separator();
        knob(ui, setter, &params.euclid_steps, "E. STEPS", false, None);
        let pulses_text = params.effective_pulses().to_string();
        knob(ui, setter, &params.euclid_pulses, "PULSES", false, Some(pulses_text));
        ui.separator();
        knob(ui, setter, &params.pitch, "PITCH", true, None);
        knob(ui, setter, &params.root, "ROOT", false, None);

        ui.vertical(|ui| {
            ui.add_space(14.0);
            ui.label(egui::RichText::new("SCALE").color(TEXT_DIM).size(11.0));
            let current = params.scale.value();
            egui::ComboBox::from_id_salt("scale")
                .width(140.0)
                .selected_text(egui::RichText::new(current.name()).color(TEXT))
                .show_ui(ui, |ui| {
                    for scale in Scale::ALL {
                        if ui.selectable_label(current == scale, scale.name()).clicked() {
                            setter.begin_set_parameter(&params.scale);
                            setter.set_parameter(&params.scale, scale);
                            setter.end_set_parameter(&params.scale);
                        }
                    }
                });
        });
    });
}

/// Rotary knob: drag vertically (Shift for fine), scroll for single steps,
/// double-click to reset.
fn knob(
    ui: &mut Ui,
    setter: &ParamSetter,
    param: &IntParam,
    label: &str,
    bipolar: bool,
    value_text: Option<String>,
) {
    let size = vec2(60.0, 92.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let drag_id = resp.id.with("drag_value");

    if resp.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, param.default_plain_value());
        setter.end_set_parameter(param);
    } else if resp.drag_started() {
        setter.begin_set_parameter(param);
        let start = param.unmodulated_normalized_value();
        ui.memory_mut(|m| m.data.insert_temp(drag_id, start));
    } else if resp.dragged() {
        // Accumulate in an unquantized value; the int param snaps on its own.
        let fine = ui.input(|i| i.modifiers.shift);
        let speed = if fine { 0.0015 } else { 0.006 };
        let mut v: f32 = ui.memory(|m| m.data.get_temp(drag_id)).unwrap_or(0.0);
        v = (v - resp.drag_delta().y * speed).clamp(0.0, 1.0);
        ui.memory_mut(|m| m.data.insert_temp(drag_id, v));
        setter.set_parameter_normalized(param, v);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let next = param.value() + scroll.signum() as i32;
            setter.begin_set_parameter(param);
            setter.set_parameter(param, next);
            setter.end_set_parameter(param);
        }
    }

    let painter = ui.painter_at(rect);
    let center = pos2(rect.center().x, rect.top() + 34.0);
    let radius = 24.0;
    let (a0, a1) = (0.75 * PI, 2.25 * PI); // 270° sweep, gap at the bottom
    let norm = param.unmodulated_normalized_value();
    let angle = a0 + (a1 - a0) * norm;

    let arc = |from: f32, to: f32, color: Color32, width: f32| {
        let n = 40;
        let pts: Vec<_> = (0..=n)
            .map(|k| {
                let t = from + (to - from) * k as f32 / n as f32;
                center + vec2(t.cos(), t.sin()) * radius
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(width, color)));
    };

    painter.circle_filled(center, radius - 5.0, PANEL);
    arc(a0, a1, CELL_OFF, 4.0);
    let from = if bipolar { 1.5 * PI } else { a0 };
    if (angle - from).abs() > 0.01 {
        arc(from.min(angle), from.max(angle), ACCENT, 4.0);
    }
    let tip = center + vec2(angle.cos(), angle.sin()) * (radius - 9.0);
    painter.line_segment([center, tip], Stroke::new(2.0_f32, TEXT));

    let value = value_text.unwrap_or_else(|| param.to_string());
    painter.text(
        pos2(center.x, rect.top() + 70.0),
        Align2::CENTER_CENTER,
        value,
        FontId::monospace(13.0),
        if resp.hovered() || resp.dragged() { ACCENT } else { TEXT },
    );
    painter.text(
        pos2(center.x, rect.top() + 86.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(10.5),
        TEXT_DIM,
    );

    resp.on_hover_text("Drag (Shift = fine) · scroll · double-click to reset");
}
