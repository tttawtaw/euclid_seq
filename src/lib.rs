use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;

mod editor;
pub mod sequencer;

use sequencer::{
    apply_overrides, gate_beats, note_name, parse_note_name, passes_probability, pattern, play_pos,
    ramp_velocity, scale_note, song_note_index, Direction, GateUnit, Ramp, RampMode,
    RampRange, Rate, Scale, VelocityMode, MAX_STEPS,
};

pub const NUM_TRACKS: usize = 4;
/// Playhead value meaning "transport stopped".
pub const STOPPED: i64 = i64::MIN;
/// Room for every note on/off of all tracks in one block, even at 1/64.
const MAX_EVENTS: usize = 1024;

/// How many notes one track can hold at once (gates longer than a step
/// overlap). Past this, the oldest note is released.
const MAX_HELD: usize = 32;

/// A note a track is holding: what was sent, and when it must stop.
#[derive(Clone, Copy)]
struct Held {
    note: u8,
    channel: u8,
    on_beat: f64,
    off_beat: f64,
}

/// A note event waiting to be sent; `seq` keeps same-sample events in the
/// order they were generated (note-off before the next note-on).
#[derive(Clone, Copy)]
struct Pending {
    timing: u32,
    seq: u32,
    on: bool,
    note: u8,
    channel: u8,
    velocity: f32,
}

pub struct EuclidSeq {
    params: Arc<EuclidSeqParams>,
    /// Global step under the playhead for each track (steps since beat 0, so
    /// the GUI can tell which loop it is in), or STOPPED.
    playheads: Arc<[AtomicI64; NUM_TRACKS]>,
    /// Notes each track is holding, so they are always released with exactly
    /// what was sent, even if pitch, channel, rate or gate changed meanwhile.
    held: [[Option<Held>; MAX_HELD]; NUM_TRACKS],
    /// Where the previous block ended, to detect loops and jumps.
    last_end_beat: Option<f64>,
    /// Pre-allocated event buffer (never grows on the audio thread).
    events: Vec<Pending>,
}

#[derive(Params)]
pub struct EuclidSeqParams {
    /// Deliberately not persisted: a size saved by an older version would
    /// reopen the window too small for newer controls.
    editor_state: Arc<EguiState>,

    /// Global key (pitch class 0-11). Setting it in the GUI copies it to every
    /// track, keeping each track's octave. Not automatable, because that copy
    /// happens in the GUI.
    #[id = "g_root"]
    pub global_root: IntParam,

    /// Global scale. Setting it in the GUI copies it to every track.
    #[id = "g_scale"]
    pub global_scale: EnumParam<Scale>,

    /// Parameter IDs get a `_1` … `_4` suffix per track.
    #[nested(array, group = "Track")]
    pub tracks: [TrackParams; NUM_TRACKS],
}

#[derive(Params)]
pub struct TrackParams {
    #[id = "on"]
    pub on: BoolParam,

    /// MIDI channel, 1-based for display.
    #[id = "channel"]
    pub channel: IntParam,

    /// Sequence length: the grid loops after this many steps.
    #[id = "length"]
    pub length: IntParam,

    /// When on, the euclidean cycle always equals the sequence length.
    #[id = "link"]
    pub link: BoolParam,

    /// Length of the euclidean cycle. It repeats inside the sequence length.
    #[id = "e_steps"]
    pub euclid_steps: IntParam,

    /// Number of hits spread evenly over the euclidean cycle.
    #[id = "e_pulses"]
    pub euclid_pulses: IntParam,

    /// How long each step lasts.
    #[id = "rate"]
    pub rate: EnumParam<Rate>,

    /// Dotted steps last 1.5x as long.
    #[id = "dotted"]
    pub dotted: BoolParam,

    /// Forward, Reverse or Ping-Pong.
    #[id = "direction"]
    pub direction: EnumParam<Direction>,

    /// Whether the gate is a % of the step or a time in ms.
    #[id = "gate_unit"]
    pub gate_unit: EnumParam<GateUnit>,

