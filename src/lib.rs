use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod sequencer;

use sequencer::{note_name, parse_note_name, pattern, scale_note, Scale};

use sequencer::STEPS_PER_BEAT;
const VELOCITY: f32 = 100.0 / 127.0;
const CHANNEL: u8 = 0;

pub struct EuclidSeq {
    params: Arc<EuclidSeqParams>,
    /// Step currently under the playhead (-1 when stopped), read by the GUI.
    playhead: Arc<AtomicI32>,
    /// The note currently held, so it can always be released correctly even if
    /// pitch/root/scale changed while it was sounding.
    held_note: Option<u8>,
}

#[derive(Params)]
pub struct EuclidSeqParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    /// Sequence length: the grid loops after this many steps.
    #[id = "length"]
    pub length: IntParam,

    /// Length of the euclidean cycle. It repeats inside the sequence length.
    #[id = "e_steps"]
    pub euclid_steps: IntParam,

    /// Number of hits spread evenly over the euclidean cycle.
    #[id = "e_pulses"]
    pub euclid_pulses: IntParam,

    /// Transposition in scale degrees, so the sequence always stays in key.
    #[id = "pitch"]
    pub pitch: IntParam,

    #[id = "scale"]
    pub scale: EnumParam<Scale>,

    /// Root note as a MIDI note number (60 = C3).
    #[id = "root"]
    pub root: IntParam,
}

impl Default for EuclidSeqParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),
            length: IntParam::new("Steps", 16, IntRange::Linear { min: 1, max: 32 }),
            euclid_steps: IntParam::new("Euclid Steps", 16, IntRange::Linear { min: 1, max: 32 }),
            euclid_pulses: IntParam::new("Euclid Pulses", 5, IntRange::Linear { min: 0, max: 32 }),
            pitch: IntParam::new("Pitch", 0, IntRange::Linear { min: -14, max: 14 })
                .with_value_to_string(Arc::new(|v| format!("{v:+}"))),
            scale: EnumParam::new("Scale", Scale::Minor),
            root: IntParam::new("Root", 60, IntRange::Linear { min: 24, max: 96 })
                .with_value_to_string(Arc::new(|v| note_name(v)))
                .with_string_to_value(Arc::new(|s| parse_note_name(s))),
        }
    }
}

impl EuclidSeqParams {
    /// Pulses can't exceed the euclidean cycle length.
    pub fn effective_pulses(&self) -> usize {
        self.euclid_pulses.value().min(self.euclid_steps.value()) as usize
    }

    pub fn current_pattern(&self) -> [bool; sequencer::MAX_STEPS] {
        pattern(
            self.length.value() as usize,
            self.euclid_steps.value() as usize,
            self.effective_pulses(),
        )
    }

    pub fn current_note(&self) -> u8 {
        scale_note(self.root.value(), self.pitch.value(), self.scale.value())
    }
}

impl Default for EuclidSeq {
    fn default() -> Self {
        Self {
            params: Arc::new(EuclidSeqParams::default()),
            playhead: Arc::new(AtomicI32::new(-1)),
            held_note: None,
        }
    }
}

impl EuclidSeq {
    fn release(&mut self, context: &mut impl ProcessContext<Self>, timing: u32) {
        if let Some(note) = self.held_note.take() {
            context.send_event(NoteEvent::NoteOff {
                timing,
                voice_id: None,
                channel: CHANNEL,
                note,
                velocity: 0.0,
            });
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
        editor::create(self.params.clone(), self.playhead.clone())
    }

    fn reset(&mut self) {
        // The host will have flushed notes; just forget ours.
        self.held_note = None;
        self.playhead.store(-1, Ordering::Relaxed);
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
                self.release(context, 0);
                self.playhead.store(-1, Ordering::Relaxed);
                return ProcessStatus::Normal;
            }
        };

        let beats_per_sample = tempo / 60.0 / sample_rate;
        let end_beat = start_beat + beats_per_sample * num_samples as f64;

        let length = self.params.length.value() as i64;
        let pattern = self.params.current_pattern();
        let note = self.params.current_note();

        // Even ticks start a step; odd ticks end the gate of the previous one.
        for (tick, timing) in sequencer::block_ticks(start_beat, beats_per_sample, num_samples) {
            // Always close the previous note before anything else happens.
            self.release(context, timing);

            if tick.rem_euclid(2) == 0 {
                let step = (tick / 2).rem_euclid(length) as usize;
                if pattern[step] {
                    context.send_event(NoteEvent::NoteOn {
                        timing,
                        voice_id: None,
                        channel: CHANNEL,
                        note,
                        velocity: VELOCITY,
                    });
                    self.held_note = Some(note);
                }
            }
        }

        // Playhead for the GUI: the step sounding at the end of this block.
        let last_beat = end_beat - beats_per_sample;
        let step_now = ((last_beat * STEPS_PER_BEAT).floor() as i64).rem_euclid(length);
        self.playhead.store(step_now as i32, Ordering::Relaxed);

        ProcessStatus::Normal
    }
}

impl ClapPlugin for EuclidSeq {
    const CLAP_ID: &'static str = "com.thomasverney.euclid-seq";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Minimal euclidean MIDI step sequencer");
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
