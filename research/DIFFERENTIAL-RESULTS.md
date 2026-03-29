# Differential Encoding — Autoresearch Results

## Summary

**Differential encoding unlocks 16 amplitude levels (up from 8 absolute).**

Best zero-error config: **13,500 bps (1,687 B/s)** — 16amp × 2phase, 5 bits per bin.

This is a **1.11x improvement over absolute pilot encoding** (12,200 bps / 1,525 B/s).

## What Works and What Doesn't

| Constellation | Absolute | Differential | Status |
|--------------|----------|-------------|--------|
| 8 amp × 2 ph | ✅ 12,200 bps | ✅ works | Baseline |
| **16 amp × 1 ph** | ❌ fails | **✅ 20 configs** | **DIFF UNLOCKS** |
| **16 amp × 2 ph** | ❌ fails | **✅ 30 configs** | **DIFF UNLOCKS — BEST** |
| 16 amp × 4 ph | ❌ fails | ❌ fails | Too much |
| 32 amp × any | ❌ fails | ❌ fails | Way too much |
| any × 4+ phase | ❌ fails | ❌ fails | Phase hard to distinguish |

**Conclusion:** Differential encoding adds exactly 1 bit per bin in the amplitude dimension.
Phase remains capped at 2 levels — differential doesn't help with phase resolution.

## Sweep Parameters

- **Base configs:** 11 pilot winner variants (spacing 5-10, bins 48-79, bitrate 128-256)
- **Amplitude levels:** 2, 4, 8, 16, 32
- **Phase levels:** 1, 2, 4, 8
- **Recalibration interval:** 0 (never), 5, 10, 20, 50 frames
- **Diff mode:** Both, AmpOnly, PhaseOnly
- **Total trials:** 2,200 (12 workers × ~184 each)
- **Runtime:** ~5 minutes wall clock

## Top Configs

| Rank | Eff bps | B/s  | Amp×Ph | bpb | Spacing | Bins | kbps | Recal | Mode |
|------|---------|------|--------|-----|---------|------|------|-------|------|
| 1    | 13,500  | 1687 | 16×2   | 5   | 10      | 60   | 256  | 0     | Any  |
| 2    | 13,235  | 1654 | 16×2   | 5   | 10      | 60   | 256  | 50    | Any  |
| 3    | 12,857  | 1607 | 16×2   | 5   | 10      | 60   | 256  | 20    | Any  |
| 4    | 12,272  | 1534 | 16×2   | 5   | 10      | 60   | 256  | 10    | Any  |
| 5    | 11,250  | 1406 | 16×2   | 5   | 10      | 60   | 256  | 5     | Any  |

## Key Insights

1. **recal_interval=0 is best.** No recalibration needed — the initial calibration frame is
   sufficient. Opus's temporal smoothing keeps the channel stable enough that drift isn't
   an issue. This means zero overhead from calibration.

2. **All three diff modes work equally.** AmpOnly, PhaseOnly, and Both produce identical
   results. This makes sense: with modular wrapping, the delta encoding is just relabeling
   the constellation points. The real benefit is that Opus preserves frame-to-frame
   continuity better than absolute values.

3. **The gain is exactly +1 bit on amplitude.** 8 → 16 amplitude levels. Not 32. The
   differential domain gives one extra "notch" of distinguishability.

4. **Phase stays at 2 levels max.** Even with differential encoding, 4 phase levels fail.
   Phase is inherently harder for Opus to preserve than amplitude.

## Throughput Progression

```
yip mono FSK baseline:    3,950 bps  (493 B/s)
yip mono FSK optimized:   7,900 bps  (987 B/s)   — Quad, 128kbps
CRAM stereo best:         7,900 bps  (987 B/s)   — 2a×2p stereo
Pilot mono best:         12,200 bps (1,525 B/s)   — 8a×2p, mono 256kbps
Diff + Pilot best:       13,500 bps (1,687 B/s)   — 16a×2p, mono 256kbps  ← NEW
```

**Compound improvement: 1.71x over CRAM, 3.42x over baseline.**
With zstd on text: 1,687 × 3-5x = **5.0-8.4 KB/s effective.**

## Recommended Config

```rust
DiffConfig {
    pilot: PilotConfig {
        config: Config { n_bins: 60, bin_spacing: 200.0, opus_bitrate: 256, .. },
        pilot_spacing: 10,
        amp_levels: 16,
        phase_levels: 2,
        pilot_amplitude: 1.0,
        interp_method: InterpMethod::NearestNeighbor,
    },
    recal_interval: 0,
    diff_mode: DiffMode::Both,
}
// 13,500 bps = 1,687 B/s
```

## Raw Data

Full CSV: `diff_autoresearch_results.csv` (2,201 rows)
