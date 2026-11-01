//! Pure, allocation-free sequencing logic: euclidean rhythms, scales, note naming.
//! Kept free of any plugin/GUI types so it can be unit tested and reused.

use nih_plug::prelude::Enum;

pub const MAX_STEPS: usize = 32;
/// Steps are 16th notes. The gate is half a step, so step starts and gate ends
/// both fall on a 1/32-note "tick" grid: even ticks start steps, odd ticks end gates.
pub const STEPS_PER_BEAT: f64 = 4.0;
pub const TICKS_PER_BEAT: f64 = STEPS_PER_BEAT * 2.0;

/// Every tick that falls inside a block, with its sample offset in the block.
/// A tick exactly on the block start belongs to this block; one exactly on the
/// end belongs to the next, so no tick is ever doubled or dropped.
pub fn block_ticks(
    start_beat: f64,
    beats_per_sample: f64,
    num_samples: usize,
) -> impl Iterator<Item = (i64, u32)> {
    let end_beat = start_beat + beats_per_sample * num_samples as f64;
    let first = (start_beat * TICKS_PER_BEAT).ceil() as i64;
    let end = (end_beat * TICKS_PER_BEAT).ceil() as i64;
    (first..end).map(move |tick| {
        let beat = tick as f64 / TICKS_PER_BEAT;
        let timing = (((beat - start_beat) / beats_per_sample).round() as i64)
            .clamp(0, num_samples as i64 - 1) as u32;
        (tick, timing)
    })
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
pub fn pattern(length: usize, euclid_steps: usize, pulses: usize) -> [bool; MAX_STEPS] {
    let e = euclid(euclid_steps, pulses);
    let cycle = euclid_steps.clamp(1, MAX_STEPS);
    let mut out = [false; MAX_STEPS];
    for (i, slot) in out.iter_mut().enumerate().take(length.min(MAX_STEPS)) {
        *slot = e[i % cycle];
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
}

impl Scale {
    pub const ALL: [Scale; 7] = [
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Mixolydian,
        Scale::HarmonicMinor,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
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
        pattern(len, steps, pulses)[..len]
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

    /// Process one bar at 120 BPM / 48 kHz in awkward block sizes and check every
    /// tick lands exactly once, on the right absolute sample.
    #[test]
    fn ticks_are_sample_accurate_across_blocks() {
        let sr = 48_000.0;
        let bps = 120.0 / 60.0 / sr; // beats per sample
        let samples_per_tick = (1.0 / TICKS_PER_BEAT / bps) as u64; // 3000
        for block in [1usize, 64, 333, 512, 3000, 4096] {
            let bar = 32 * samples_per_tick;
            let mut seen = Vec::new();
            let mut pos = 0u64;
            while pos < bar {
                let start = pos as f64 * bps;
                for (tick, t) in block_ticks(start, bps, block) {
                    seen.push((tick, pos + t as u64));
                }
                pos += block as u64;
            }
            seen.retain(|&(_, at)| at < bar); // last block may run past the bar
            let expected: Vec<_> = (0..seen.len() as i64)
                .map(|k| (k, k as u64 * samples_per_tick))
                .collect();
            assert_eq!(seen, expected, "block size {block}");
            assert_eq!(seen.len(), 32, "one bar = 16 steps = 32 ticks");
        }
    }

    #[test]
    fn scale_quantizing() {
        assert_eq!(scale_note(60, 0, Scale::Major), 60);
        assert_eq!(scale_note(60, 2, Scale::Major), 64);
        assert_eq!(scale_note(60, 7, Scale::Major), 72);
        assert_eq!(scale_note(60, -1, Scale::Major), 59);
        assert_eq!(scale_note(60, 5, Scale::MinorPentatonic), 72);
        assert_eq!(scale_note(126, 14, Scale::Major), 127);
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
