use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui,
};
use nih_plug_egui::{create_egui_editor, EguiState};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::sequencer::{
    cell_step_in_pass, note_name, play_pos, splitmix64, toggle_override, Direction, GateUnit, Rate, RampMode,
    RampRange, Scale, MAX_STEPS,
};
use crate::presets;
use crate::{solo_after_click, EuclidSeqParams, Mix, TrackParams, NUM_TRACKS, STOPPED};

const WIDTH: u32 = 1020;
const HEIGHT: u32 = 700;

const BG: Color32 = Color32::from_rgb(0x17, 0x17, 0x19);
const PANEL: Color32 = Color32::from_rgb(0x22, 0x22, 0x26);
const CELL_OFF: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x34);
const CELL_OUT: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x20);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x1f);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x7a, 0x47, 0x18);
const TEXT: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x88, 0x86);
/// Quieter than TEXT_DIM: for the version number.
const VERSION_COLOR: Color32 = Color32::from_rgb(0x55, 0x54, 0x53);
const MUTE_COLOR: Color32 = Color32::from_rgb(0xd9, 0x4f, 0x4f);
const SOLO_COLOR: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
/// Background of the control panels, a shade above the window background.
const HALF_MARK: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x50);
const SECTION: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x21);

const KNOB_W: f32 = 60.0;
const KNOB_H: f32 = 92.0;
/// Vertical centre of a knob's dial, relative to the top of its rect.
const DIAL_Y: f32 = 34.0;

type Playheads = Arc<[AtomicI64; NUM_TRACKS]>;

/// (grid cell under the playhead, pass number) for a track, following its
/// playback direction, or (-1, 0) when stopped.
fn split_playhead(track: &TrackParams, global: i64) -> (i32, i64) {
    if global == STOPPED {
        return (-1, 0);
    }
    let pos = play_pos(global, track.length.value() as usize, track.direction.value());
    (pos.position as i32, pos.pass)
}

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WIDTH, HEIGHT)
}

/// GUI-only state: which track page is showing, and the track clipboard.
/// It lives as long as the plugin instance, so a copy survives closing and
/// reopening the window.
#[derive(Default)]
struct EditorState {
    page: usize,
    clipboard: Option<TrackSnapshot>,
    /// Text in the preset name box.
    preset_name: String,
    /// The preset last loaded or saved, shown in the preset list.
    current_preset: Option<String>,
    /// SAVE over an existing preset is armed until this time (second click confirms).
    overwrite_until: f64,
    /// A short message under the preset bar: (text, is_error, shown until).
    status: Option<(String, bool, f64)>,
}

/// Everything about a track except its ON switch and MIDI channel.
struct TrackSnapshot {
    from: usize,
    values: Vec<(String, f32)>,
    added: u32,
    removed: u32,
    offsets: [i8; MAX_STEPS],
}

fn copy_track(track: &TrackParams, from: usize) -> TrackSnapshot {
    let values = track
        .param_map()
        .into_iter()
        // SAFETY: pointers from the track's own param map, valid while it lives.
        .map(|(id, ptr, _)| (id, unsafe { ptr.unmodulated_normalized_value() }))
        .collect();
    TrackSnapshot {
        from,
        values,
        added: track.added.load(Ordering::Relaxed),
        removed: track.removed.load(Ordering::Relaxed),
        offsets: track.pitch_offsets.load(),
    }
}

