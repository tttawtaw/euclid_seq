use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod sequencer;

use sequencer::{
    note_name, parse_note_name, pattern, scale_note, Ramp, RampMode, RampRange, Scale, MAX_STEPS,
    STEPS_PER_BEAT,
};

pub const NUM_TRACKS: usize = 4;
const VELOCITY: f32 = 100.0 / 127.0;

pub struct EuclidSeq {
    params: Arc<EuclidSeqParams>,
    /// Step under the playhead for each track (-1 when stopped), read by the GUI.
    playheads: Arc<[AtomicI32; NUM_TRACKS]>,
    /// The (note, channel) each track is holding, so it is always released with
    /// exactly what was sent, even if pitch or channel changed meanwhile.
    held: [Option<(u8, u8)>; NUM_TRACKS],
}

#[derive(Params)]
pub struct EuclidSeqParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

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

    pub fn current_pattern(&self) -> [bool; MAX_STEPS] {
        pattern(
            self.length.value() as usize,
            self.effective_euclid_steps(),
            self.effective_pulses(),
            self.rotate.value(),
        )
    }

    pub fn ramp(&self) -> Ramp {
        Ramp {
            mode: self.ramp_mode.value(),
            octaves: self.ramp_range.value().octaves(),
            bipolar: self.ramp_bipolar.value(),
            seed: self.ramp_seed.value() as u32,
        }
    }

    /// Scale degree (relative to the root) played on `step`.
    pub fn step_degree(&self, step: usize) -> i32 {
        let ramp = self.ramp();
        let len = self.length.value() as usize;
        self.pitch.value() + ramp.degree(step, len, self.scale.value())
    }

    pub fn step_note(&self, step: usize) -> u8 {
        scale_note(self.root.value(), self.step_degree(step), self.scale.value())
    }
}

impl Default for EuclidSeqParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),
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
        }
    }
}

impl EuclidSeq {
    fn release(&mut self, track: usize, context: &mut impl ProcessContext<Self>, timing: u32) {
        if let Some((note, channel)) = self.held[track].take() {
            context.send_event(NoteEvent::NoteOff {
                timing,
                voice_id: None,
                channel,
                note,
                velocity: 0.0,
            });
        }
    }

    fn release_all(&mut self, context: &mut impl ProcessContext<Self>, timing: u32) {
        for t in 0..NUM_TRACKS {
            self.release(t, context, timing);
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
                self.release_all(context, 0);
                for p in self.playheads.iter() {
                    p.store(-1, Ordering::Relaxed);
                }
                return ProcessStatus::Normal;
            }
        };

        let beats_per_sample = tempo / 60.0 / sample_rate;
        let end_beat = start_beat + beats_per_sample * num_samples as f64;
        let params = self.params.clone();

        // Even ticks start a step; odd ticks end the gate of the previous one.
        // All tracks share the tick grid, so events stay in time order.
        for (tick, timing) in sequencer::block_ticks(start_beat, beats_per_sample, num_samples) {
            for (t, track) in params.tracks.iter().enumerate() {
                // Always close the previous note before anything else happens.
                self.release(t, context, timing);

                if tick.rem_euclid(2) != 0 || !track.on.value() {
                    continue;
                }
                let length = track.length.value() as i64;
                let step = (tick / 2).rem_euclid(length) as usize;
                if track.current_pattern()[step] {
                    let note = track.step_note(step);
                    let channel = (track.channel.value() - 1) as u8;
                    context.send_event(NoteEvent::NoteOn {
                        timing,
                        voice_id: None,
                        channel,
                        note,
                        velocity: VELOCITY,
                    });
                    self.held[t] = Some((note, channel));
                }
            }
        }

        // Playheads for the GUI: the step sounding at the end of this block.
        let last_step = ((end_beat - beats_per_sample) * STEPS_PER_BEAT).floor() as i64;
        for (t, track) in params.tracks.iter().enumerate() {
            let step = last_step.rem_euclid(track.length.value() as i64);
            self.playheads[t].store(step as i32, Ordering::Relaxed);
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