    /// Gate as a percentage of the step length (0-200 %).
    #[id = "gate_pct"]
    pub gate_pct: IntParam,

    /// Gate as a fixed time (10 ms - 20 s).
    #[id = "gate_ms"]
    pub gate_ms: FloatParam,

    /// Chance that each note plays (0-100 %).
    #[id = "probability"]
    pub probability: IntParam,

    /// Shifts the euclidean pattern right (+) or left (-) within its cycle.
    #[id = "rotate"]
    pub rotate: IntParam,

    /// Transposition in scale degrees, so the sequence always stays in key.
    #[id = "pitch"]
    pub pitch: IntParam,

    #[id = "scale"]
    pub scale: EnumParam<Scale>,

    /// Root note as a MIDI note number (60 = C3).
    #[id = "root"]
    pub root: IntParam,

    #[id = "ramp"]
    pub ramp_mode: EnumParam<RampMode>,

    #[id = "ramp_range"]
    pub ramp_range: EnumParam<RampRange>,

    /// Off: ramp goes 0..+range. On: -range..+range.
    #[id = "ramp_bipolar"]
    pub ramp_bipolar: BoolParam,

    /// Selects which random melody the Random ramp plays.
    #[id = "ramp_seed"]
    pub ramp_seed: IntParam,

    /// When on, the pitch ramp runs over `ramp_length` instead of the
    /// sequence length, following the song position, so the melody drifts in
    /// and out of phase with the rhythm.
    #[id = "ramp_free"]
    pub ramp_free: BoolParam,

    /// Ramp length when free: steps for Up/Down/Random, notes for Climb/Descend.
    #[id = "ramp_length"]
    pub ramp_length: IntParam,

    /// Fixed velocity (0-127). Also used when the velocity ramp is off or has length 1.
    #[id = "velocity"]
    pub velocity: IntParam,

    #[id = "vel_mode"]
    pub vel_mode: EnumParam<VelocityMode>,

    #[id = "vel_low"]
    pub vel_low: IntParam,

    #[id = "vel_high"]
    pub vel_high: IntParam,

    /// Notes per velocity ramp (1-16). Independent of the pattern's note count:
    /// the ramp keeps counting across loops.
    #[id = "vel_length"]
    pub vel_length: IntParam,

    /// Manually added steps (bit i = step i), toggled with Cmd+click.
    /// Saved with the project but not exposed as host parameters.
    #[persist = "added"]
    pub added: AtomicU32,

    /// Euclidean hits manually removed with Cmd+click.
    #[persist = "removed"]
    pub removed: AtomicU32,

    /// Per-step pitch offsets in scale degrees, set by Cmd+dragging a step.
    /// Added on top of everything else; saved with the project.
    #[persist = "pitch_offsets"]
    pub pitch_offsets: AtomicCell<[i8; MAX_STEPS]>,
}

