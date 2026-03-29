# Phase 1 Results: Programmatic Throughput Ceiling

## Winner

| Parameter | Value |
|-----------|-------|
| Depth | Quad (2-bit, 4-tone FSK) |
| Bins | 79 |
| Bin spacing | 250 Hz |
| Base freq | 200 Hz |
| Max freq | 19,700 Hz |
| Opus bitrate | 128 kbps |
| Frame duration | 20 ms (960 samples) |
| **Throughput** | **7,900 bps (987 B/s)** |
| Bit errors | ZERO |

## Improvement

- Baseline: 2,400 bps (300 B/s) — Binary, 48 bins, 64 kbps
- Winner: 7,900 bps (987 B/s) — **3.3x improvement**
- Token rates: ~247 tok/s UTF-8, ~493 tok/s int16 IDs

## Key Findings

### Depth
- **Quad dominates.** Binary tops out at 3,950 bps. Hex16 has zero working configs.
- The 4-tone FSK sweet spot: enough levels to double throughput, close enough spacing that Opus preserves the tone positions.

### Bitrate
- **128 kbps is the sweet spot.** Same results as 192/256 kbps but less bandwidth.
- 96 kbps still achieves 7,100 bps (Quad, 71 bins) — good fallback.
- 64 kbps maxes at 5,500 bps. 48 kbps: 3,900 bps. 32 kbps: nothing works.

### Frequency Range
- Extending to **19,700 Hz** (from 12,000 Hz) was the biggest win.
- At 128+ kbps, Opus preserves spectral content up to ~20 kHz.
- This allowed 79 bins (up from 48) — a 1.65x increase in carrier count.

### Frame Duration
- **20 ms wins** over 40 ms for raw throughput.
- 10 ms: zero working configs (insufficient frequency resolution).
- 40 ms: caps at 6,600 bps (more bins but half the frame rate).

### Bin Spacing
- **250 Hz remains optimal.** Same as original. Not too tight (Opus smearing), not too wide (wasted bandwidth).

## Sweep Statistics

- Total parameter combinations: 2,205
- Valid configurations: 875
- Zero-error configurations: 425
- Sweep runtime: 1,028 seconds (17 minutes) on AMD 3900X

## Predefined Configs

| Config | Depth | Bins | Bitrate | Throughput | Use case |
|--------|-------|------|---------|------------|----------|
| `Config::default()` | Quad | 79 | 128 kbps | 987 B/s | Standard — maximum throughput |
| `Config::low_bandwidth()` | Quad | 71 | 96 kbps | 887 B/s | Constrained connections |
| `Config::conservative()` | Binary | 48 | 64 kbps | 300 B/s | Maximum compatibility, lowest bitrate |

## Math Stack Results (Phase 1.5)

### Compound Throughput Progression

| Stage                    | Throughput | B/s   | Multiplier | With zstd text |
|--------------------------|-----------|-------|------------|----------------|
| Mono FSK baseline        | 3,950     | 493   | 1.0x       | ~1.5-2.5 KB/s  |
| Mono FSK optimized       | 7,900     | 987   | 2.0x       | ~3-5 KB/s      |
| CRAM stereo              | 7,900     | 987   | 2.0x       | ~3-5 KB/s      |
| + Pilots (Technique 1)   | 12,200    | 1,525 | 3.1x       | ~4.5-7.6 KB/s  |
| + Differential (Tech 2)  | 13,500    | 1,687 | 3.4x       | ~5-8.4 KB/s    |
| + Shaping (Tech 3)       | 13,500    | 1,687 | 3.4x       | NO IMPROVEMENT |

### Best Configuration (Math Stack Champion)

| Parameter | Value |
|-----------|-------|
| Modulation | Pilot + Differential |
| Amplitude levels | 16 (4 bits) |
| Phase levels | 2 (1 bit) |
| Bits per bin | 5 |
| Pilot spacing | 10 (every 10th bin is a pilot) |
| Data bins | 54 (of 60 total) |
| Bin spacing | 200 Hz |
| Opus bitrate | 256 kbps mono |
| Recalibration | None needed (recal_interval=0) |
| Interpolation | NearestNeighbor |
| **Throughput** | **13,500 bps (1,687 B/s)** |
| Bit errors | ZERO |

### Sweep Statistics

| Sweep | Trials | Zero-error | Runtime |
|-------|--------|-----------|---------|
| CRAM (stereo) | 2,400 | 39 | ~80 min |
| Pilot (mono) | 4,320 | 1,278 | ~10 min |
| Differential | 2,200 | 609 | ~5 min |
| Constellation | 2,304 | 373 | ~5 min |
| **Total** | **11,224** | **2,299** | |

### Key Insights

1. **Pilots > CRAM.** Mono pilots beat stereo reference (no 50% bandwidth tax)
2. **Differential unlocks 16 amp levels.** Absolute max is 8; diff adds 1 bit/bin
3. **Phase caps at 2 levels.** Neither diff nor shaping can push beyond 2
4. **Constellation shaping doesn't help.** Uniform grids already match Opus well
5. **Math stack ceiling: 5 bits/bin × 54 bins × 50 fps = 13,500 bps**

## What's Next

The math stack establishes the **programmatic ceiling at 1,687 B/s (13,500 bps)**.

Phase 2 (yarl) targets >1,687 B/s using a learned neural encoder/decoder trained end-to-end with Opus as the frozen channel. Scaffold built, real Opus channel verified. Training pending.