/// Paste a copied track over `track`, keeping its ON switch and MIDI channel so
/// it keeps playing its own instrument.
fn paste_track(setter: &ParamSetter, track: &TrackParams, snap: &TrackSnapshot) {
    for (id, ptr, _) in track.param_map() {
        if matches!(id.as_str(), "on" | "channel" | "mute" | "solo") {
            continue;
        }
        if let Some((_, v)) = snap.values.iter().find(|(sid, _)| *sid == id) {
            // SAFETY: as above.
            unsafe {
                let ctx = setter.raw_context;
                ctx.raw_begin_set_parameter(ptr);
                ctx.raw_set_parameter_normalized(ptr, *v);
                ctx.raw_end_set_parameter(ptr);
            }
        }
    }
    track.added.store(snap.added, Ordering::Relaxed);
    track.removed.store(snap.removed, Ordering::Relaxed);
    track.pitch_offsets.store(snap.offsets);
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
                    top_bar(ui, &params, setter, &playheads, state);
                    let track = &params.tracks[state.page];
                    let (playhead, loop_idx) = split_playhead(track, playheads[state.page].load(Ordering::Relaxed));
                    ui.add_space(14.0);
                    step_grid(ui, track, setter, playhead, loop_idx, params.mix());
                    ui.add_space(16.0);
                    controls(ui, &params, track, setter, state.page);
                    ui.add_space(12.0);
                    preset_bar(ui, setter, state);

                    // Version, discreetly in the bottom-left corner (in the
                    // window margin, so it never overlaps the controls).
                    let window = ctx.screen_rect();
                    ui.painter().with_clip_rect(window).text(
                        window.left_bottom() + vec2(24.0, -12.0),
                        Align2::LEFT_CENTER,
                        concat!("v", env!("CARGO_PKG_VERSION")),
                        FontId::monospace(10.0),
                        VERSION_COLOR,
                    );
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
fn is_firing(track: &TrackParams, playhead: i32, mix: Mix) -> bool {
    track.audible(mix)
        && playhead >= 0
        && (playhead as usize) < MAX_STEPS
        && track.current_pattern()[playhead as usize]
}

fn top_bar(ui: &mut Ui, params: &EuclidSeqParams, setter: &ParamSetter, playheads: &Playheads, state: &mut EditorState) {
    let page = &mut state.page;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("EUCLID SEQ").color(TEXT).strong().size(16.0));
        ui.add_space(18.0);

        // Track tabs: number, dim when the track can't be heard, light when
        // it fires; each followed by its Mute and Solo buttons.
        let mix = params.mix();
        ui.spacing_mut().item_spacing.x = 3.0;
        for (t, track) in params.tracks.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 26.0), Sense::click());
            let p = ui.painter();
            let selected = *page == t;
            let fill = if selected { CELL_OFF } else if resp.hovered() { PANEL } else { BG };
            p.rect_filled(rect, CornerRadius::same(5), fill);
            if selected {
                p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
            }
            let text_color = match (selected, track.audible(mix)) {
                (true, _) => TEXT,
                (false, true) => TEXT_DIM,
                (false, false) => CELL_OFF,
            };
            p.text(
                rect.center() - vec2(4.0, 0.0),
                Align2::CENTER_CENTER,
                format!("{}", t + 1),
                FontId::monospace(13.0),
                text_color,
            );
            let dot = if is_firing(track, split_playhead(track, playheads[t].load(Ordering::Relaxed)).0, mix) {
                ACCENT
            } else if track.audible(mix) {
                ACCENT_DIM
            } else {
                CELL_OFF
            };
            p.circle_filled(pos2(rect.right() - 8.0, rect.center().y), 3.0, dot);
            if resp.clicked() {
                *page = t;
                // Going to a track means you want to hear it.
                if !track.on.value() {
                    set(setter, &track.on, true);
                }
            }
            resp.on_hover_text(format!("Track {} (opening it turns it on)", t + 1));
            if flag_button(ui, &track.mute, "M", MUTE_COLOR, &format!("Mute track {}", t + 1)) {
                set(setter, &track.mute, !track.mute.value());
            }
            let solo_hint = format!(
                "Solo track {}\n(when any track is soloed, only soloed tracks play;\nunsoloing one of several soloed tracks clears all solos)",
                t + 1
            );
            if flag_button(ui, &track.solo, "S", SOLO_COLOR, &solo_hint) {
                let solos = std::array::from_fn(|i| params.tracks[i].solo.value());
                for (track, solo) in params.tracks.iter().zip(solo_after_click(solos, t)) {
                    if track.solo.value() != solo {
                        set(setter, &track.solo, solo);
                    }
                }
            }
            ui.add_space(7.0);
        }
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(12.0);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let track = &params.tracks[*page];
            value_box(ui, setter, &track.channel, "CH");
            ui.add_space(6.0);
            pill_toggle(ui, setter, &track.on, "ON", "OFF", "Turn this track on or off");
            ui.add_space(12.0);
            if text_button(ui, "RESET", "Reset this track's settings to their defaults\n(keeps ON and the MIDI channel)") {
                reset_track(setter, track);
            }
            if text_button(
                ui,
                "RANDOM",
                "Randomise this track's Steps, E. Steps, Pulses, Pitch,\nramp Mode, ramp Range (1/2/4 oct) and ramp Length",
            ) {
                randomize_track(setter, track);
            }
            if text_button(ui, "CLEAR", "Remove every note from this track\n(Pulses to 0, manual edits and step pitches cleared)") {
                clear_track(setter, track);
            }
            ui.add_space(12.0);
            let page_now = *page;
            let paste_hint = match &state.clipboard {
                Some(snap) if snap.from == page_now => "Paste the copied track back here".to_string(),
                Some(snap) => format!("Paste track {} here\n(keeps this track's ON switch and MIDI channel)", snap.from + 1),
                None => "Copy a track first".to_string(),
            };
            if text_button(ui, "PASTE", &paste_hint) {
                if let Some(snap) = &state.clipboard {
                    paste_track(setter, track, snap);
                }
            }
            let copy_label = match &state.clipboard {
                Some(snap) => format!("COPY ({})", snap.from + 1),
                None => "COPY".to_string(),
            };
            if text_button(ui, &copy_label, "Copy this track, to paste onto another one") {
                state.clipboard = Some(copy_track(track, page_now));
            }
        });
    });
}