impl TrackParams {
    fn new(index: usize) -> Self {
        // Each track starts with its own rhythm/register so turning one on
        // immediately adds something different. Only track 1 is on by default.
        let (pulses, e_steps, root) = [(5, 16, 60), (3, 8, 48), (7, 16, 72), (2, 5, 36)][index];
        let n = index + 1;
        let name = |s: &str| format!("T{n} {s}");
        Self {
            on: BoolParam::new(name("On"), index == 0),
            channel: IntParam::new(name("Channel"), n as i32, IntRange::Linear { min: 1, max: 16 }),
            length: IntParam::new(name("Steps"), 16, IntRange::Linear { min: 1, max: 32 }),
            link: BoolParam::new(name("Link"), e_steps == 16),
            euclid_steps: IntParam::new(
                name("Euclid Steps"),
                e_steps,
                IntRange::Linear { min: 1, max: 32 },
            ),
            euclid_pulses: IntParam::new(
                name("Euclid Pulses"),
                pulses,
                IntRange::Linear { min: 0, max: 32 },
            ),
            rate: EnumParam::new(name("Rate"), Rate::N16),
            dotted: BoolParam::new(name("Dotted"), false),
            direction: EnumParam::new(name("Direction"), Direction::Forward),
            gate_unit: EnumParam::new(name("Gate Unit"), GateUnit::Percent),
            gate_pct: IntParam::new(name("Gate %"), 50, IntRange::Linear { min: 0, max: 200 }).with_unit("%"),
            gate_ms: FloatParam::new(
                name("Gate ms"),
                250.0,
                FloatRange::Skewed { min: 10.0, max: 20_000.0, factor: FloatRange::skew_factor(-2.5) },
            )
            .with_value_to_string(Arc::new(|v| {
                // Decide the unit on the rounded value, so text converts back
                // to the same text (999.6 shows as "1.00 s", not "1000 ms").
                let r = v.round();
                if r < 1000.0 {
                    format!("{r:.0} ms")
                } else {
                    format!("{:.2} s", v / 1000.0)
                }
            }))
            .with_string_to_value(Arc::new(|s| {
                let s = s.trim().to_ascii_lowercase();
                if let Some(ms) = s.strip_suffix("ms") {
                    ms.trim().parse().ok()
                } else if let Some(sec) = s.strip_suffix('s') {
                    sec.trim().parse::<f32>().ok().map(|v| v * 1000.0)
                } else {
                    s.parse().ok()
                }
            })),
            probability: IntParam::new(name("Probability"), 100, IntRange::Linear { min: 0, max: 100 }).with_unit("%"),
            rotate: IntParam::new(name("Rotate"), 0, IntRange::Linear { min: -16, max: 16 })
                .with_value_to_string(Arc::new(|v| format!("{v:+}"))),
            pitch: IntParam::new(name("Pitch"), 0, IntRange::Linear { min: -14, max: 14 })
                .with_value_to_string(Arc::new(|v| format!("{v:+}"))),
            scale: EnumParam::new(name("Scale"), Scale::Minor),
            root: IntParam::new(name("Root"), root, IntRange::Linear { min: 24, max: 96 })
                .with_value_to_string(Arc::new(|v| note_name(v)))
                .with_string_to_value(Arc::new(|s| parse_note_name(s))),
            ramp_mode: EnumParam::new(name("Ramp"), RampMode::Off),
            ramp_range: EnumParam::new(name("Ramp Range"), RampRange::One),
            ramp_bipolar: BoolParam::new(name("Ramp Bipolar"), false)
                .with_value_to_string(Arc::new(|b| if b { "Bi" } else { "Uni" }.into()))
                .with_string_to_value(Arc::new(|s| match s.trim().to_ascii_lowercase().as_str() {
                    "bi" | "bipolar" | "on" | "1" => Some(true),
                    "uni" | "unipolar" | "off" | "0" => Some(false),
                    _ => None,
                })),
            ramp_free: BoolParam::new(name("Ramp Free Length"), false),
            ramp_length: IntParam::new(name("Ramp Length"), 16, IntRange::Linear { min: 1, max: 32 }),
            ramp_seed: IntParam::new(name("Ramp Seed"), 1, IntRange::Linear { min: 0, max: 9999 })
                .non_automatable(),
            velocity: IntParam::new(name("Velocity"), 75, IntRange::Linear { min: 0, max: 127 }),
            vel_mode: EnumParam::new(name("Velocity Ramp"), VelocityMode::Off),
            vel_low: IntParam::new(name("Velocity Low"), 40, IntRange::Linear { min: 0, max: 127 }),
            vel_high: IntParam::new(name("Velocity High"), 110, IntRange::Linear { min: 0, max: 127 }),
            vel_length: IntParam::new(name("Velocity Length"), 4, IntRange::Linear { min: 1, max: 16 }),
            added: AtomicU32::new(0),
            removed: AtomicU32::new(0),
            pitch_offsets: AtomicCell::new([0; MAX_STEPS]),
        }
    }

