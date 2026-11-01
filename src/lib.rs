use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;

mod editor;
pub mod sequencer;

use sequencer::{
    apply_overrides, note_name, parse_note_name, pattern, ramp_velocity, scale_note, Ramp, RampMode,
    RampRange, Rate, Scale, VelocityMode, MAX_STEPS,
};

pub const NUM_TRACKS: usize = 4;
/// Room for every note on/off of all tracks in one block, even at 1/64.
const MAX_EVENTS: usize = 1024;

/// A note a track is holding: what was sent, and when it must stop.
#[derive(Clone, Copy)]
struct Held {
    note: u8,
    channel: u8,
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
    /// Step under the playhead for each track (-1 when stopped), read by the GUI.
    playheads: Arc<[AtomicI32; NUM_TRACKS]>,
    /// The note each track is holding, so it is always released with exactly
    /// what was sent, even if pitch, channel or rate changed meanwhile.
    held: [Option<Held>; NUM_TRACKS],
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

    /// Fixed velocity (0-127). Also used when the velocity ramp is off or has length 1.
    #[id = "velocity"]
    pub velocity: IntParam,

    #[id = "vel_mode"]
    pub vel_mode: EnumParam<VelocityMode>,

    #[id = "vel_low"]
    pub vel_low: IntParam,

    #[id = "vel_high"]
    pub vel_high: IntParam,

    /// Notes per velocity ramp; capped at the number of notes in the pattern.
    #[id = "vel_length"]
    pub vel_length: IntParam,

    /// Manually added steps (bit i = step i), toggled with Cmd+click.
    /// Saved with the project but not exposed as host parameters.
    #[persist = "added"]
    pub added: AtomicU32,

    /// Euclidean hits manually removed with Cmd+click.
    #[persist = "removed"]
    pub removed: AtomicU32,
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
            ramp_seed: IntParam::new(name("Ramp Seed"), 1, IntRange::Linear { min: 0, max: 9999 })
                .non_automatable(),
            velocity: IntParam::new(name("Velocity"), 75, IntRange::Linear { min: 0, max: 127 }),
            vel_mode: EnumParam::new(name("Velocity Ramp"), VelocityMode::Off),
            vel_low: IntParam::new(name("Velocity Low"), 40, IntRange::Linear { min: 0, max: 127 }),
            vel_high: IntParam::new(name("Velocity High"), 110, IntRange::Linear { min: 0, max: 127 }),
            vel_length: IntParam::new(name("Velocity Length"), 4, IntRange::Linear { min: 1, max: 32 }),
            added: AtomicU32::new(0),
            removed: AtomicU32::new(0),
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

    /// Velocity (0-127) of the note on `step` of `pattern`.
    pub fn step_velocity(&self, step: usize, pattern: &[bool; MAX_STEPS]) -> i32 {
        let len = self.length.value() as usize;
        let note_index = pattern[..step.min(len)].iter().filter(|&&h| h).count();
        let total = pattern[..len].iter().filter(|&&h| h).count();
        ramp_velocity(
            self.velocity.value(),
            self.vel_mode.value(),
            self.vel_low.value(),
            self.vel_high.value(),
            self.vel_length.value() as usize,
            note_index,
            total,
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

    /// Scale degree (relative to the root) played on `step` of `pattern`.
    pub fn step_degree(&self, step: usize, pattern: &[bool; MAX_STEPS]) -> i32 {
        let len = self.length.value() as usize;
        let note_index = pattern[..step.min(len)].iter().filter(|&&h| h).count();
        self.pitch.value() + self.ramp().degree(step, note_index, len, self.scale.value())
    }

    pub fn step_note(&self, step: usize, pattern: &[bool; MAX_STEPS]) -> u8 {
        scale_note(self.root.value(), self.step_degree(step, pattern), self.scale.value())
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
            playheads: Arc::new(std::array::from_fn(|_| AtomicI32::new(-1))),
            held: [None; NUM_TRACKS],
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
        for t in 0..NUM_TRACKS {
            if let Some(h) = self.held[t].take() {
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
        self.held = [None; NUM_TRACKS];
        self.last_end_beat = None;
        for p in self.playheads.iter() {
            p.store(-1, Ordering::Relaxed);
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
                    p.store(-1, Ordering::Relaxed);
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
            let gate = step_beats * 0.5;
            let length = track.length.value() as i64;
            let pattern = track.current_pattern();
            let on = track.on.value();

            for (k, beat) in sequencer::block_steps(start_beat, beats_per_sample, num_samples, step_beats) {
                // Close the previous note at its gate end, or right here at the
                // latest (e.g. when the rate just got faster).
                if let Some(h) = self.held[t].take() {
                    self.push(timing(h.off_beat.min(beat)), false, h.note, h.channel, 0.0);
                }
                let step = k.rem_euclid(length) as usize;
                if on && pattern[step] {
                    let note = track.step_note(step, &pattern);
                    let velocity = track.step_velocity(step, &pattern) as f32 / 127.0;
                    let channel = (track.channel.value() - 1) as u8;
                    self.push(timing(beat), true, note, channel, velocity);
                    self.held[t] = Some(Held { note, channel, off_beat: beat + gate });
                }
            }

            // Gate ends inside this block with no new step after it.
            if let Some(h) = self.held[t] {
                if h.off_beat < end_beat || !on {
                    self.held[t] = None;
                    self.push(timing(h.off_beat), false, h.note, h.channel, 0.0);
                }
            }

            // Playhead for the GUI: the step sounding at the end of this block.
            let last = ((end_beat - beats_per_sample) / step_beats).floor() as i64;
            self.playheads[t].store(last.rem_euclid(length) as i32, Ordering::Relaxed);
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
