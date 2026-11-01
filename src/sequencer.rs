//! Pure, allocation-free sequencing logic: euclidean rhythms, scales, note naming.
//! Kept free of any plugin/GUI types so it can be unit tested and reused.

use nih_plug::prelude::Enum;

pub const MAX_STEPS: usize = 32;

/// How long one step lasts, per track.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    #[name = "1/64"]
    N64,
    #[name = "1/32"]
    N32,
    #[name = "1/16"]
    N16,
    #[name = "1/8"]
    N8,
    #[name = "1/4"]
    N4,
    #[name = "1/2"]
    N2,
    #[name = "1/1"]
    N1,
    #[name = "x2"]
    X2,
    #[name = "x4"]
    X4,
}

impl Rate {
    /// Step length in beats (quarter notes). x2 / x4 are two / four whole bars
    /// per step. Dotted steps last 1.5 times as long.
    pub fn step_beats(self, dotted: bool) -> f64 {
        let beats = match self {
            Rate::N64 => 1.0 / 16.0,
            Rate::N32 => 1.0 / 8.0,
            Rate::N16 => 0.25,
            Rate::N8 => 0.5,
            Rate::N4 => 1.0,
            Rate::N2 => 2.0,
            Rate::N1 => 4.0,
            Rate::X2 => 8.0,
            Rate::X4 => 16.0,
        };
        if dotted {
            beats * 1.5
        } else {
            beats
        }
    }
}

/// Sample offset of `beat` inside a block that starts at `start_beat`.
pub fn beat_to_timing(beat: f64, start_beat: f64, beats_per_sample: f64, num_samples: usize) -> u32 {
    (((beat - start_beat) / beats_per_sample).round() as i64).clamp(0, num_samples as i64 - 1) as u32
}

/// Every step boundary (multiples of `step_beats`, anchored to beat 0) that
/// falls inside a block, as (global step index, beat position). A boundary
/// exactly on the block start belongs to this block; one exactly on the end
/// belongs to the next, so no step is ever doubled or dropped.
pub fn block_steps(
    start_beat: f64,
    beats_per_sample: f64,
    num_samples: usize,
    step_beats: f64,
) -> impl Iterator<Item = (i64, f64)> {
    let end_beat = start_beat + beats_per_sample * num_samples as f64;
    let first = (start_beat / step_beats - 1e-9).ceil() as i64;
    let end = (end_beat / step_beats - 1e-9).ceil() as i64;
    (first..end).map(move |k| (k, k as f64 * step_beats))
}

/// Euclidean rhythm E(`pulses`, `steps`) using Bjorklund's algorithm, producing the
/// canonical patterns from Toussaint's paper (e.g. E(3,8) = x..x..x., E(5,8) = x.xx.xx.).
///
/// Allocation-free (groups are u32 bitmasks) so it is safe on the audio thread.
pub fn euclid(steps: usize, pulses: usize) -> [bool; MAX_STEPS] {
    let n = steps.min(MAX_STEPS);
    let k = pulses.min(n);
    let mut out = [false; MAX_STEPS];
    if k == 0 {
        return out;
    }

    // Each group is (bitmask, length). Start with k "1" groups and n-k "0" groups,
    // then repeatedly append the remainder groups onto the front groups.
    let mut a = [(0u32, 0usize); MAX_STEPS];
    let mut b = [(0u32, 0usize); MAX_STEPS];
    let (mut na, mut nb) = (k, n - k);
    for g in a.iter_mut().take(na) {
        *g = (1, 1);
    }
    for g in b.iter_mut().take(nb) {
        *g = (0, 1);
    }

    while nb > 1 {
        let m = na.min(nb);
        let mut new_a = [(0u32, 0usize); MAX_STEPS];
        for i in 0..m {
            let (ma, la) = a[i];
            let (mb, lb) = b[i];
            new_a[i] = (ma | (mb << la), la + lb);
        }
        let mut new_b = [(0u32, 0usize); MAX_STEPS];
        let new_nb = if na > m {
            new_b[..na - m].copy_from_slice(&a[m..na]);
            na - m
        } else {
            new_b[..nb - m].copy_from_slice(&b[m..nb]);
            nb - m
        };
        a = new_a;
        b = new_b;
        na = m;
        nb = new_nb;
    }

    let mut pos = 0;
    for &(mask, len) in a[..na].iter().chain(b[..nb].iter()) {
        for bit in 0..len {
            out[pos] = mask >> bit & 1 == 1;
            pos += 1;
        }
    }
    out
}