    /// Euclidean cycle length actually in use (follows Steps while linked).
    pub fn effective_euclid_steps(&self) -> usize {
        if self.link.value() {
            self.length.value() as usize
        } else {
            self.euclid_steps.value() as usize
        }
    }

    /// Pulses can't exceed the euclidean cycle length.
    pub fn effective_pulses(&self) -> usize {
        (self.euclid_pulses.value() as usize).min(self.effective_euclid_steps())
    }

    /// The euclidean pattern before manual edits.
    pub fn euclid_pattern(&self) -> [bool; MAX_STEPS] {
        pattern(
            self.length.value() as usize,
            self.effective_euclid_steps(),
            self.effective_pulses(),
            self.rotate.value(),
        )
    }

    /// The pattern that actually plays: euclidean hits plus manual edits,
    /// limited to the sequence length.
    pub fn current_pattern(&self) -> [bool; MAX_STEPS] {
        let mut p = apply_overrides(
            self.euclid_pattern(),
            self.added.load(Ordering::Relaxed),
            self.removed.load(Ordering::Relaxed),
        );
        for hit in p.iter_mut().skip(self.length.value() as usize) {
            *hit = false;
        }
        p
    }

    /// Velocity (0-127) of the note at `global_step` (steps since beat 0).
    pub fn step_velocity(&self, global_step: i64, pattern: &[bool; MAX_STEPS]) -> i32 {
        let len = self.length.value() as usize;
        ramp_velocity(
            self.velocity.value(),
            self.vel_mode.value(),
            self.vel_low.value(),
            self.vel_high.value(),
            self.vel_length.value() as usize,
            song_note_index(global_step, len, self.direction.value(), pattern),
        )
    }

    /// Number of notes in the playing pattern (the velocity ramp's max length).
    pub fn note_count(&self) -> usize {
        self.current_pattern().iter().filter(|&&h| h).count()
    }

    pub fn ramp(&self) -> Ramp {
        Ramp {
            mode: self.ramp_mode.value(),
            octaves: self.ramp_range.value().octaves(),
            bipolar: self.ramp_bipolar.value(),
            seed: self.ramp_seed.value() as u32,
        }
    }

    /// Scale degree (relative to the root) played on grid cell `cell` at
    /// `global_step` (steps since beat 0).
    ///
    /// With the ramp's length following the sequence, pitch belongs to the
    /// cell: directions only move the playhead and the grid always shows what
    /// will sound (Climb/Descend number notes in grid order). With a free
    /// length, the ramp follows the song position instead (steps, or notes in
    /// playing order for Climb/Descend), so it drifts against the pattern.
    /// The step's own pitch offset is added on top either way.
    pub fn step_degree(&self, global_step: i64, cell: usize, pattern: &[bool; MAX_STEPS]) -> i32 {
        let len = self.length.value() as usize;
        let ramp = self.ramp();
        let ramp_degree = if self.ramp_free.value() {
            let n = self.ramp_length.value().max(1) as i64;
            let note_index = song_note_index(global_step, len, self.direction.value(), pattern).rem_euclid(n);
            ramp.degree(global_step.rem_euclid(n) as usize, note_index as usize, n as usize, self.scale.value())
        } else {
            let note_index = pattern[..cell.min(len)].iter().filter(|&&h| h).count();
            ramp.degree(cell, note_index, len, self.scale.value())
        };
        let offset = self.pitch_offsets.load()[cell.min(MAX_STEPS - 1)] as i32;
        self.pitch.value() + ramp_degree + offset
    }

    pub fn step_note(&self, global_step: i64, cell: usize, pattern: &[bool; MAX_STEPS]) -> u8 {
        scale_note(self.root.value(), self.step_degree(global_step, cell, pattern), self.scale.value())
    }

    /// Forget all manual edits: added/removed notes and per-step pitches.
    pub fn clear_edits(&self) {
        self.added.store(0, Ordering::Relaxed);
        self.removed.store(0, Ordering::Relaxed);
        self.pitch_offsets.store([0; MAX_STEPS]);
    }

