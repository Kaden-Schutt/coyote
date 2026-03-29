# (co)yote Project Nomenclature

## ⚠️ READ THIS FIRST — the project has been renamed.

**sonicpack is now called yip.** It is one member of a larger protocol family called **(co)yote**.

## The family

**(co)yote** is the umbrella project name. "Data smuggling through audio codecs."
The parenthetical reads as both "coyote" and "co-yote" (co-authored with Claude).

Five protocols, four coyote vocalizations + one modulation technique:

| Name | What it is | Medium | Throughput | File ext |
|------|-----------|--------|-----------|----------|
| **yip** | Hand-crafted FSK, mono | Opus / any VoIP | 987 B/s (optimized) | `.yip` |
| **yip+CRAM** | Stereo: reference + data channel | Opus stereo | 1.5-2+ KB/s (est.) | `.yip` |
| **yarl** | Learned neural encoder/decoder | Opus / any VoIP | 2-8+ KB/s (target) | `.yarl` |
| **yowl** | Codec2 / narrowband | HF radio, FM, walkie-talkie | 50-100 B/s | `.yowl` |
| **yap** | Acoustic coupling (including ultrasonic) | Physical air (speaker→mic) | 50-500 B/s | `.yap` |

## CRAM — Codec-Relative Amplitude Modulation

A stereo modulation technique usable by any (co)yote protocol. Not a separate protocol,
but a mode/modifier.

**How it works:**
- Left channel: known reference waveform (clean tones at known frequencies/amplitudes)
- Right channel: data-carrying signal
- On decode: measure what the codec did to the reference (per-bin attenuation, phase shift)
- Apply that measured transfer function to interpret the data channel
- Result: you know exactly how the codec distorted each bin, so you can use amplitude
  levels as a data dimension — not just "which tone" but "how loud relative to reference"

**Why it works:**
Opus stereo uses mid/side coding: M=(L+R)/2, S=(L-R)/2. The inter-channel relationship
is a first-class citizen in the codec's model. The ratio between reference and data channels
is exactly what Opus is designed to preserve faithfully.

**What it unlocks:**
Standard yip uses 2-tone or 4-tone FSK: 1-2 bits per bin per frame.
CRAM enables amplitude modulation with 8-16 distinguishable levels: 3-4 bits per bin.
79 bins × 3 bits × 50 fps = 11,850 bps (~1,481 B/s) per stereo pair.
79 bins × 4 bits × 50 fps = 15,800 bps (~1,975 B/s) per stereo pair.

**Limitations:**
Requires stereo-capable channels (files, podcasts, Discord, WebRTC — not phone calls).
Doubles the audio bandwidth used (stereo vs mono).

The name: you're literally CRAMming data into the channel.

## What this means for the current codebase

- The Rust crate that exists right now (sonicpack-rs) = **yip**
- The `.spk` file format → `.yip`
- The `sonicpack` CLI binary → `yip`
- The `sonicpack` crate name → `yip`
- `SONICPACK` references in docs/comments → `yip` or `(co)yote` as appropriate
- The W&B project can stay as-is for now but future runs should use `yote-yip` or similar

## Naming conventions going forward

- GitHub org/monorepo: `yote` (or `coyote`)
- Crate names: `yip`, `yarl`, `yowl`, `yap`
- Shared code (framing, crypto, wire headers): `yote-core` or just inline in each crate
- CLI commands: `yip encode`, `yip decode`, `yip stats`, etc.
- CRAM is a flag/mode, not a separate binary: `yip encode --cram -o out.yip`

## Autoresearch results (Phase 1 complete)

Optimized yip config (mono, no CRAM):
- Quad (4-tone FSK), 79 bins, 250Hz spacing, 128kbps Opus, 20ms frames
- 7,900 bps = 987 B/s raw
- 3.3x improvement over original baseline (2,400 bps / 300 B/s)
- With zstd on text: ~3-5 KB/s effective
- With zstd on JSON: ~8-10 KB/s effective

## Priority order

1. ✅ Phase 1: Autoresearch throughput optimization (DONE — 987 B/s)
2. Phase 1.5: CRAM implementation (stereo reference channel — est. 1.5-2 KB/s)
3. Phase 2: yarl neural model (target: beat CRAM ceiling)
4. Phase 3: Crypto layer (PLAN-crypto.md)
5. Future: yowl (Codec2/HF), yap (acoustic/ultrasonic)

## Do NOT rename files/dirs yet

Just update the naming in:
1. Config defaults (the autoresearch winner) ← IN PROGRESS
2. Cargo.toml (crate name, binary name, description)
3. CLI help text and output messages
4. File format magic/extension (.spk → .yip)
5. README.md
6. Doc comments in lib.rs

The directory can stay as `sonicpack-rs` for now to avoid breaking paths.
