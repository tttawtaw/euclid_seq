# Euclid Seq

![Euclid Seq](screenshot.png)

A minimal 4-track euclidean MIDI step sequencer, built in Rust with [nih-plug](https://github.com/robbert-vdh/nih-plug). Exports **CLAP** (note effect) and **VST3**.

## Tracks

Four independent tracks, one per page. Click the **1–4** tabs to switch. A tab's dot lights up when its track fires a note, and dims when the track is off. Every control below is per track, except the **Global** panel.

The controls sit in three panels:
- **Rhythm + Timing + Velocity:** when notes play, for how long, and how hard.
- **Pitch + Pitch Ramp:** which notes.
- **Global:** settings shared by all tracks.

- **ON / OFF** – only track 1 is on by default.
- **CH** – MIDI channel (defaults: track 1 → ch 1, track 2 → ch 2, …). Drag or scroll.
- **CLEAR** – removes every note from the track (Pulses to 0, manual edits cleared).
- **RESET** – returns all of the track's settings to their defaults and clears manual edits. It keeps ON and CH, so the track isn't silenced or rerouted.

## Controls

**Rhythm**

| Control | Range | What it does |
|---|---|---|
| Steps | 1–32 | Sequence length. The grid loops after this step. You can also click a grid cell to end the sequence there. |
| Link (chain icon) | on / off | Ties Steps and E. Steps together: turning either one moves both. Unlinking keeps the shared value. |
| E. Steps | 1–32 | Length of the euclidean cycle. It repeats inside the sequence and restarts when the sequence wraps. |
| Pulses | 0–32 | Hits spread evenly over the euclidean cycle (Bjorklund's algorithm, clamped to E. Steps). |
| Rotate | −16…+16 | Shifts the hits right (+) or left (−), wrapping inside the euclidean cycle. |
| Prob | 0–100 % | Chance that each note plays. Every pass rolls again, but the same spot in the song always gives the same result, so playback and exports are repeatable. |

**Timing**

| Control | Range | What it does |
|---|---|---|
| Rate | 1/64 … 1/1, x2, x4 | How long each step lasts. 1/16 is the default; x2 and x4 mean one step every 2 or 4 bars. |
| Dot | on / off | Dotted rate: each step lasts 1.5× as long (shown as e.g. `1/8.`). |
| Direction | FWD / REV / P-P | Forward, Reverse, or Ping-Pong (1 2 3 4 3 2 1 2 …, the end steps aren't repeated). Only the playhead moves differently: each step keeps the pitch shown on it. |
| Gate | 0–200 % or 10 ms–20 s | How long each note lasts, set with the **% / ms** switch. In % it's a share of the step (50 % = half a step; over 100 % notes overlap). In ms it's a fixed time whatever the tempo. 0 % plays a 1 ms blip rather than nothing. |

Long gates let notes overlap, up to 32 per track; past that, the oldest is released. If a note repeats a pitch that's still ringing, the old one ends just before the new one starts, so the synth doesn't choke it.

**Pitch**

| Control | Range | What it does |
|---|---|---|
| Pitch | −14…+14 | Transposition in **scale degrees**, so the sequence always stays in key. |
| Root | C0–C6 | Root note (MIDI 60 = C3, the Ableton/Bitwig convention). Type a value like `F#2` or `54`. |
| Scale | 8 scales | Major, Minor, Dorian, Mixolydian, Harmonic Minor, Major Pentatonic, Minor Pentatonic, Chromatic. |

**Pitch ramp** – gives each step its own pitch, always locked to the scale.

| Control | Options | What it does |
|---|---|---|
| Mode | Off / Up / Down / Random / Climb / Descend | Up and Down sweep across the whole sequence; Random gives every step a fixed random note. Climb goes up, up, down, repeating (Descend: down, down, up), and wraps back around at the edge of the range. |
| Range | 1 / 2 / 4 oct | How far the ramp reaches. |
| Polarity | + / ± | `+` goes from the root upwards; `±` spans below and above the root. |
| Dice | – | (Random only) re-rolls the random melody. The melody is saved with your project. |

Up, Down and Random are tied to step positions, not to hits. So Rotate, Pulses and E. Steps change *which* notes of the contour get played, which makes them a quick way to vary a melody. Climb and Descend follow the notes that actually play, so you always hear the up-up-down figure, and they restart at the top of each loop. With a ramp active, each grid cell shows its pitch as a short line (higher line = higher note), and hovering a cell shows its note name.

**Velocity**

| Control | Range | What it does |
|---|---|---|
| Velocity | 0–127 (default 75) | Fixed velocity, used when the ramp is off or its length is 1. 0 is silent (MIDI treats it as a note-off). |
| Mode | Off / Up / Down | Ramp from Low to High (Up) or High to Low (Down). |
| Low / High | 0–127 | The two values the ramp moves between. |
| Length | 1–16 | Notes per ramp before it starts again. 2 alternates Low/High; 1 = fixed Velocity. Not tied to the pattern: it can be longer than the number of notes. |

The velocity ramp counts *notes* in the order they're played (so Reverse and Ping-Pong are taken into account), not steps, and keeps counting across sequence loops. When Length differs from the number of notes in the pattern, the velocities drift against the rhythm and only line up again after several loops. That's deliberate, and it keeps a repeating pattern moving. It's worked out from the song position, so the same spot in your song always gets the same velocities. Each hit shows its velocity for the loop currently playing as a bar along the bottom of its cell.

**Editing steps by hand**

- **Cmd+click** a step to add a note, or Cmd+click a note to remove it. **Ctrl+click** and **right-click** do the same, which helps in hosts that keep the Cmd key for themselves.
- Repeat the click to undo an edit.
- Added notes get a corner notch; removed ones keep an outline.
- Edits stick to their step position and are saved with your project.
- A plain click on a step still sets the sequence length.

**Global**

| Control | What it does |
|---|---|
| Root | The key (C … B). Changing it moves every track to that key, **keeping each track's octave**, so a bass track stays low. |
| Scale | Changing it sets every track to that scale. |
| Following | Lit for each track that still matches the global key and scale. |

| CLEAR ALL | Removes every note from all 4 tracks. |
| RESET ALL | Returns the whole plugin to its factory settings: every track (including ON switches and MIDI channels), the global key and scale, and all hand-placed notes. |

CLEAR ALL and RESET ALL ask for a second click ("SURE?") within 3 seconds, because hand-placed notes can't be brought back with your DAW's undo.

The global controls pass their value to all tracks when you change them. After that, changing a track's own Root or Scale only affects that track. They aren't automatable, because that hand-off happens in the plugin window.

**Knobs**

- Drag vertically (Shift = fine), scroll for single steps, double‑click to reset.
- **Cmd+click** (or Ctrl+click) any numeric knob to type a value: Enter confirms, Esc cancels. Root accepts note names like `F#2`.

Timing: each track runs at its own Rate and Gate, locked to the host transport (sample‑accurate). Patterns are anchored to the song position, so they stay in sync when you jump around or loop.

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

Tests (euclidean patterns checked against all 560 Bjorklund patterns up to 32 steps, rotation, playback directions, probability, gate lengths, pitch and velocity ramps (incl. Climb/Descend), manual edits, scale math, sample‑accurate step timing for every rate (straight and dotted) across block sizes):

```sh
cargo test --lib
```

## Using it in a DAW

- **Bitwig**: drop it on an instrument track *before* the instrument. It's a CLAP note effect, so notes flow straight into the next device.
- **Ableton Live**: Live doesn't load CLAP, so use the **VST3** build. Put the sequencer on its own MIDI track, then on the instrument track set "MIDI From" to that track and pick *Euclid Seq* in the second dropdown.
- **Several tracks on:** all tracks leave the plugin together, each on its own MIDI channel. A single synth will usually play all of them. To give each track its own instrument, filter by MIDI channel on the receiving side, or use a multi-timbral instrument.

## Code layout

- `src/sequencer.rs` – pure logic: Bjorklund, rotation, scales, ramps, note names, block tick timing (unit tested, allocation‑free)
- `src/lib.rs` – plugin, per-track parameters, MIDI output in `process()`
- `src/editor.rs` – egui GUI: track pages, step grid (incl. manual edits), panels, knobs with type-in, link button, ramps, global section, Clear/Reset (per track and all)

## Known notes

- The window size is no longer saved in projects. Earlier versions saved it, which could make projects from an older version open the window too small for newer controls.
- v0.2 changed every parameter ID (they now carry a `_1`…`_4` track suffix), so projects saved with v0.1 open with default settings.

- `clap-validator` 0.4.1 reports failures in the `state-reproducibility-*` and `state-invalid-random` tests. nih-plug's own reference `gain` example fails the same four tests with the same validator, so this is a framework/validator interaction rather than something in this plugin; all processing, parameter and transport tests pass.
- nih-plug's VST3 export uses GPLv3 bindings, so VST3 builds you distribute must be GPLv3. CLAP has no such restriction.
