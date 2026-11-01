# Euclid Seq

![Euclid Seq](screenshot.png)

A minimal euclidean MIDI step sequencer, built in Rust with [nih-plug](https://github.com/robbert-vdh/nih-plug). Exports **CLAP** (note effect) and **VST3**.

## Controls

| Control | Range | What it does |
|---|---|---|
| Steps | 1–32 | Sequence length. The grid loops after this step. You can also click a grid cell to end the sequence there. |
| E. Steps | 1–32 | Length of the euclidean cycle. It repeats inside the sequence and restarts when the sequence wraps. |
| Pulses | 0–32 | Hits spread evenly over the euclidean cycle (Bjorklund's algorithm, clamped to E. Steps). |
| Pitch | −14…+14 | Transposition in **scale degrees**, so the sequence always stays in key (±2 octaves in 7‑note scales). |
| Root | C0–C6 | Root note (MIDI 60 = C3, the Ableton/Bitwig convention). Type a value like `F#2` or `54`. |
| Scale | 7 scales | Major, Minor, Dorian, Mixolydian, Harmonic Minor, Major Pentatonic, Minor Pentatonic. |

Knobs: drag vertically (Shift = fine), scroll for single steps, double‑click to reset.

Timing: steps are 16th notes locked to the host transport (sample‑accurate), gate is 50 %, velocity 100, MIDI channel 1. The pattern is anchored to the song position, so it stays in sync when you jump around or loop.

Grid: orange = hit, white = hit under the playhead, small dot = start of each euclidean cycle, dim cells = beyond the sequence length.

## Build

Requires Rust (stable). On Linux also: `libx11-xcb-dev libxcursor-dev libgl1-mesa-dev libxcb-icccm4-dev libxcb-dri2-0-dev libxkbcommon-dev`.

```sh
cargo xtask bundle euclid_seq --release
```

Bundles land in `target/bundled/`. Copy them to your plugin folder:

| OS | CLAP | VST3 |
|---|---|---|
| macOS | `~/Library/Audio/Plug-Ins/CLAP/` | `~/Library/Audio/Plug-Ins/VST3/` |
| Windows | `C:\Program Files\Common Files\CLAP\` | `C:\Program Files\Common Files\VST3\` |
| Linux | `~/.clap/` | `~/.vst3/` |

On macOS, build a universal binary with `cargo xtask bundle-universal euclid_seq --release`. Unsigned plugins may need `xattr -cr <bundle>` before the host will load them.

GUI without a DAW (dev only):

```sh
cargo run --release --features standalone -- --backend dummy --tempo 120
```

Tests (euclidean patterns checked against all 560 Bjorklund patterns up to 32 steps, scale math, sample‑accurate tick timing across block sizes):

```sh
cargo test --lib
```

## Using it in a DAW

- **Bitwig**: drop it on an instrument track *before* the instrument. It's a CLAP note effect, so notes flow straight into the next device.
- **Ableton Live**: Live doesn't load CLAP, so use the **VST3** build. Put the sequencer on its own MIDI track, then on the instrument track set "MIDI From" to that track and pick *Euclid Seq* in the second dropdown.

## Code layout

- `src/sequencer.rs` – pure logic: Bjorklund, scales, note names, block tick timing (unit tested, allocation‑free)
- `src/lib.rs` – plugin, parameters, MIDI output in `process()`
- `src/editor.rs` – egui GUI: step grid, custom knobs, scale dropdown

## Known notes

- `clap-validator` 0.4.1 reports failures in the `state-reproducibility-*` and `state-invalid-random` tests. nih-plug's own reference `gain` example fails the same four tests with the same validator, so this is a framework/validator interaction rather than something in this plugin; all processing, parameter and transport tests pass.
- nih-plug's VST3 export uses GPLv3 bindings, so VST3 builds you distribute must be GPLv3. CLAP has no such restriction.