/// Two rows of 16 cells. Cells past the sequence length are dimmed; clicking a
/// cell sets the sequence length to end on it. With a ramp active, each step
/// shows its pitch as a short line (higher line = higher note).
/// `loop_idx` is which pass through the sequence is playing; velocity ramps
/// longer than the pattern differ from loop to loop, so the grid shows the
/// current one.
fn step_grid(ui: &mut Ui, track: &TrackParams, setter: &ParamSetter, playhead: i32, loop_idx: i64, mix: Mix) {
    let length = track.length.value() as usize;
    let e_steps = track.effective_euclid_steps();
    let pattern = track.current_pattern();
    let added = track.added.load(Ordering::Relaxed);
    let removed = track.removed.load(Ordering::Relaxed);
    let ramp = track.ramp();
    // Off (globally or here), muted or soloed out: show the pattern quietly.
    let on = track.audible(mix);
    let dir = track.direction.value();
    let offsets = track.pitch_offsets.load();

    // Pitch of each cell for the pass now playing. Climb/Descend only give
    // notes a pitch, so rests show none in those modes.
    let degrees: [Option<i32>; MAX_STEPS] = std::array::from_fn(|i| {
        let shown = i < length && (pattern[i] || !ramp.per_note());
        shown.then(|| track.step_degree(cell_step_in_pass(loop_idx, i, length, dir), i, &pattern))
    });
    let lo = degrees.iter().flatten().copied().min().unwrap_or(0);
    let hi = degrees.iter().flatten().copied().max().unwrap_or(0);

    let gap = 6.0;
    let cols = 16;
    let cell = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let grid_h = cell * 2.0 + gap;
    let (grid_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), grid_h), Sense::hover());
    let painter = ui.painter_at(grid_rect.expand(4.0));

    // Half-bar marks: a faint tick in the gap after steps 8 and 24, so the
    // middle of each row is easy to find.
    for row in 0..2 {
        let x = grid_rect.min.x + 8.0 * (cell + gap) - gap / 2.0;
        let y = grid_rect.min.y + row as f32 * (cell + gap) + cell / 2.0;
        let half = (cell * 0.18).round();
        painter.line_segment([pos2(x, y - half), pos2(x, y + half)], Stroke::new(1.5_f32, HALF_MARK));
    }

    for i in 0..MAX_STEPS {
        let (row, col) = (i / cols, i % cols);
        let min = grid_rect.min + vec2(col as f32 * (cell + gap), row as f32 * (cell + gap));
        let rect = Rect::from_min_size(min, vec2(cell, cell));
        let resp = ui.interact(rect, Id::new(("step", i)), Sense::click_and_drag());
        let bit = 1u32 << i;
        let pitch_drag_id = Id::new(("pitch_drag", i));
        let dragging_pitch = ui.memory(|m| m.data.get_temp::<(i8, f32)>(pitch_drag_id)).is_some();

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
        if dragging_pitch {
            painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(2.0_f32, ACCENT), StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, TEXT_DIM), StrokeKind::Inside);
        }

        // Pitch contour (ramp + step pitches), scaled to the lowest and
        // highest pitch on the grid. Nothing to draw when all steps match.
        if let (Some(deg), true) = (degrees[i], hi > lo) {
            let t = (deg - lo) as f32 / (hi - lo) as f32;
            // Kept below the top strip, where a step's "+2" label sits.
            let y = rect.bottom() - 9.0 - t * (cell - 27.0);
            let w = cell * 0.3;
            let color = if hit { BG } else { ACCENT_DIM };
            painter.line_segment(
                [pos2(rect.center().x - w, y), pos2(rect.center().x + w, y)],
                Stroke::new(2.0_f32, color),
            );
        }

        // Velocity: a bar along the bottom of each hit, as wide as it is loud.
        if hit {
            let vel = track.step_velocity(cell_step_in_pass(loop_idx, i, length, track.direction.value()), &pattern) as f32 / 127.0;
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

        // The step's own pitch offset, shown as "+2" / "-1".
        if active && offsets[i] != 0 && !dragging_pitch {
            painter.text(
                pos2(rect.center().x, rect.top() + 9.0),
                Align2::CENTER_CENTER,
                format!("{:+}", offsets[i]),
                FontId::monospace(10.0),
                if hit { BG } else { TEXT_DIM },
            );
        }

        // Cmd+drag (or Ctrl+drag / right-drag) up or down changes the step's
        // pitch in scale steps; 8 px per step, like a knob.
        let edit_mod = edit_modifier(ui);
        let starts_pitch_drag = (resp.drag_started_by(egui::PointerButton::Primary) && edit_mod)
            || resp.drag_started_by(egui::PointerButton::Secondary);
        if starts_pitch_drag && active {
            ui.memory_mut(|m| m.data.insert_temp(pitch_drag_id, (offsets[i], 0.0_f32)));
        }
        if let Some((start, acc)) = ui.memory(|m| m.data.get_temp::<(i8, f32)>(pitch_drag_id)) {
            if resp.dragged() {
                let acc = acc - resp.drag_delta().y;
                ui.memory_mut(|m| m.data.insert_temp(pitch_drag_id, (start, acc)));
                let new = (start as i32 + (acc / 8.0).round() as i32).clamp(-24, 24) as i8;
                if new != offsets[i] {
                    let mut all = track.pitch_offsets.load();
                    all[i] = new;
                    track.pitch_offsets.store(all);
                }
                // Show the note being set right on the step.
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    note_name(track.step_note(cell_step_in_pass(loop_idx, i, length, dir), i, &pattern) as i32),
                    FontId::monospace(12.0),
                    if hit { BG } else { TEXT },
                );
            }
            if resp.drag_stopped() || !resp.dragged() {
                ui.memory_mut(|m| m.data.remove::<(i8, f32)>(pitch_drag_id));
            }
        }

        // Plain drag: end the sequence here (like a click), then up/down sets
        // the number of pulses, 8 px per pulse like a knob. One gesture, so the
        // host records it as a single undoable change.
        let pulse_drag_id = Id::new(("pulse_drag", i));
        if resp.drag_started_by(egui::PointerButton::Primary) && !edit_mod {
            set(setter, &track.length, i as i32 + 1);
            // Pulses can't exceed the euclidean cycle; while linked, the cycle
            // is the new sequence length.
            let cycle = if track.link.value() { i as i32 + 1 } else { track.euclid_steps.value() };
            let start = track.euclid_pulses.value().min(cycle);
            setter.begin_set_parameter(&track.euclid_pulses);
            ui.memory_mut(|m| m.data.insert_temp(pulse_drag_id, (start, cycle, 0.0_f32)));
        }
        if let Some((start, cycle, acc)) = ui.memory(|m| m.data.get_temp::<(i32, i32, f32)>(pulse_drag_id)) {
            if resp.dragged() {
                let acc = acc - resp.drag_delta().y;
                ui.memory_mut(|m| m.data.insert_temp(pulse_drag_id, (start, cycle, acc)));
                let pulses = (start + (acc / 8.0).round() as i32).clamp(0, cycle);
                if pulses != track.euclid_pulses.value() {
                    setter.set_parameter(&track.euclid_pulses, pulses);
                }
                // Show the pulse count on the step while dragging.
                let badge = Rect::from_center_size(rect.center(), vec2(30.0, 20.0));
                painter.rect_filled(badge, CornerRadius::same(4), BG);
                painter.text(badge.center(), Align2::CENTER_CENTER, pulses.to_string(), FontId::monospace(13.0), ACCENT);
            }
            if resp.drag_stopped() || !resp.dragged() {
                setter.end_set_parameter(&track.euclid_pulses);
                ui.memory_mut(|m| m.data.remove::<(i32, i32, f32)>(pulse_drag_id));
            }
        }

        // Cmd+click (or Ctrl+click / right-click, which hosts never swallow)
        // toggles the step; a plain click sets the sequence length.
        let edit_click = (resp.clicked() && edit_mod) || resp.secondary_clicked();
        if edit_click && active {
            let (a, r) = toggle_override(added, removed, i, hit);
            track.added.store(a, Ordering::Relaxed);
            track.removed.store(r, Ordering::Relaxed);
        } else if resp.clicked() {
            set(setter, &track.length, i as i32 + 1);
        }

        let hover = if active {
            let note = note_name(track.step_note(cell_step_in_pass(loop_idx, i, length, dir), i, &pattern) as i32);
            let what = if hit {
                format!(
                    "{} · velocity {}",
                    note,
                    track.step_velocity(cell_step_in_pass(loop_idx, i, length, track.direction.value()), &pattern)
                )
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
            let pitch = match offsets[i] {
                0 => String::new(),
                o => format!(" · pitch {o:+}"),
            };
            format!(
                "Step {} · {}{}{}\nClick: end the sequence here\nDrag up/down: end here + more/fewer pulses\nCmd+click: {} a note\nCmd+drag up/down: change this step's pitch",
                i + 1,
                what,
                manual,
                pitch,
                if hit { "remove" } else { "add" }
            )
        } else {
            format!("Step {}\nClick: end the sequence here\nDrag up/down: end here + more/fewer pulses", i + 1)
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

/// Inner height shared by the three panels so their edges line up.
const PANEL_INNER_H: f32 = 3.0 * (18.0 + KNOB_H) + 2.0 * 14.0;

/// A rounded panel that visually groups related sections.
fn panel(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(SECTION)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_height(PANEL_INNER_H);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                add(ui)
            });
        });
}