    pub fn step_beats(&self) -> f64 {
        self.rate.value().step_beats(self.dotted.value())
    }

    /// Does this track follow the global key and scale?
    pub fn follows_global(&self, root_pc: i32, scale: Scale) -> bool {
        self.root.value().rem_euclid(12) == root_pc && self.scale.value() == scale
    }
}

impl Default for EuclidSeqParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),
            global_root: IntParam::new("Global Root", 0, IntRange::Linear { min: 0, max: 11 })
                .with_value_to_string(Arc::new(|v| note_name(v).trim_end_matches(char::is_numeric).trim_end_matches('-').to_string()))
                .with_string_to_value(Arc::new(|s| {
                    // Accept "F#", "f#2" or a number; only the pitch class matters.
                    let s = s.trim();
                    let with_octave = if s.chars().last().is_some_and(|c| c.is_ascii_digit()) { s.to_string() } else { format!("{s}3") };
                    parse_note_name(&with_octave).map(|n| n.rem_euclid(12))
                }))
                .non_automatable(),
            global_scale: EnumParam::new("Global Scale", Scale::Minor).non_automatable(),
            tracks: std::array::from_fn(TrackParams::new),
        }
    }
}

impl Default for EuclidSeq {
    fn default() -> Self {
        Self {
            params: Arc::new(EuclidSeqParams::default()),
            playheads: Arc::new(std::array::from_fn(|_| AtomicI64::new(STOPPED))),
            held: [[None; MAX_HELD]; NUM_TRACKS],
            last_end_beat: None,
            events: Vec::with_capacity(MAX_EVENTS),
        }
    }
}

impl EuclidSeq {
    fn push(&mut self, timing: u32, on: bool, note: u8, channel: u8, velocity: f32) {
        if self.events.len() < MAX_EVENTS {
            let seq = self.events.len() as u32;
            self.events.push(Pending { timing, seq, on, note, channel, velocity });
        }
    }

    fn release_all_now(&mut self, context: &mut impl ProcessContext<Self>) {
        for slot in self.held.iter_mut().flatten() {
            if let Some(h) = slot.take() {
                context.send_event(NoteEvent::NoteOff {
                    timing: 0,
                    voice_id: None,
                    channel: h.channel,
                    note: h.note,
                    velocity: 0.0,
                });
            }
        }
    }

    /// Queue note-offs for every note of `track` matching `due`, at `at(note)`.
    fn release_where(
        &mut self,
        track: usize,
        due: impl Fn(&Held) -> bool,
        at: impl Fn(&Held) -> u32,
    ) {
        for i in 0..MAX_HELD {
            if let Some(h) = self.held[track][i] {
                if due(&h) {
                    self.held[track][i] = None;
                    self.push(at(&h), false, h.note, h.channel, 0.0);
                }
            }
        }
    }
}