/// The playable pattern over the full sequence length. The euclidean pattern
/// repeats every `euclid_steps` and restarts when the sequence wraps at `length`.
/// `rotate` shifts the euclidean cycle right (positive) or left (negative),
/// wrapping around inside the cycle.
pub fn pattern(length: usize, euclid_steps: usize, pulses: usize, rotate: i32) -> [bool; MAX_STEPS] {
    let e = euclid(euclid_steps, pulses);
    let cycle = euclid_steps.clamp(1, MAX_STEPS) as i32;
    let mut out = [false; MAX_STEPS];
    for (i, slot) in out.iter_mut().enumerate().take(length.min(MAX_STEPS)) {
        let pos = (i as i32 % cycle - rotate).rem_euclid(cycle);
        *slot = e[pos as usize];
    }
    out
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    #[name = "Major"]
    Major,
    #[name = "Minor"]
    Minor,
    #[name = "Dorian"]
    Dorian,
    #[name = "Mixolydian"]
    Mixolydian,
    #[name = "Harmonic Minor"]
    HarmonicMinor,
    #[name = "Major Pentatonic"]
    MajorPentatonic,
    #[name = "Minor Pentatonic"]
    MinorPentatonic,
    #[name = "Chromatic"]
    Chromatic,
}

impl Scale {
    pub const ALL: [Scale; 8] = [
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Mixolydian,
        Scale::HarmonicMinor,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
        Scale::Chromatic,
    ];

    pub fn intervals(self) -> &'static [i32] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::Dorian => "Dorian",
            Scale::Mixolydian => "Mixolydian",
            Scale::HarmonicMinor => "Harmonic Minor",
            Scale::MajorPentatonic => "Major Pentatonic",
            Scale::MinorPentatonic => "Minor Pentatonic",
            Scale::Chromatic => "Chromatic",
        }
    }
}