fn scale_combo(ui: &mut Ui, id: impl std::hash::Hash, current: Scale, width: f32) -> Option<Scale> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        // Tall enough to show every scale without scrolling.
        .height(420.0)
        .selected_text(egui::RichText::new(current.name()).color(TEXT))
        .show_ui(ui, |ui| {
            for scale in Scale::ALL {
                if ui.selectable_label(current == scale, scale.name()).clicked() {
                    picked = Some(scale);
                }
            }
        });
    picked
}

fn controls(ui: &mut Ui, params: &EuclidSeqParams, track: &TrackParams, setter: &ParamSetter, page: usize) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;

        // Timing: when notes play and how hard.
        panel(ui, |ui| {
            group(ui, "RHYTHM", |ui| {
                let linked = track.link.value();
                knob(ui, setter, &track.length, "STEPS", KnobStyle::typeable());
                link_button(ui, setter, track);
                // While linked, the E. Steps knob *is* the Steps knob, so both
                // always move together, including under host automation.
                if linked {
                    knob(ui, setter, &track.length, "E. STEPS", KnobStyle::typeable());
                } else {
                    knob(ui, setter, &track.euclid_steps, "E. STEPS", KnobStyle::typeable());
                }
                let pulses = KnobStyle { text: Some(track.effective_pulses().to_string()), ..KnobStyle::typeable() };
                knob(ui, setter, &track.euclid_pulses, "PULSES", pulses);
                knob(ui, setter, &track.rotate, "ROTATE", KnobStyle { bipolar: true, ..KnobStyle::typeable() });
                knob(ui, setter, &track.probability, "PROB", KnobStyle::typeable());
            });
            ui.add_space(14.0);
            group(ui, "TIMING", |ui| {
                let rate_text = format!("{}{}", track.rate, if track.dotted.value() { "." } else { "" });
                let rate = KnobStyle { stepped: Some(Rate::variants().len()), text: Some(rate_text), ..Default::default() };
                knob(ui, setter, &track.rate, "RATE", rate);
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 12.0);
                    pill_toggle(ui, setter, &track.dotted, "DOT", "DOT", "Dotted: each step lasts 1.5x as long");
                    ui.add_space(28.0);
                    ui.label(egui::RichText::new("DOTTED").color(TEXT_DIM).size(10.5));
                });
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 12.0);
                    let dirs = [
                        (Direction::Forward, "FWD".to_string()),
                        (Direction::Reverse, "REV".to_string()),
                        (Direction::PingPong, "P-P".to_string()),
                    ];
                    segmented(ui, setter, &track.direction, &dirs, 38.0);
                    ui.add_space(24.0);
                    ui.label(egui::RichText::new("DIRECTION").color(TEXT_DIM).size(10.5));
                });
                // One GATE knob, showing whichever unit is selected.
                match track.gate_unit.value() {
                    GateUnit::Percent => {
                        knob(ui, setter, &track.gate_pct, "GATE", KnobStyle::typeable());
                    }
                    GateUnit::Ms => {
                        knob(ui, setter, &track.gate_ms, "GATE", KnobStyle::typeable());
                    }
                }
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 12.0);
                    segmented(ui, setter, &track.gate_unit, &[(GateUnit::Percent, "%".into()), (GateUnit::Ms, "ms".into())], 32.0);
                    ui.add_space(24.0);
                    ui.label(egui::RichText::new("UNIT").color(TEXT_DIM).size(10.5));
                });
            });
            ui.add_space(14.0);
            group(ui, "VELOCITY", |ui| {
                knob(ui, setter, &track.velocity, "VELOCITY", KnobStyle::typeable());
                // Same shapes as the pitch ramp, as a dropdown (still automatable).
                labelled(ui, "MODE", |ui| enum_combo(ui, ("vel_mode", page), setter, &track.vel_mode, 92.0));
                knob(ui, setter, &track.vel_low, "LOW", KnobStyle::typeable());
                knob(ui, setter, &track.vel_high, "HIGH", KnobStyle::typeable());
                knob(ui, setter, &track.vel_length, "LENGTH", KnobStyle::typeable());
                let random = track.vel_mode.value() == RampMode::Random;
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 16.0);
                    dice_button(ui, setter, &track.vel_seed, random);
                });
            });
        });

        // Pitch and its ramp, together.
        panel(ui, |ui| {
            group(ui, "PITCH", |ui| {
                knob(ui, setter, &track.pitch, "PITCH", KnobStyle { bipolar: true, ..KnobStyle::typeable() });
                knob(ui, setter, &track.root, "ROOT", KnobStyle::typeable());
                // Octave of the root: moves it by whole octaves, keeping the note.
                let root = track.root.value();
                let octave = root.div_euclid(12) - 2;
                let (lo, hi) = octave_limits(root.rem_euclid(12));
                if let Some(new) = labelled(ui, "OCTAVE", |ui| octave_stepper(ui, ("oct", page), octave, lo, hi)) {
                    set(setter, &track.root, root + (new - octave) * 12);
                }
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 12.0);
                    if let Some(scale) = scale_combo(ui, ("scale", page), track.scale.value(), 118.0) {
                        set(setter, &track.scale, scale);
                    }
                    ui.add_space(22.0);
                    ui.label(egui::RichText::new("SCALE").color(TEXT_DIM).size(10.5));
                });
            });
            ui.add_space(14.0);
            group(ui, "PITCH RAMP", |ui| {
                // A dropdown, but still the same automatable parameter.
                labelled(ui, "MODE", |ui| enum_combo(ui, ("ramp_mode", page), setter, &track.ramp_mode, 100.0));
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
                // A custom range in semitones; turning it selects it, clicking
                // 1/2/4 goes back to octaves.
                let custom = track.ramp_range.value() == RampRange::Custom;
                let style = KnobStyle { bipolar: true, dim: !custom, ..KnobStyle::typeable() };
                if knob(ui, setter, &track.ramp_semitones, "RANGE ST", style).is_some() && !custom {
                    set(setter, &track.ramp_range, RampRange::Custom);
                }
                let enabled = track.ramp_mode.value() == RampMode::Random;
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 16.0);
                    dice_button(ui, setter, &track.ramp_seed, enabled);
                });
            });
            ui.add_space(14.0);
            group(ui, "RAMP LENGTH", |ui| {
                let free = track.ramp_free.value();
                let style = if free {
                    KnobStyle::typeable()
                } else {
                    // Following the pattern: show the length actually used.
                    KnobStyle { text: Some(track.length.value().to_string()), dim: true, ..KnobStyle::typeable() }
                };
                // Turning the knob is a clear sign you want your own length.
                if knob(ui, setter, &track.ramp_length, "LENGTH", style).is_some() && !free {
                    set(setter, &track.ramp_free, true);
                }
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 12.0);
                    pill_toggle(
                        ui,
                        setter,
                        &track.ramp_free,
                        "FREE",
                        "FREE",
                        "Free: the ramp runs over its own length and drifts against the pattern.\nOff: the ramp spans the sequence (Steps).",
                    );
                    ui.add_space(28.0);
                    ui.label(egui::RichText::new("FREE").color(TEXT_DIM).size(10.5));
                });
                ui.vertical(|ui| {
                    ui.add_space(DIAL_Y - 14.0);
                    let unit = if track.ramp().per_note() { "notes" } else { "steps" };
                    let text = if free {
                        format!("Own cycle of {} {}:\ndrifts against the pattern", track.ramp_length.value(), unit)
                    } else {
                        "Follows Steps".to_string()
                    };
                    ui.label(egui::RichText::new(text).color(TEXT_DIM).size(10.5));
                });
            });
        });

        // Settings shared by all tracks.
        panel(ui, |ui| global_section(ui, params, setter));
    });
}

