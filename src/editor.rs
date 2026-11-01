use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui,
};
use nih_plug_egui::{create_egui_editor, EguiState};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use crate::sequencer::{note_name, toggle_override, RampMode, RampRange, Scale, MAX_STEPS};
use crate::{EuclidSeqParams, TrackParams, NUM_TRACKS};

const WIDTH: u32 = 910;
const HEIGHT: u32 = 470;

const BG: Color32 = Color32::from_rgb(0x17, 0x17, 0x19);
const PANEL: Color32 = Color32::from_rgb(0x22, 0x22, 0x26);
const CELL_OFF: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x34);
const CELL_OUT: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x20);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x1f);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x7a, 0x47, 0x18);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x88, 0x86);

const KNOB_W: f32 = 60.0;
const KNOB_H: f32 = 92.0;
/// Vertical centre of a knob's dial, relative to the top of its rect.
const DIAL_Y: f32 = 34.0;

type Playheads = Arc<[AtomicI32; NUM_TRACKS]>;

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WIDTH, HEIGHT)
}

/// GUI-only state: which track page is showing.
#[derive(Default)]
struct EditorState {
    page: usize,
}

pub fn create(params: Arc<EuclidSeqParams>, playheads: Playheads) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        EditorState::default(),
        |ctx, _| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BG;
            visuals.selection.bg_fill = ACCENT_DIM;
            visuals.widgets.inactive.weak_bg_fill = PANEL;
            visuals.widgets.hovered.weak_bg_fill = CELL_OFF;
            ctx.set_visuals(visuals);
        },
        move |ctx, setter, state| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(BG).inner_margin(24.0))
                .show(ctx, |ui| {
                    top_bar(ui, &params, setter, &playheads, &mut state.page);
                    let track = &params.tracks[state.page];
                    let playhead = playheads[state.page].load(Ordering::Relaxed);
                    ui.add_space(14.0);
                    step_grid(ui, track, setter, playhead);
                    ui.add_space(16.0);
                    controls(ui, track, setter, state.page);
                });
            // Keep the playhead animating while the transport runs.
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        },
    )
}

fn set<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Is this track sounding a note right now? Drives the tab activity light.
fn is_firing(track: &TrackParams, playhead: i32) -> bool {
    track.on.value()
        && playhead >= 0
        && (playhead as usize) < MAX_STEPS
        && track.current_pattern()[playhead as usize]
}

fn top_bar(
    ui: &mut Ui,
    params: &EuclidSeqParams,
    setter: &ParamSetter,
    playheads: &Playheads,
    page: &mut usize,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("EUCLID SEQ").color(TEXT).strong().size(16.0));
        ui.add_space(18.0);

        // Track tabs: number, dim when the track is off, light when it fires.
        for (t, track) in params.tracks.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 26.0), Sense::click());
            let p = ui.painter();
            let selected = *page == t;
            let fill = if selected { CELL_OFF } else if resp.hovered() { PANEL } else { BG };
            p.rect_filled(rect, CornerRadius::same(5), fill);
            if selected {
                p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
            }
            let text_color = match (selected, track.on.value()) {
                (true, _) => TEXT,
                (false, true) => TEXT_DIM,
                (false, false) => CELL_OFF,
            };
            p.text(
                rect.center() - vec2(5.0, 0.0),
                Align2::CENTER_CENTER,
                format!("{}", t + 1),
                FontId::monospace(13.0),
                text_color,
            );
            let dot = if is_firing(track, playheads[t].load(Ordering::Relaxed)) {
                ACCENT
            } else if track.on.value() {
                ACCENT_DIM
            } else {
                CELL_OFF
            };
            p.circle_filled(pos2(rect.right() - 12.0, rect.center().y), 3.5, dot);
            if resp.clicked() {
                *page = t;
            }
            resp.on_hover_text(format!("Track {}", t + 1));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let track = &params.tracks[*page];
            value_box(ui, setter, &track.channel, "CH");
            ui.add_space(6.0);
            pill_toggle(ui, setter, &track.on, "ON", "OFF");
            ui.add_space(12.0);
            if text_button(ui, "RESET", "Reset this track's settings to their defaults\n(keeps ON and the MIDI channel)") {
                reset_track(setter, track);
            }
            if text_button(ui, "CLEAR", "Remove every note from this track\n(Pulses to 0, manual edits cleared)") {
                clear_track(setter, track);
            }
            ui.add_space(12.0);
            let rot = match track.rotate.value() {
                0 => String::new(),
                r => format!(" {r:+}"),
            };
            let summary = format!(
                "E({},{}){}  ·  {}  ·  {}",
                track.effective_pulses(),
                track.effective_euclid_steps(),
                rot,
                track.scale.value().name(),
                note_name(track.root.value()),
            );
            ui.label(egui::RichText::new(summary).color(TEXT_DIM).monospace().size(12.0));
        });
    });
}

