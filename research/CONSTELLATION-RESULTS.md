# Constellation Shaping — Autoresearch Results

## Summary

**Constellation shaping does NOT improve over uniform grids with differential encoding.**

Best zero-error config: 13,500 bps (1,687 B/s) — same as differential, using uniform 16amp × 2phase.

## What We Tested

Three constellation types:
1. **Uniform grid** — standard amp×phase grid (baseline comparison)
2. **Nonuniform amplitude** — log/sqrt-spaced amplitude levels (wider gaps at extremes)
3. **Pruned grid** — take NxM grid, keep only K most reliable points

## Results

### Zero-error configs: 373 / 2,304 trials (16.2%)

### Best per constellation type

| Type | Best bps | B/s | Points | Notes |
|------|---------|-----|--------|-------|
| **Uniform** | **13,500** | **1,687** | 32 (16a×2p) | Same as diff result — still the winner |
| Pruned | 12,200 | 1,525 | 16 (from 8x2) | Pruning doesn't help when full grid works |
| Nonuniform | 8,100 | 1,012 | 8 | Significantly worse — uniform spacing is better |

### Key findings

1. **Nonuniform spacing hurts.** The uniform amplitude mapping (0.2 + 0.8 * sym/(levels-1))
   is already well-matched to Opus's psychoacoustic model. Log/sqrt spacing makes points
   too close at low amplitudes where Opus has less precision.

2. **Pruning doesn't help.** Keeping 16 points from an 8×2 grid is the same as just using
   the 8×2 grid. The pruned points that survive are the same ones the uniform grid uses.

3. **Uniform + differential remains the champion.** The hand-crafted math stack hit the
   right answer: uniform grids are already optimal for Opus's linear amplitude response
   in the frequency domain.

4. **6 configs work with 32 uniform points (5 bps/symbol).** These are the 16×2 configs
   that also appeared in the differential sweep. Constellation shaping confirms this is
   the ceiling for the pilot+diff architecture.

## Throughput Progression (Final Math Stack)

```
| Stage                    | Throughput | B/s   | Multiplier | With zstd text |
|--------------------------|-----------|-------|------------|----------------|
| Mono FSK baseline        | 3,950     | 493   | 1.0x       | ~1.5-2.5 KB/s  |
| Mono FSK optimized       | 7,900     | 987   | 2.0x       | ~3-5 KB/s      |
| + Pilots (Technique 1)   | 12,200    | 1,525 | 3.1x       | ~4.5-7.6 KB/s  |
| + Differential (Tech 2)  | 13,500    | 1,687 | 3.4x       | ~5-8.4 KB/s    |
| + Shaping (Tech 3)       | 13,500    | 1,687 | 3.4x       | NO IMPROVEMENT |
| yarl neural (Phase 2)    | ???       | ???   | ???        | target: >1,687 |
```

**Math stack ceiling: 13,500 bps (1,687 B/s) = 5 bits/bin × 54 data bins × 50 fps.**
This is the number yarl must beat.

## Raw Data

Full CSV: `constellation_autoresearch_results.csv` (2,305 rows)