/// Global key and scale. Changing them here copies them to every track;
/// changing a track's own Root/Scale afterwards only affects that track.
fn global_section(ui: &mut Ui, params: &EuclidSeqParams, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("GLOBAL").color(TEXT_DIM).size(10.5).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            pill_toggle(
                ui,
                setter,
                &params.global_on,
                "ON",
                "OFF",
                "Switch all tracks on or off\n(each track keeps its own ON setting)",
            );
        });
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let key = KnobStyle { text: Some(params.global_root.to_string()), ..KnobStyle::typeable() };
        if let Some(pc) = knob(ui, setter, &params.global_root, "ROOT", key) {
            // New key, same octave for every track.
            for track in params.tracks.iter() {
                let current = track.root.value();
                let mut root = current - current.rem_euclid(12) + pc;
                while root > 96 {
                    root -= 12;
                }
                while root < 24 {
                    root += 12;
                }
                set(setter, &track.root, root);
            }
        }
        // Swing for every track: 50 % straight, ~66 % triplet shuffle, 75 % max.
        knob(ui, setter, &params.global_swing, "SWING", KnobStyle::typeable());
    });
    // Global octave: every track's root moves into this octave, keeping its note.
    ui.horizontal(|ui| {
        ui.add_space(-6.0);
        let octave = params.global_octave.value();
        if let Some(new) = octave_stepper(ui, "global_oct", octave, 0, 6) {
            set(setter, &params.global_octave, new);
            for track in params.tracks.iter() {
                let pc = track.root.value().rem_euclid(12);
                let mut root = pc + (new + 2) * 12;
                if root > 96 {
                    root -= 12;
                }
                set(setter, &track.root, root);
            }
        }
    });
    ui.label(egui::RichText::new("OCTAVE").color(TEXT_DIM).size(10.5));
    ui.add_space(8.0);
    if let Some(scale) = scale_combo(ui, "global_scale", params.global_scale.value(), 112.0) {
        set(setter, &params.global_scale, scale);
        for track in params.tracks.iter() {
            set(setter, &track.scale, scale);
        }
    }
    ui.label(egui::RichText::new("SCALE").color(TEXT_DIM).size(10.5));
    ui.add_space(14.0);

    // Which tracks still match the global key and scale.
    ui.label(egui::RichText::new("FOLLOWING").color(TEXT_DIM).size(10.5));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (pc, scale) = (params.global_root.value(), params.global_scale.value());
        for (t, track) in params.tracks.iter().enumerate() {
            let follows = track.follows_global(pc, scale);
            let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(4), if follows { ACCENT } else { PANEL });
            p.text(rect.center(), Align2::CENTER_CENTER, format!("{}", t + 1), FontId::monospace(11.0), if follows { BG } else { TEXT_DIM });
            resp.on_hover_text(if follows {
                format!("Track {} uses the global key and scale", t + 1)
            } else {
                format!(
                    "Track {} has its own key or scale ({} {})\nChange a global setting to bring it back in line",
                    t + 1,
                    note_name(track.root.value()),
                    track.scale.value().name()
                )
            });
        }
    });

    // Whole-plugin actions, each needing a second click to confirm.
    ui.add_space(18.0);
    if confirm_button(ui, "clear_all", "CLEAR ALL", "Remove every note from all 4 tracks") {
        for track in params.tracks.iter() {
            clear_track(setter, track);
        }
    }
    ui.add_space(4.0);
    if confirm_button(ui, "reset_all", "RESET ALL", "Return the whole plugin to its factory settings\n(all tracks, ON switches, MIDI channels, global key and scale)") {
        reset_all(setter, params);
    }
}