/// Two rows of 16 cells. Cells past the sequence length are dimmed; clicking a
/// cell sets the sequence length to end on it. With a ramp active, each step
/// shows its pitch as a short line (higher line = higher note).
fn step_grid(ui: &mut Ui, track: &TrackParams, setter: &ParamSetter, playhead: i32) {
    let length = track.length.value() as usize;
    let e_steps = track.effective_euclid_steps();
    let pattern = track.current_pattern();
    let added = track.added.load(Ordering::Relaxed);
    let removed = track.removed.load(Ordering::Relaxed);
    let ramp = track.ramp();
    let (lo, hi) = ramp.bounds(track.scale.value());
    let on = track.on.value();

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
        let resp = ui.interact(rect, Id::new(("step", i)), Sense::click());
        let bit = 1u32 << i;

        let active = i < length;
        let hit = active && pattern[i];
        let is_playhead = active && playhead == i as i32;

        let mut fill = match (active, hit, is_playhead) {
            (false, _, _) => CELL_OUT,
            (true, true, true) => Color32::WHITE,
            (true, true, false) => ACCENT,
            (true, false, true) => Color32::from_rgb(0x55, 0x55, 0x5e),
            (true, false, false) => CELL_OFF,
        };
        if !on && hit {
            fill = ACCENT_DIM; // muted track: show the pattern, but quietly
        }
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        if resp.hovered() {
            painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, TEXT_DIM), StrokeKind::Inside);
        }

        // Pitch contour of the ramp.
        if active && ramp.mode != RampMode::Off && hi > lo {
            let deg = ramp.degree(i, length, track.scale.value());
            let t = (deg - lo) as f32 / (hi - lo) as f32;
            let y = rect.bottom() - 8.0 - t * (cell - 16.0);
            let w = cell * 0.3;
            let color = if hit { BG } else { ACCENT_DIM };
            painter.line_segment(
                [pos2(rect.center().x - w, y), pos2(rect.center().x + w, y)],
                Stroke::new(2.0_f32, color),
            );
        }

        // Velocity: a bar along the bottom of each hit, as wide as it is loud.
        if hit {
            let vel = track.step_velocity(i, &pattern) as f32 / 127.0;
            let w = (cell - 10.0) * vel;
            let bar = Rect::from_min_size(pos2(rect.left() + 5.0, rect.bottom() - 6.0), vec2(w.max(1.0), 2.5));
            painter.rect_filled(bar, CornerRadius::ZERO, BG);
        }

        // Small marker at the start of each euclidean cycle.
        if active && e_steps > 0 && i % e_steps == 0 {
            let c = pos2(rect.left() + 6.0, rect.top() + 6.0);
            painter.circle_filled(c, 2.0, if hit { BG } else { ACCENT_DIM });
        }

        // Manual edits: a corner notch on added notes, an outline on removed ones.
        if active && added & bit != 0 {
            let tr = rect.right_top();
            painter.add(Shape::convex_polygon(
                vec![tr + vec2(-11.0, 0.0), tr, tr + vec2(0.0, 11.0)],
                if hit { BG } else { ACCENT_DIM },
                Stroke::NONE,
            ));
        }
        if active && !hit && removed & bit != 0 {
            painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(4), Stroke::new(1.5_f32, ACCENT_DIM), StrokeKind::Inside);
        }

        // Cmd+click (or Ctrl+click / right-click, which hosts never swallow)
        // toggles the step; a plain click sets the sequence length.
        let modifiers = ui.input(|inp| inp.modifiers);
        let edit_click = (resp.clicked() && (modifiers.mac_cmd || modifiers.command || modifiers.ctrl))
            || resp.secondary_clicked();
        if edit_click && active {
            let (a, r) = toggle_override(added, removed, i, hit);
            track.added.store(a, Ordering::Relaxed);
            track.removed.store(r, Ordering::Relaxed);
        } else if resp.clicked() {
            set(setter, &track.length, i as i32 + 1);
        }

        let hover = if active {
            let note = note_name(track.step_note(i) as i32);
            let what = if hit {
                format!("{} · velocity {}", note, track.step_velocity(i, &pattern))
            } else {
                format!("{} (rest)", note)
            };
            let manual = if added & bit != 0 {
                " · added by hand"
            } else if removed & bit != 0 {
                " · removed by hand"
            } else {
                ""
            };
            format!(
                "Step {} · {}{}\nClick: end the sequence here\nCmd+click: {} a note",
                i + 1,
                what,
                manual,
                if hit { "remove" } else { "add" }
            )
        } else {
            format!("Step {}\nClick: end the sequence here", i + 1)
        };
        resp.on_hover_text(hover);
    }
}

fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).color(TEXT_DIM).size(10.5).strong());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            add(ui);
        });
    });
}

fn controls(ui: &mut Ui, track: &TrackParams, setter: &ParamSetter, page: usize) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 34.0;

        group(ui, "RHYTHM", |ui| {
            let linked = track.link.value();
            knob(ui, setter, &track.length, "STEPS", KnobStyle::default());
            link_button(ui, setter, track);
            // While linked, the E. Steps knob *is* the Steps knob, so both
            // always move together, including under host automation.
            if linked {
                knob(ui, setter, &track.length, "E. STEPS", KnobStyle::default());
            } else {
                knob(ui, setter, &track.euclid_steps, "E. STEPS", KnobStyle::default());
            }
            let pulses = KnobStyle { text: Some(track.effective_pulses().to_string()), ..Default::default() };
            knob(ui, setter, &track.euclid_pulses, "PULSES", pulses);
            knob(ui, setter, &track.rotate, "ROTATE", KnobStyle { bipolar: true, ..Default::default() });
        });

        group(ui, "PITCH", |ui| {
            knob(ui, setter, &track.pitch, "PITCH", KnobStyle { bipolar: true, ..Default::default() });
            knob(ui, setter, &track.root, "ROOT", KnobStyle::default());
            ui.vertical(|ui| {
                ui.add_space(DIAL_Y - 20.0);
                let current = track.scale.value();
                egui::ComboBox::from_id_salt(("scale", page))
                    .width(128.0)
                    .selected_text(egui::RichText::new(current.name()).color(TEXT))
                    .show_ui(ui, |ui| {
                        for scale in Scale::ALL {
                            if ui.selectable_label(current == scale, scale.name()).clicked() {
                                set(setter, &track.scale, scale);
                            }
                        }
                    });
                ui.add_space(6.0);
                ui.label(egui::RichText::new("SCALE").color(TEXT_DIM).size(10.5));
            });
        });
    });

    ui.add_space(12.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 34.0;

        group(ui, "PITCH RAMP", |ui| {
            let mode = KnobStyle { stepped: Some(4), ..Default::default() };
            knob(ui, setter, &track.ramp_mode, "MODE", mode);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                let ranges: Vec<_> = RampRange::ALL
                    .iter()
                    .map(|&r| (r, format!("{}", r.octaves())))
                    .collect();
                segmented(ui, setter, &track.ramp_range, &ranges, 30.0);
                ui.label(egui::RichText::new("RANGE · OCT").color(TEXT_DIM).size(10.5));
                segmented(ui, setter, &track.ramp_bipolar, &[(false, "+".into()), (true, "±".into())], 45.0);
                ui.label(egui::RichText::new("POLARITY").color(TEXT_DIM).size(10.5));
            });
            let enabled = track.ramp_mode.value() == RampMode::Random;
            ui.vertical(|ui| {
                ui.add_space(DIAL_Y - 16.0);
                dice_button(ui, setter, &track.ramp_seed, enabled);
            });
        });

        group(ui, "VELOCITY", |ui| {
            knob(ui, setter, &track.velocity, "VELOCITY", KnobStyle::default());
            let mode = KnobStyle { stepped: Some(3), ..Default::default() };
            knob(ui, setter, &track.vel_mode, "MODE", mode);
            knob(ui, setter, &track.vel_low, "LOW", KnobStyle::default());
            knob(ui, setter, &track.vel_high, "HIGH", KnobStyle::default());
            // Show the length actually used: capped at the notes in the pattern.
            let used = (track.vel_length.value() as usize).min(track.note_count()).max(1);
            let length = KnobStyle { text: Some(used.to_string()), ..Default::default() };
            knob(ui, setter, &track.vel_length, "LENGTH", length);
        });
    });
}

/// Empties the track: no euclidean pulses, no manually added notes.
fn clear_track(setter: &ParamSetter, track: &TrackParams) {
    set(setter, &track.euclid_pulses, 0);
    track.added.store(0, Ordering::Relaxed);
    track.removed.store(0, Ordering::Relaxed);
}