impl Plugin for EuclidSeq {
    const NAME: &'static str = "Euclid Seq";
    const VENDOR: &'static str = "Thomas Verney";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Pure note effect: no audio in or out.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];
    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.playheads.clone())
    }

    fn reset(&mut self) {
        // The host will have flushed notes; just forget ours.
        self.held = [[None; MAX_HELD]; NUM_TRACKS];
        self.last_end_beat = None;
        for p in self.playheads.iter() {
            p.store(STOPPED, Ordering::Relaxed);
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        let transport = context.transport();
        let sample_rate = transport.sample_rate as f64;

        let (start_beat, tempo) = match (transport.playing, transport.pos_beats(), transport.tempo) {
            (true, Some(pos), Some(tempo)) if num_samples > 0 && tempo > 0.0 => (pos, tempo),
            _ => {
                self.release_all_now(context);
                self.last_end_beat = None;
                for p in self.playheads.iter() {
                    p.store(STOPPED, Ordering::Relaxed);
                }
                return ProcessStatus::Normal;
            }
        };

        let beats_per_sample = tempo / 60.0 / sample_rate;
        let end_beat = start_beat + beats_per_sample * num_samples as f64;
        let timing = |beat: f64| sequencer::beat_to_timing(beat, start_beat, beats_per_sample, num_samples);

        // The transport looped or jumped: stop whatever is sounding.
        if let Some(prev) = self.last_end_beat {
            if (start_beat - prev).abs() > beats_per_sample * 4.0 {
                self.release_all_now(context);
            }
        }
        self.last_end_beat = Some(end_beat);

        let params = self.params.clone();
        self.events.clear();

        for (t, track) in params.tracks.iter().enumerate() {
            let step_beats = track.step_beats();
            let gate = gate_beats(
                track.gate_unit.value(),
                track.gate_pct.value(),
                track.gate_ms.value(),
                step_beats,
                tempo,
            );
            let len = track.length.value() as usize;
            let dir = track.direction.value();
            let pattern = track.current_pattern();
            let on = track.on.value();
            let probability = track.probability.value();

            // Track switched off: let go of everything right away.
            if !on {
                self.release_where(t, |_| true, |_| 0);
            }

            for (k, beat) in sequencer::block_steps(start_beat, beats_per_sample, num_samples, step_beats) {
                // Notes whose gate has ended by this step.
                self.release_where(t, |h| h.off_beat <= beat, |h| timing(h.off_beat));
                if !on {
                    continue;
                }
                let cell = play_pos(k, len, dir).position;
                if !pattern[cell] || !passes_probability(probability, t, k) {
                    continue;
                }
                let note = track.step_note(k, cell, &pattern);
                let velocity = track.step_velocity(k, &pattern) as f32 / 127.0;
                let channel = (track.channel.value() - 1) as u8;

                // The same pitch still ringing (long gates): end it right
                // before retriggering, or most synths would cut the new note.
                self.release_where(t, |h| h.note == note && h.channel == channel, |_| timing(beat));
                // All slots busy: drop the oldest note.
                let slot = match self.held[t].iter().position(Option::is_none) {
                    Some(free) => free,
                    None => {
                        let oldest = (0..MAX_HELD)
                            .min_by(|&a, &b| {
                                let (a, b) = (self.held[t][a].unwrap(), self.held[t][b].unwrap());
                                a.on_beat.total_cmp(&b.on_beat)
                            })
                            .unwrap_or(0);
                        let h = self.held[t][oldest].take().unwrap();
                        self.push(timing(beat), false, h.note, h.channel, 0.0);
                        oldest
                    }
                };
                self.push(timing(beat), true, note, channel, velocity);
                self.held[t][slot] = Some(Held { note, channel, on_beat: beat, off_beat: beat + gate });
            }

            // Gates ending inside this block after the last step.
            self.release_where(t, |h| h.off_beat < end_beat, |h| timing(h.off_beat));

            // Playhead for the GUI: global step sounding at the end of this block.
            let last = ((end_beat - beats_per_sample) / step_beats).floor() as i64;
            self.playheads[t].store(last, Ordering::Relaxed);
        }

        // Hosts need events in time order; seq keeps same-sample order stable.
        self.events.sort_unstable_by_key(|e| (e.timing, e.seq));
        for e in self.events.iter() {
            context.send_event(if e.on {
                NoteEvent::NoteOn { timing: e.timing, voice_id: None, channel: e.channel, note: e.note, velocity: e.velocity }
            } else {
                NoteEvent::NoteOff { timing: e.timing, voice_id: None, channel: e.channel, note: e.note, velocity: 0.0 }
            });
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for EuclidSeq {
    const CLAP_ID: &'static str = "com.thomasverney.euclid-seq";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("4-track euclidean MIDI step sequencer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::NoteEffect, ClapFeature::Utility];
}

impl Vst3Plugin for EuclidSeq {
    const VST3_CLASS_ID: [u8; 16] = *b"EuclidSeqTVerney";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Tools];
}

nih_export_clap!(EuclidSeq);
nih_export_vst3!(EuclidSeq);