/// Every parameter of the plugin back to its default and all manual edits
/// cleared: the plugin as it was when first loaded.
fn reset_all(setter: &ParamSetter, params: &EuclidSeqParams) {
    for (_, ptr, _) in params.param_map() {
        // SAFETY: pointers from the plugin's own param map, valid while it lives.
        unsafe {
            let ctx = setter.raw_context;
            ctx.raw_begin_set_parameter(ptr);
            ctx.raw_set_parameter_normalized(ptr, ptr.default_normalized_value());
            ctx.raw_end_set_parameter(ptr);
        }
    }
    for track in params.tracks.iter() {
        track.clear_edits();
    }
}

/// A button for destructive actions: the first click arms it ("SURE?") for
/// three seconds, a second click runs the action. Returns true on confirm.
fn confirm_button(ui: &mut Ui, key: &str, label: &str, hint: &str) -> bool {
    let id = Id::new(("confirm", key));
    let now = ui.input(|i| i.time);
    let armed = ui.memory(|m| m.data.get_temp::<f64>(id)).is_some_and(|until| now < until);

    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 26.0), Sense::click());
    let p = ui.painter();
    let (fill, color, text) = if armed {
        (ACCENT, BG, "SURE?")
    } else if resp.hovered() {
        (CELL_OFF, ACCENT, label)
    } else {
        (PANEL, TEXT, label)
    };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(11.5), color);

    let mut confirmed = false;
    if resp.clicked() {
        if armed {
            confirmed = true;
            ui.memory_mut(|m| m.data.remove::<f64>(id));
        } else {
            ui.memory_mut(|m| m.data.insert_temp(id, now + 3.0));
        }
    }
    resp.on_hover_text(format!("{hint}\nClick twice to confirm"));
    confirmed
}

/// Empties the track: no euclidean pulses, no manually added notes.
fn clear_track(setter: &ParamSetter, track: &TrackParams) {
    set(setter, &track.euclid_pulses, 0);
    track.clear_edits();
}

/// Every setting of the track back to its default, except whether the track is
/// on and its MIDI channel (resetting those would silence or reroute it).
fn reset_track(setter: &ParamSetter, track: &TrackParams) {
    for (id, ptr, _) in track.param_map() {
        if matches!(id.as_str(), "on" | "channel" | "mute" | "solo") {
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
    track.clear_edits();
}

/// A widget placed at dial height with a caption under it, matching the knobs.
fn labelled<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.vertical(|ui| {
        ui.add_space(DIAL_Y - 12.0);
        let r = add(ui);
        ui.add_space(22.0);
        ui.label(egui::RichText::new(label).color(TEXT_DIM).size(10.5));
        r
    })
    .inner
}

/// Octaves a root with pitch class `pc` can sit in, within C0 (24) … C6 (96).
fn octave_limits(pc: i32) -> (i32, i32) {
    (0, if pc == 0 { 6 } else { 5 })
}

/// [−] octave [+]; scroll over it works too. Returns the new octave.
fn octave_stepper(ui: &mut Ui, id: impl std::hash::Hash, octave: i32, min: i32, max: i32) -> Option<i32> {
    let mut result = None;
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let step_button = |ui: &mut Ui, label: &str, enabled: bool| -> bool {
                let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 24.0), Sense::click());
                let p = ui.painter();
                let fill = if enabled && resp.hovered() { CELL_OFF } else { PANEL };
                p.rect_filled(rect, CornerRadius::same(4), fill);
                p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(13.0), if enabled { TEXT } else { CELL_OFF });
                enabled && resp.clicked()
            };
            if step_button(ui, "-", octave > min) {
                result = Some(octave - 1);
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::hover());
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, octave.to_string(), FontId::monospace(13.0), ACCENT);
            let scroll = if resp.hovered() { ui.input(|i| i.raw_scroll_delta.y) } else { 0.0 };
            if step_button(ui, "+", octave < max) {
                result = Some(octave + 1);
            }
            if scroll > 0.0 && octave < max {
                result = Some(octave + 1);
            } else if scroll < 0.0 && octave > min {
                result = Some(octave - 1);
            }
        });
    });
    result
}

/// Dropdown for any list parameter, in the list's own order. It sets the
/// parameter like any other control, so hosts can still automate it.
fn enum_combo<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    setter: &ParamSetter,
    param: &EnumParam<T>,
    width: f32,
) {
    let names = T::variants();
    let current = param.value().to_index();
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(egui::RichText::new(names[current]).color(TEXT))
        .show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                if ui.selectable_label(i == current, *name).clicked() {
                    set(setter, param, T::from_index(i));
                }
            }
        });
}