/// Every setting of the track back to its default, except whether the track is
/// on and its MIDI channel (resetting those would silence or reroute it).
fn reset_track(setter: &ParamSetter, track: &TrackParams) {
    for (id, ptr, _) in track.param_map() {
        if id == "on" || id == "channel" {
            continue;
        }
        // SAFETY: the pointers come from this track's own param map and stay
        // valid for as long as the plugin's parameters live.
        unsafe {
            let ctx = setter.raw_context;
            ctx.raw_begin_set_parameter(ptr);
            ctx.raw_set_parameter_normalized(ptr, ptr.default_normalized_value());
            ctx.raw_end_set_parameter(ptr);
        }
    }
    track.added.store(0, Ordering::Relaxed);
    track.removed.store(0, Ordering::Relaxed);
}

/// Small text button; returns true when clicked.
fn text_button(ui: &mut Ui, label: &str, hint: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, 24.0), Sense::click());
    let p = ui.painter();
    let (fill, color) = if resp.is_pointer_button_down_on() {
        (ACCENT, BG)
    } else if resp.hovered() {
        (CELL_OFF, ACCENT)
    } else {
        (PANEL, TEXT)
    };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(11.5), color);
    let clicked = resp.clicked();
    resp.on_hover_text(hint);
    clicked
}

#[derive(Default)]
struct KnobStyle {
    /// Fill the arc from the centre (for +/- values).
    bipolar: bool,
    /// Draw this many detents and show the value without a fill arc.
    stepped: Option<usize>,
    /// Override the value text.
    text: Option<String>,
}

/// Shared drag/scroll/double-click behaviour for knobs and value boxes.
fn param_interaction<P: Param>(ui: &Ui, resp: &Response, setter: &ParamSetter, param: &P, speed: f32) {
    let drag_id = resp.id.with("drag_value");
    if resp.double_clicked() {
        set(setter, param, param.default_plain_value());
    } else if resp.drag_started() {
        setter.begin_set_parameter(param);
        let start = param.unmodulated_normalized_value();
        ui.memory_mut(|m| m.data.insert_temp(drag_id, start));
    } else if resp.dragged() {
        // Accumulate in an unquantized value; stepped params snap on their own.
        let fine = ui.input(|i| i.modifiers.shift);
        let speed = if fine { speed / 4.0 } else { speed };
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
            let current = param.unmodulated_plain_value();
            let next = if scroll > 0.0 {
                param.next_step(current, false)
            } else {
                param.previous_step(current, false)
            };
            set(setter, param, next);
        }
    }
}

/// Rotary knob: drag vertically (Shift for fine), scroll for single steps,
/// double-click to reset.
fn knob<P: Param>(ui: &mut Ui, setter: &ParamSetter, param: &P, label: &str, style: KnobStyle) {
    let (rect, resp) = ui.allocate_exact_size(vec2(KNOB_W, KNOB_H), Sense::click_and_drag());
    let speed = if style.stepped.is_some() { 0.012 } else { 0.006 };
    param_interaction(ui, &resp, setter, param, speed);

    let painter = ui.painter_at(rect);
    let center = pos2(rect.center().x, rect.top() + DIAL_Y);
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
    match style.stepped {
        Some(n) if n > 1 => {
            // Detents, with the selected one lit.
            for k in 0..n {
                let a = a0 + (a1 - a0) * k as f32 / (n - 1) as f32;
                let selected = (angle - a).abs() < 0.01;
                let c = center + vec2(a.cos(), a.sin()) * radius;
                painter.circle_filled(c, if selected { 4.0 } else { 2.5 }, if selected { ACCENT } else { TEXT_DIM });
            }
        }
        _ => {
            let from = if style.bipolar { 1.5 * PI } else { a0 };
            if (angle - from).abs() > 0.01 {
                arc(from.min(angle), from.max(angle), ACCENT, 4.0);
            }
        }
    }
    let tip = center + vec2(angle.cos(), angle.sin()) * (radius - 9.0);
    painter.line_segment([center, tip], Stroke::new(2.0_f32, TEXT));

    let value = style.text.unwrap_or_else(|| param.to_string());
    painter.text(
        pos2(center.x, rect.top() + 70.0),
        Align2::CENTER_CENTER,
        value,
        FontId::monospace(13.0),
        if resp.hovered() || resp.dragged() { ACCENT } else { TEXT },
    );
    painter.text(pos2(center.x, rect.top() + 86.0), Align2::CENTER_CENTER, label, FontId::proportional(10.5), TEXT_DIM);

    resp.on_hover_text("Drag (Shift = fine) · scroll · double-click to reset");
}

