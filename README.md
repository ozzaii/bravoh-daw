# bravo-daw

[![CI](https://github.com/ozzaii/bravo-daw/actions/workflows/ci.yml/badge.svg)](https://github.com/ozzaii/bravo-daw/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Parse DAW project files in pure Rust — no DAW installation required.**

One library, four formats, one unified schema:

| Format | Extension | Under the hood |
|---|---|---|
| **Ableton Live** | `.als` | gzipped XML |
| **FL Studio** | `.flp` | binary event stream |
| **Logic Pro** | `.logicx` | bundle + ProjectData binary |
| **REAPER** | `.rpp` | plain-text chunk format |

Every parser returns the same `ParsedIntelligence` struct, so downstream code never cares which DAW a project came from.

## Why

Project files are where the real story of a track lives — tempo, arrangement, plugin chains, sample choices, routing. But each DAW buries that story in a different proprietary format, and the existing tooling is scattered across single-format libraries in different languages. `bravo-daw` gives you all four majors behind one function call, fast enough to run on every file-save event.

Extracted from [BRAVOH](https://altidus.world) Studio, where these parsers run in production on real artists' projects every day.

## Quick start

### CLI

```bash
cargo install --git https://github.com/ozzaii/bravo-daw
bravo-daw my_track.als
```

```json
{
  "daw": "Ableton Live",
  "daw_version": "Ableton Live 12.0",
  "bpm": 124.0,
  "tracks": [
    { "name": "Drums", "type": "audio", "plugins": [] },
    { "name": "Bass Synth", "type": "midi", "plugins": [] }
  ],
  "plugins": [],
  "samples": [],
  "midi_tracks": [],
  "markers": [],
  "mixer": {
    "total_tracks": 2,
    "audio_tracks": 1,
    "midi_tracks": 1,
    "return_tracks": 0,
    "group_tracks": 0,
    "has_sidechain": false
  },
  "automated_params": []
}
```

### Library

```toml
[dependencies]
bravo-daw = { git = "https://github.com/ozzaii/bravo-daw" }
```

```rust
// Auto-detect the format from the extension
let intel = bravo_daw::parse("my_track.als")?;
println!("{} @ {:?} BPM, {} tracks", intel.daw, intel.bpm, intel.tracks.len());

// Or parse as a specific format
use bravo_daw::Daw;
let intel = bravo_daw::parse_as("weird_name.backup", Daw::AbletonLive)?;
```

## What gets extracted

| Field | Ableton Live | FL Studio | Logic Pro | REAPER |
|---|:-:|:-:|:-:|:-:|
| BPM | ✅ | ✅ | ✅ | ✅ |
| Time signature | — | ✅ | ✅ | ✅ |
| DAW version | ✅ | — | — | ✅ |
| Tracks (name + type) | ✅ | ✅ | ✅ | ✅ |
| Plugins (name + kind + count) | ✅ | ✅ | ✅ | ✅ |
| Samples | ✅ | — | ✅ | ✅ |
| MIDI stats (notes, pitch range) | ✅ | — | — | ✅ |
| Markers | ✅ | — | ✅ | ✅ |
| Mixer summary (track counts, sidechain) | ✅ | ✅ | ✅ | ✅ |
| Automated parameters | ✅ | — | — | ✅ |
| Duration + swing | — | — | ✅ | — |

## Reliability doctrine

These formats are proprietary and partially reverse-engineered. The rule this crate lives by:

> **If a field can't be parsed dependably, it is omitted — never guessed.**

That's why the matrix above has gaps. A missing field (`None` / empty vec) means "this DAW doesn't expose it reliably", not "we forgot". Logic Pro is the extreme case: its `ProjectData` binary is undocumented, so the parser extracts only what survives string-level analysis (plugin chains, samples, markers, routing) and refuses to report values that Logic itself computes unreliably.

Corrupt or truncated files return an error instead of panicking — every parser is fuzz-tested against garbage input in its test suite.

## MSRV

Rust 1.87.

## Contributing

Issues and PRs welcome — especially:

- Sample project files from different DAW versions (the more real-world fixtures, the better)
- Field coverage improvements for FL Studio and Logic Pro
- Support for more DAWs (Cubase `.cpr`, Studio One `.song`, Bitwig `.bwproject`)

Run the gates before pushing:

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

## License

[MIT](LICENSE) © 2026 [BRAVOH](https://altidus.world)

---

*Part of **Bravo**, the open-source initiative from the team building BRAVOH — the AI operating system for music artists.*