/// Bottom bar: pick a preset to load it; type a name and SAVE; FOLDER shows
/// the preset files. A preset is the whole plugin (all tracks + Global).
fn preset_bar(ui: &mut Ui, setter: &ParamSetter, state: &mut EditorState) {
    let now = ui.input(|i| i.time);
    egui::Frame::new()
        .fill(SECTION)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(egui::RichText::new("PRESET").color(TEXT_DIM).size(10.5).strong());

                // The list is read from the folder each time it's opened, so
                // files added or renamed in Finder/Explorer show up.
                let shown = state.current_preset.clone().unwrap_or_else(|| "—".to_string());
                let mut to_load = None;
                egui::ComboBox::from_id_salt("preset_list")
                    .width(200.0)
                    .height(360.0)
                    .selected_text(egui::RichText::new(shown).color(TEXT))
                    .show_ui(ui, |ui| {
                        let names = presets::list();
                        if names.is_empty() {
                            ui.label(egui::RichText::new("No presets yet").color(TEXT_DIM));
                        }
                        for name in names {
                            let current = state.current_preset.as_deref() == Some(name.as_str());
                            if ui.selectable_label(current, &name).clicked() {
                                to_load = Some(name);
                            }
                        }
                    })
                    .response
                    .on_hover_text("Pick a preset to load it (all tracks and Global settings)");
                if let Some(name) = to_load {
                    match presets::load(&name) {
                        Ok(preset) => {
                            setter.raw_context.set_state(preset);
                            state.status = Some((format!("Loaded '{name}'"), false, now + 3.0));
                            state.preset_name = name.clone();
                            state.current_preset = Some(name);
                        }
                        Err(e) => state.status = Some((e, true, now + 6.0)),
                    }
                }

                ui.add_space(10.0);
                ui.add(
                    egui::TextEdit::singleline(&mut state.preset_name)
                        .hint_text("Preset name")
                        .desired_width(170.0),
                )
                .on_hover_text("Name for SAVE. Left empty, SAVE picks \"Preset 1\", \"Preset 2\", …");

                // Saving over an existing preset needs a second click.
                let typed = presets::sanitize(&state.preset_name);
                let name = if typed.is_empty() { presets::next_free_name() } else { typed };
                let exists = presets::exists(&name);
                let armed = exists && now < state.overwrite_until;
                let (label, hint) = if armed {
                    ("REPLACE?", format!("Click again to replace '{name}'"))
                } else {
                    ("SAVE", format!("Save everything as '{name}'"))
                };
                if button(ui, label, &hint, armed) {
                    if exists && !armed {
                        state.overwrite_until = now + 3.0;
                    } else {
                        state.overwrite_until = 0.0;
                        match presets::save(&name, &setter.raw_context.get_state()) {
                            Ok(saved) => {
                                state.status = Some((format!("Saved '{saved}'"), false, now + 3.0));
                                state.preset_name = saved.clone();
                                state.current_preset = Some(saved);
                            }
                            Err(e) => state.status = Some((e, true, now + 6.0)),
                        }
                    }
                }
                let folder_hint = match presets::dir() {
                    Some(d) => format!("Show the preset files\n{}", d.display()),
                    None => "Show the preset files".to_string(),
                };
                if text_button(ui, "FOLDER", &folder_hint) {
                    if let Err(e) = presets::open_folder() {
                        state.status = Some((e, true, now + 6.0));
                    }
                }

                if let Some((msg, is_error, until)) = &state.status {
                    if now < *until {
                        ui.add_space(8.0);
                        let color = if *is_error { MUTE_COLOR } else { TEXT_DIM };
                        ui.label(egui::RichText::new(msg).color(color).size(11.5));
                    }
                }
            });
        });
}

/// Small square toggle for Mute / Solo, lit in `color` when on.
/// Returns true when clicked; the caller decides what changes.
fn flag_button(ui: &mut Ui, param: &BoolParam, label: &str, color: Color32, hint: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 26.0), Sense::click());
    let value = param.value();
    let p = ui.painter();
    let fill = if value { color } else if resp.hovered() { CELL_OFF } else { PANEL };
    p.rect_filled(rect, CornerRadius::same(4), fill);
    p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(11.0), if value { BG } else { TEXT_DIM });
    let clicked = resp.clicked();
    resp.on_hover_text(hint);
    clicked
}

/// New random values for the rhythm and melody shape of a track: Steps,
/// E. Steps, Pulses, Pitch, ramp Mode, ramp Range (octave presets) and ramp
/// Length. Ranges are kept musical: at least 4 steps and at least one pulse.
fn randomize_track(setter: &ParamSetter, track: &TrackParams) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678);
    let mut rnd = |lo: i32, hi: i32| -> i32 {
        seed = splitmix64(seed);
        lo + (seed % (hi - lo + 1) as u64) as i32
    };

    let steps = rnd(4, 32);
    set(setter, &track.length, steps);
    // Linked: E. Steps follows Steps. Unlinked: a cycle up to the sequence length.
    let e_steps = if track.link.value() { steps } else { rnd(2, steps) };
    set(setter, &track.euclid_steps, e_steps);
    set(setter, &track.euclid_pulses, rnd(1, e_steps));
    set(setter, &track.pitch, rnd(-14, 14));
    let modes = RampMode::variants().len() as i32;
    set(setter, &track.ramp_mode, RampMode::from_index(rnd(0, modes - 1) as usize));
    set(setter, &track.ramp_range, RampRange::ALL[rnd(0, 2) as usize]);
    set(setter, &track.ramp_length, rnd(1, 32));
}

/// Small text button; returns true when clicked.
fn text_button(ui: &mut Ui, label: &str, hint: &str) -> bool {
    button(ui, label, hint, false)
}