/// Chain-link toggle drawn at dial height between Steps and E. Steps.
fn link_button(ui: &mut Ui, setter: &ParamSetter, track: &TrackParams) {
    let (rect, _) = ui.allocate_exact_size(vec2(24.0, KNOB_H), Sense::hover());
    let hit = Rect::from_center_size(pos2(rect.center().x, rect.top() + DIAL_Y), vec2(24.0, 30.0));
    let resp = ui.interact(hit, ui.id().with("link"), Sense::click());
    let linked = track.link.value();

    if resp.clicked() {
        if linked {
            // Unlinking: E. Steps keeps the value it was showing.
            set(setter, &track.euclid_steps, track.length.value());
        }
        set(setter, &track.link, !linked);
    }

    let p = ui.painter();
    let color = if linked { ACCENT } else if resp.hovered() { TEXT_DIM } else { CELL_OFF };
    let c = hit.center();
    // Two chain links, slightly apart when unlinked.
    let spread = if linked { 4.5 } else { 7.0 };
    for dy in [-spread, spread] {
        let r = Rect::from_center_size(c + vec2(0.0, dy), vec2(9.0, 12.0));
        p.rect_stroke(r, CornerRadius::same(4), Stroke::new(2.0_f32, color), StrokeKind::Middle);
    }
    if linked {
        p.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], Stroke::new(2.0_f32, color));
    }
    resp.on_hover_text(if linked {
        "Linked: Steps and E. Steps move together. Click to unlink."
    } else {
        "Click to link Steps and E. Steps"
    });
}

/// A row of mutually exclusive buttons bound to one parameter.
fn segmented<P: Param>(
    ui: &mut Ui,
    setter: &ParamSetter,
    param: &P,
    options: &[(P::Plain, String)],
    width: f32,
) where
    P::Plain: PartialEq + Copy,
{
    let current = param.unmodulated_plain_value();
    let (rect, _) = ui.allocate_exact_size(vec2(width * options.len() as f32, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), PANEL);
    for (k, (value, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(k as f32 * width, 0.0), vec2(width, 24.0)).shrink(2.0);
        let resp = ui.interact(r, ui.id().with((param.name(), k)), Sense::click());
        let selected = current == *value;
        let p = ui.painter();
        if selected {
            p.rect_filled(r, CornerRadius::same(4), ACCENT);
        } else if resp.hovered() {
            p.rect_filled(r, CornerRadius::same(4), CELL_OFF);
        }
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(13.0),
            if selected { BG } else { TEXT },
        );
        if resp.clicked() {
            set(setter, param, *value);
        }
    }
}

fn pill_toggle(ui: &mut Ui, setter: &ParamSetter, param: &BoolParam, on: &str, off: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 24.0), Sense::click());
    let value = param.value();
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), if value { ACCENT } else { PANEL });
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        if value { on } else { off },
        FontId::monospace(12.0),
        if value { BG } else { TEXT_DIM },
    );
    if resp.clicked() {
        set(setter, param, !value);
    }
    resp.on_hover_text("Turn this track on or off");
}

/// Compact draggable number (used for the MIDI channel).
fn value_box(ui: &mut Ui, setter: &ParamSetter, param: &IntParam, label: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(62.0, 24.0), Sense::click_and_drag());
    param_interaction(ui, &resp, setter, param, 0.01);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(5), PANEL);
    let color = if resp.hovered() || resp.dragged() { ACCENT } else { TEXT };
    p.text(rect.left_center() + vec2(9.0, 0.0), Align2::LEFT_CENTER, label, FontId::monospace(11.0), TEXT_DIM);
    p.text(rect.right_center() - vec2(9.0, 0.0), Align2::RIGHT_CENTER, param.to_string(), FontId::monospace(13.0), color);
    resp.on_hover_text("MIDI channel for this track · drag or scroll");
}

/// Re-rolls the Random ramp. Each seed gives a completely different melody.
fn dice_button(ui: &mut Ui, setter: &ParamSetter, seed: &IntParam, enabled: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let p = ui.painter();
    let color = if !enabled { CELL_OFF } else if resp.hovered() { ACCENT } else { TEXT };
    p.rect_stroke(rect.shrink(3.0), CornerRadius::same(6), Stroke::new(2.0_f32, color), StrokeKind::Middle);
    let c = rect.center();
    for (dx, dy) in [(-6.0, -6.0), (6.0, -6.0), (0.0, 0.0), (-6.0, 6.0), (6.0, 6.0)] {
        p.circle_filled(c + vec2(dx, dy), 2.0, color);
    }
    if enabled && resp.clicked() {
        set(setter, seed, (seed.value() + 1) % 10_000);
    }
    resp.on_hover_text(if enabled {
        "Re-roll the random melody"
    } else {
        "Re-roll (Random mode only)"
    });
}