/// MIDI note for `degree` scale steps above (or below) `root`, clamped to 0..=127.
pub fn scale_note(root: i32, degree: i32, scale: Scale) -> u8 {
    let iv = scale.intervals();
    let len = iv.len() as i32;
    let octave = degree.div_euclid(len);
    let idx = degree.rem_euclid(len) as usize;
    (root + octave * 12 + iv[idx]).clamp(0, 127) as u8
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RampMode {
    #[name = "Off"]
    Off,
    #[name = "Up"]
    Up,
    #[name = "Down"]
    Down,
    #[name = "Random"]
    Random,
    /// Up, up, down, repeating: climbs one scale step every three notes.
    #[name = "Climb"]
    Climb,
    /// Down, down, up, repeating.
    #[name = "Descend"]
    Descend,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RampRange {
    #[name = "1 Oct"]
    One,
    #[name = "2 Oct"]
    Two,
    #[name = "4 Oct"]
    Four,
}

impl RampRange {
    pub const ALL: [RampRange; 3] = [RampRange::One, RampRange::Two, RampRange::Four];

    pub fn octaves(self) -> i32 {
        match self {
            RampRange::One => 1,
            RampRange::Two => 2,
            RampRange::Four => 4,
        }
    }
}

/// Everything that shapes the per-step pitch ramp of one track.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    pub mode: RampMode,
    pub octaves: i32,
    pub bipolar: bool,
    pub seed: u32,
}

impl Ramp {
    /// Lowest and highest scale degree the ramp can reach. Working in scale
    /// degrees (not semitones) is what keeps every ramp note in key.
    pub fn bounds(&self, scale: Scale) -> (i32, i32) {
        let span = self.octaves * scale.intervals().len() as i32;
        if self.bipolar {
            (-span, span)
        } else {
            (0, span)
        }
    }

    /// Climb and Descend follow the notes that play rather than step
    /// positions, so the up-up-down figure is heard whatever the rhythm.
    pub fn per_note(&self) -> bool {
        matches!(self.mode, RampMode::Climb | RampMode::Descend)
    }

    /// Scale-degree offset for `step` in a sequence of `length` steps, where
    /// `note_index` is how many notes play before this step in the sequence.
    /// Up/Down sweep linearly across the whole sequence; Random picks a fixed
    /// degree per step from the seed, so the melody repeats until re-rolled;
    /// Climb/Descend move +1 +1 -1 (or -1 -1 +1) per note and wrap around
    /// inside the range.
    pub fn degree(&self, step: usize, note_index: usize, length: usize, scale: Scale) -> i32 {
        let (lo, hi) = self.bounds(scale);
        let t = if length > 1 { step as f64 / (length - 1) as f64 } else { 0.0 };
        let sweep = (t * (hi - lo) as f64).round() as i32;
        match self.mode {
            RampMode::Off => 0,
            RampMode::Up => lo + sweep,
            RampMode::Down => hi - sweep,
            RampMode::Random => {
                let r = splitmix64(((self.seed as u64) << 32) | step as u64);
                lo + (r % (hi - lo + 1) as u64) as i32
            }
            RampMode::Climb | RampMode::Descend => {
                let k = note_index as i32;
                let rise = (k / 3 + k % 3) % (hi - lo + 1);
                if self.mode == RampMode::Climb {
                    lo + rise
                } else {
                    hi - rise
                }
            }
        }
    }
}

/// Apply manual edits to a pattern: `added` steps always play, `removed` steps
/// never do. Bit `i` of each mask is step `i`.
pub fn apply_overrides(mut pattern: [bool; MAX_STEPS], added: u32, removed: u32) -> [bool; MAX_STEPS] {
    for (i, hit) in pattern.iter_mut().enumerate() {
        let bit = 1u32 << i;
        if added & bit != 0 {
            *hit = true;
        } else if removed & bit != 0 {
            *hit = false;
        }
    }
    pattern
}

/// New (added, removed) masks after toggling `step` whose current state is `is_hit`.
/// Toggling twice always returns to the original masks, so a manual edit can be
/// undone by repeating it.
pub fn toggle_override(added: u32, removed: u32, step: usize, is_hit: bool) -> (u32, u32) {
    let bit = 1u32 << step;
    match (is_hit, added & bit != 0, removed & bit != 0) {
        (true, true, _) => (added & !bit, removed),  // undo a manual add
        (true, false, _) => (added, removed | bit),  // remove a euclidean hit
        (false, _, true) => (added, removed & !bit), // undo a manual removal
        (false, _, false) => (added | bit, removed), // add a note on a rest
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityMode {
    #[name = "Off"]
    Off,
    #[name = "Up"]
    Up,
    #[name = "Down"]
    Down,
}

/// Velocity for the `note_index`-th note counted from the start of the song
/// (it keeps counting across sequence loops, so a ramp longer than the
/// pattern drifts against it instead of restarting every loop).
///
/// The ramp runs from `low` to `high` (Up) or `high` to `low` (Down) over
/// `length` notes, then starts again. A length of 1 (or mode Off) gives the
/// fixed `base`.
pub fn ramp_velocity(base: i32, mode: VelocityMode, low: i32, high: i32, length: usize, note_index: i64) -> i32 {
    if mode == VelocityMode::Off || length <= 1 {
        return base;
    }
    let t = note_index.rem_euclid(length as i64) as f64 / (length - 1) as f64;
    let (from, to) = match mode {
        VelocityMode::Up => (low, high),
        _ => (high, low),
    };
    from + (t * (to - from) as f64).round() as i32
}

/// How many notes have played since the song start (beat 0) before global step
/// `global_step`, for a sequence of `length` steps playing `pattern`.
pub fn song_note_index(global_step: i64, length: usize, pattern: &[bool; MAX_STEPS]) -> i64 {
    let len = length.clamp(1, MAX_STEPS);
    let per_loop = pattern[..len].iter().filter(|&&h| h).count() as i64;
    let loops = global_step.div_euclid(len as i64);
    let step = global_step.rem_euclid(len as i64) as usize;
    loops * per_loop + pattern[..step].iter().filter(|&&h| h).count() as i64
}

/// Small, fast, well-mixed hash; deterministic across platforms.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Note name using the Ableton/Bitwig convention where MIDI 60 = C3.
pub fn note_name(note: i32) -> String {
    format!("{}{}", NOTE_NAMES[note.rem_euclid(12) as usize], note.div_euclid(12) - 2)
}

pub fn parse_note_name(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    let upper = s.to_ascii_uppercase();
    // Longest names first so "C#" wins over "C".
    let mut names: Vec<(usize, &str)> = NOTE_NAMES.iter().copied().enumerate().collect();
    names.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    for (pc, name) in names {
        if let Some(rest) = upper.strip_prefix(name) {
            let octave: i32 = rest.trim().parse().ok()?;
            return Some((octave + 2) * 12 + pc as i32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(len: usize, steps: usize, pulses: usize) -> String {
        rotated(len, steps, pulses, 0)
    }

    fn rotated(len: usize, steps: usize, pulses: usize, rot: i32) -> String {
        pattern(len, steps, pulses, rot)[..len]
            .iter()
            .map(|&b| if b { 'x' } else { '.' })
            .collect()
    }

    #[test]
    fn classic_euclidean_rhythms() {
        assert_eq!(bits(8, 8, 3), "x..x..x."); // tresillo
        assert_eq!(bits(16, 16, 4), "x...x...x...x...");
        assert_eq!(bits(5, 5, 2), "x.x..");
        assert_eq!(bits(8, 8, 5), "x.xx.xx.");
        assert_eq!(bits(4, 4, 0), "....");
        assert_eq!(bits(4, 4, 9), "xxxx"); // pulses clamp to steps
    }

    #[test]
    fn euclid_repeats_within_length() {
        assert_eq!(bits(8, 3, 1), "x..x..x.");
    }

    #[test]
    fn rotation_shifts_within_the_cycle() {
        assert_eq!(rotated(8, 8, 3, 1), ".x..x..x"); // right
        assert_eq!(rotated(8, 8, 3, -1), "..x..x.x"); // left
        assert_eq!(rotated(8, 8, 3, 8), "x..x..x."); // full turn
        assert_eq!(rotated(8, 8, 3, -9), rotated(8, 8, 3, -1));
        // In a polymeter the shift stays inside each euclidean cycle.
        assert_eq!(rotated(8, 3, 1, 1), ".x..x..x");
        // Rotation never changes the number of hits.
        for r in -31..=31 {
            assert_eq!(rotated(16, 16, 5, r).matches('x').count(), 5);
        }
    }

    #[test]
    fn matches_bjorklund_reference_for_all_patterns() {
        for line in include_str!("bjorklund_reference.txt").lines() {
            let mut it = line.split(' ');
            let k: usize = it.next().unwrap().parse().unwrap();
            let n: usize = it.next().unwrap().parse().unwrap();
            assert_eq!(bits(n, n, k), it.next().unwrap(), "E({k},{n})");
        }
    }

    #[test]
    fn every_pulse_count_is_exact() {
        for n in 1..=32 {
            for k in 0..=n {
                assert_eq!(bits(n, n, k).matches('x').count(), k, "E({k},{n})");
            }
        }
    }

    /// Process four bars at 120 BPM / 48 kHz in awkward block sizes, for every
    /// rate (straight and dotted), and check each step lands exactly once on
    /// the right absolute sample.
    #[test]
    fn steps_are_sample_accurate_across_blocks_and_rates() {
        let sr = 48_000.0;
        let bps = 120.0 / 60.0 / sr; // beats per sample
        let span = 16.0; // beats
        let total = (span / bps) as u64;
        for rate in [Rate::N64, Rate::N32, Rate::N16, Rate::N8, Rate::N4, Rate::N2, Rate::N1, Rate::X2, Rate::X4] {
            for dotted in [false, true] {
                let step = rate.step_beats(dotted);
                for block in [1usize, 64, 333, 512, 4096] {
                    let mut seen = Vec::new();
                    let mut pos = 0u64;
                    while pos < total {
                        let start = pos as f64 * bps;
                        for (k, beat) in block_steps(start, bps, block, step) {
                            seen.push((k, pos + beat_to_timing(beat, start, bps, block) as u64));
                        }
                        pos += block as u64;
                    }
                    seen.retain(|&(_, at)| at < total);
                    let expected: Vec<_> = (0..(span / step - 1e-9).ceil() as i64)
                        .map(|k| (k, (k as f64 * step / bps).round() as u64))
                        .collect();
                    assert_eq!(seen, expected, "{rate:?} dotted={dotted} block={block}");
                }
            }
        }
    }

    #[test]
    fn rates() {
        assert_eq!(Rate::N16.step_beats(false), 0.25);
        assert_eq!(Rate::N8.step_beats(true), 0.75);
        assert_eq!(Rate::X4.step_beats(false), 16.0);
    }

    #[test]
    fn climb_and_descend() {
        let climb = ramp(RampMode::Climb, 2, false); // degrees 0..=14
        let up: Vec<_> = (0..9).map(|n| climb.degree(0, n, 16, Scale::Major)).collect();
        assert_eq!(up, vec![0, 1, 2, 1, 2, 3, 2, 3, 4]);
        let descend = ramp(RampMode::Descend, 2, false);
        let down: Vec<_> = (0..6).map(|n| descend.degree(0, n, 16, Scale::Major)).collect();
        assert_eq!(down, vec![14, 13, 12, 13, 12, 11]);
        // Wraps inside the range instead of running away.
        let small = ramp(RampMode::Climb, 1, true); // -5..=5 in pentatonic
        assert!((0..200).all(|n| (-5..=5).contains(&small.degree(0, n, 16, Scale::MinorPentatonic))));
        assert!(climb.per_note() && !ramp(RampMode::Up, 1, false).per_note());
    }

    #[test]
    fn scale_quantizing() {
        assert_eq!(scale_note(60, 0, Scale::Major), 60);
        assert_eq!(scale_note(60, 2, Scale::Major), 64);
        assert_eq!(scale_note(60, 7, Scale::Major), 72);
        assert_eq!(scale_note(60, -1, Scale::Major), 59);
        assert_eq!(scale_note(60, 5, Scale::MinorPentatonic), 72);
        assert_eq!(scale_note(126, 14, Scale::Major), 127);
        // Chromatic: one degree = one semitone.
        assert_eq!(scale_note(60, 1, Scale::Chromatic), 61);
        assert_eq!(scale_note(60, -13, Scale::Chromatic), 47);
        assert_eq!(Ramp { mode: RampMode::Up, octaves: 2, bipolar: false, seed: 0 }.bounds(Scale::Chromatic), (0, 24));
    }

    fn ramp(mode: RampMode, octaves: i32, bipolar: bool) -> Ramp {
        Ramp { mode, octaves, bipolar, seed: 1 }
    }

    fn degrees(r: Ramp, len: usize, scale: Scale) -> Vec<i32> {
        (0..len).map(|i| r.degree(i, i, len, scale)).collect()
    }

    #[test]
    fn ramp_up_and_down() {
        let up = ramp(RampMode::Up, 1, false);
        assert_eq!(degrees(up, 8, Scale::Major), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        let down = ramp(RampMode::Down, 1, false);
        assert_eq!(degrees(down, 8, Scale::Major), vec![7, 6, 5, 4, 3, 2, 1, 0]);
        // Bipolar spans -range..+range; pentatonic has 5 degrees per octave.
        let bi = ramp(RampMode::Up, 1, true);
        assert_eq!(degrees(bi, 11, Scale::MinorPentatonic), (-5..=5).collect::<Vec<_>>());
        // Off never moves; a single-step sequence stays on the start value.
        assert_eq!(degrees(ramp(RampMode::Off, 4, true), 16, Scale::Major), vec![0; 16]);
        assert_eq!(degrees(up, 1, Scale::Major), vec![0]);
    }

    #[test]
    fn ramp_endpoints_hit_the_range() {
        for oct in [1, 2, 4] {
            for bip in [false, true] {
                let r = ramp(RampMode::Up, oct, bip);
                let (lo, hi) = r.bounds(Scale::Dorian);
                let d = degrees(r, 16, Scale::Dorian);
                assert_eq!((d[0], d[15]), (lo, hi));
                assert_eq!(hi, oct * 7);
                // Notes land exactly `oct` octaves above the root.
                assert_eq!(scale_note(48, hi, Scale::Dorian) as i32, 48 + 12 * oct);
            }
        }
    }

    #[test]
    fn ramp_random_is_bounded_deterministic_and_reseedable() {
        let r = ramp(RampMode::Random, 2, true);
        let a = degrees(r, 32, Scale::Minor);
        assert!(a.iter().all(|&d| (-14..=14).contains(&d)));
        assert_eq!(a, degrees(r, 32, Scale::Minor));
        let b = degrees(Ramp { seed: 2, ..r }, 32, Scale::Minor);
        assert_ne!(a, b);
        // Not degenerate: uses a good part of the range.
        let distinct: std::collections::HashSet<_> = a.iter().collect();
        assert!(distinct.len() >= 10, "{a:?}");
    }

    #[test]
    fn velocity_ramp() {
        let v = |mode, len, i| ramp_velocity(75, mode, 40, 100, len, i);
        let up: Vec<_> = (0..8).map(|i| v(VelocityMode::Up, 4, i)).collect();
        assert_eq!(up, vec![40, 60, 80, 100, 40, 60, 80, 100]);
        let down: Vec<_> = (0..4).map(|i| v(VelocityMode::Down, 4, i)).collect();
        assert_eq!(down, vec![100, 80, 60, 40]);
        // Length 2 alternates between the two values.
        let alt: Vec<_> = (0..4).map(|i| v(VelocityMode::Up, 2, i)).collect();
        assert_eq!(alt, vec![40, 100, 40, 100]);
        // Length 1 or Off: fixed at the base velocity.
        assert_eq!(v(VelocityMode::Up, 1, 3), 75);
        assert_eq!(v(VelocityMode::Off, 4, 3), 75);
    }

    #[test]
    fn velocity_ramp_longer_than_pattern_drifts_across_loops() {
        // 3 notes per loop (x..x..x.), ramp over 7 notes: the ramp keeps going
        // into the next loops instead of restarting, and lines up again after
        // 7 loops' worth of notes.
        let p = pattern(8, 8, 3, 0);
        let vels: Vec<_> = (0..24i64)
            .filter(|&k| p[k.rem_euclid(8) as usize])
            .map(|k| ramp_velocity(75, VelocityMode::Up, 40, 100, 7, song_note_index(k, 8, &p)))
            .collect();
        assert_eq!(vels, vec![40, 50, 60, 70, 80, 90, 100, 40, 50]);
        // Notes before the song start (pre-roll) count backwards consistently.
        assert_eq!(song_note_index(-8, 8, &p), -3);
        assert_eq!(song_note_index(3, 8, &p), 1);
    }

    #[test]
    fn manual_overrides() {
        let base = pattern(8, 8, 3, 0); // x..x..x.
        let show = |p: [bool; MAX_STEPS]| -> String { p[..8].iter().map(|&b| if b { 'x' } else { '.' }).collect() };

        // Add step 1, remove step 3.
        let (a, r) = toggle_override(0, 0, 1, base[1]);
        let (a, r) = toggle_override(a, r, 3, base[3]);
        let edited = apply_overrides(base, a, r);
        assert_eq!(show(edited), "xx....x.");

        // Toggling the same steps again restores the original masks.
        let (a, r) = toggle_override(a, r, 1, edited[1]);
        let (a, r) = toggle_override(a, r, 3, edited[3]);
        assert_eq!((a, r), (0, 0));
        assert_eq!(show(apply_overrides(base, a, r)), "x..x..x.");
    }

    #[test]
    fn note_names_roundtrip() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(61), "C#3");
        for n in 0..128 {
            assert_eq!(parse_note_name(&note_name(n)), Some(n));
        }
    }
}