/// Text button; `hot` draws it lit (e.g. waiting for a confirming click).
fn button(ui: &mut Ui, label: &str, hint: &str, hot: bool) -> bool {
    let width = (label.len() as f32 * 8.0 + 20.0).max(58.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let p = ui.painter();
    let (fill, color) = if hot || resp.is_pointer_button_down_on() {
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
    /// Cmd/Ctrl+click lets you type a value.
    typeable: bool,
    /// Drawn subdued (the value isn't in use right now).
    dim: bool,
}

impl KnobStyle {
    fn typeable() -> Self {
        Self { typeable: true, ..Default::default() }
    }
}

/// Shared drag/scroll/double-click behaviour for knobs and value boxes.
/// Returns the value requested this frame, if any (parameter changes only land
/// a moment later, so callers that react to a change need it from here).
fn param_interaction<P: Param>(ui: &Ui, resp: &Response, setter: &ParamSetter, param: &P, speed: f32) -> Option<P::Plain>
where
    P::Plain: Copy,
{
    let drag_id = resp.id.with("drag_value");
    let mut requested = None;
    if resp.double_clicked() {
        let v = param.default_plain_value();
        set(setter, param, v);
        requested = Some(v);
    } else if resp.drag_started() {
        setter.begin_set_parameter(param);
        let start = param.unmodulated_normalized_value();
        ui.memory_mut(|m| m.data.insert_temp(drag_id, start));
    } else if resp.dragged() {
        // Accumulate in an unquantized value; stepped params snap on their own.
        let fine = ui.input(|i| i.modifiers.shift);
        let speed = if fine { speed / 4.0 } else { speed };
        let start: f32 = ui.memory(|m| m.data.get_temp(drag_id)).unwrap_or(0.0);
        let v = (start - resp.drag_delta().y * speed).clamp(0.0, 1.0);
        ui.memory_mut(|m| m.data.insert_temp(drag_id, v));
        if param.preview_normalized(param.preview_plain(v)) != param.preview_normalized(param.preview_plain(start)) {
            requested = Some(param.preview_plain(v));
        }
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
            requested = Some(next);
        }
    }
    requested
}

/// Is the "edit" modifier held? Cmd on macOS, but also Ctrl: hosts often keep
/// the Cmd key to themselves, while Ctrl always reaches the plugin with clicks.
fn edit_modifier(ui: &Ui) -> bool {
    // `command` is refreshed from every mouse event and carries Ctrl, so it is
    // always current. `mac_cmd` and `ctrl` only change on key events and get
    // stuck when the host swallows the key release, so they aren't used; Cmd
    // is asked from the system instead (see modkeys).
    ui.input(|i| i.modifiers.command) || crate::modkeys::cmd_held()
}

/// Text entry over a knob's value. Returns the value committed this frame.
fn type_in<P: Param>(ui: &mut Ui, setter: &ParamSetter, param: &P, at: Rect, edit_id: Id) -> Option<P::Plain> {
    let mut buf: String = ui.memory(|m| m.data.get_temp(edit_id))?;
    let te_id = edit_id.with("field");
    let resp = ui.put(
        at,
        egui::TextEdit::singleline(&mut buf)
            .id(te_id)
            .font(FontId::monospace(13.0))
            .horizontal_align(egui::Align::Center)
            .text_color(ACCENT)
            .frame(true)
            .margin(vec2(2.0, 2.0)),
    );
    let first_frame: bool = ui.memory(|m| m.data.get_temp(edit_id.with("new"))).unwrap_or(false);
    if first_frame {
        resp.request_focus();
        ui.memory_mut(|m| m.data.remove::<bool>(edit_id.with("new")));
    }
    if resp.lost_focus() || (!first_frame && !resp.has_focus()) {
        ui.memory_mut(|m| m.data.remove::<String>(edit_id));
        // Enter or clicking away confirms; Escape cancels.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            return None;
        }
        let normalized = param.string_to_normalized_value(buf.trim())?;
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, normalized);
        setter.end_set_parameter(param);
        return Some(param.preview_plain(normalized));
    }
    ui.memory_mut(|m| m.data.insert_temp(edit_id, buf));
    None
}

/// Rotary knob: drag vertically (Shift for fine), scroll for single steps,
/// double-click to reset.
fn knob<P: Param>(ui: &mut Ui, setter: &ParamSetter, param: &P, label: &str, style: KnobStyle) -> Option<P::Plain>
where
    P::Plain: Copy,
{
    let (rect, resp) = ui.allocate_exact_size(vec2(KNOB_W, KNOB_H), Sense::click_and_drag());
    let speed = if style.stepped.is_some() { 0.012 } else { 0.006 };
    let edit_id = resp.id.with("type_in");
    let value_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 70.0), vec2(KNOB_W - 2.0, 20.0));

    let mut requested = None;
    if style.typeable && resp.clicked() && edit_modifier(ui) {
        // Start typing, prefilled with the current value.
        let current = param.normalized_value_to_string(param.unmodulated_normalized_value(), false);
        ui.memory_mut(|m| {
            m.data.insert_temp(edit_id, current);
            m.data.insert_temp(edit_id.with("new"), true);
        });
    } else {
        requested = param_interaction(ui, &resp, setter, param, speed);
    }
    let editing = ui.memory(|m| m.data.get_temp::<String>(edit_id).is_some());
    if editing {
        if let Some(v) = type_in(ui, setter, param, value_rect, edit_id) {
            requested = Some(v);
        }
    }

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
                arc(from.min(angle), from.max(angle), if style.dim { ACCENT_DIM } else { ACCENT }, 4.0);
            }
        }
    }
    let tip = center + vec2(angle.cos(), angle.sin()) * (radius - 9.0);
    painter.line_segment([center, tip], Stroke::new(2.0_f32, TEXT));

    if !editing {
        let value = style.text.unwrap_or_else(|| param.to_string());
        painter.text(
            pos2(center.x, rect.top() + 70.0),
            Align2::CENTER_CENTER,
            value,
            FontId::monospace(13.0),
            if resp.hovered() || resp.dragged() {
                ACCENT
            } else if style.dim {
                TEXT_DIM
            } else {
                TEXT
            },
        );
    }
    painter.text(pos2(center.x, rect.top() + 86.0), Align2::CENTER_CENTER, label, FontId::proportional(10.5), TEXT_DIM);

    resp.on_hover_text(if style.typeable {
        "Drag (Shift = fine) · scroll · double-click to reset · Cmd/Ctrl+click to type"
    } else {
        "Drag (Shift = fine) · scroll · double-click to reset"
    });
    requested
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

fn pill_toggle(ui: &mut Ui, setter: &ParamSetter, param: &BoolParam, on: &str, off: &str, hint: &str) {
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
    resp.on_hover_text(hint);
}

/// Compact draggable number (used for the MIDI channel).
fn value_box(ui: &mut Ui, setter: &ParamSetter, param: &IntParam, label: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(62.0, 24.0), Sense::click_and_drag());
    let _ = param_interaction(ui, &resp, setter, param, 0.01);
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
