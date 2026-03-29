# Pilot Tone Interpolation — Autoresearch Results

## Summary

**Best zero-error config: 12,200 bps (1,525 B/s) — 1.55x improvement over CRAM**

Pilot tone interpolation works in MONO Opus, eliminating CRAM's 50% stereo overhead.
Channel estimation via scattered pilots achieves higher throughput than CRAM's dedicated
reference channel approach.

## Sweep Parameters

- **Pilot spacing:** 3, 4, 5, 6, 8, 10
- **Amplitude levels:** 2, 4, 8
- **Phase levels:** 1, 2, 4
- **Opus bitrate:** 64, 96, 128, 192, 256 kbps
- **Bin count:** 48, 60, 70, 79
- **Bin spacing:** 200, 250 Hz
- **Interpolation:** Linear, NearestNeighbor
- **Pilot amplitude:** 1.0

**Total trials:** 4,320 (12 workers × 360 each on Ryzen 9 3900X)
**Runtime:** ~10 minutes wall clock (parallelized)

## Results

### Zero-error configs: 1,278 / 4,320 (29.6% success rate)

### Top 10 by throughput (all zero errors at 50B, 500B, 2KB payloads)

| Rank | Throughput | B/s  | Spacing | Amp×Phase | bpb | Data/Total bins | Hz   | Bitrate | Interp  |
|------|-----------|------|---------|-----------|-----|-----------------|------|---------|---------|
| 1    | 12,200    | 1525 | 8       | 8×2       | 4   | 61/70           | 200  | 256     | NN      |
| 2    | 12,200    | 1525 | 8       | 8×2       | 4   | 61/70           | 200  | 256     | Linear  |
| 3    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 250  | 256     | NN      |
| 4    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 250  | 256     | Linear  |
| 5    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 200  | 256     | NN      |
| 6    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 200  | 256     | Linear  |
| 7    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 200  | 192     | NN      |
| 8    | 10,800    | 1350 | 10      | 8×2       | 4   | 54/60           | 200  | 192     | Linear  |
| 9    | 10,650    | 1331 | 10      | 4×2       | 3   | 71/79           | 250  | 256     | NN      |
| 10   | 10,650    | 1331 | 10      | 4×2       | 3   | 71/79           | 250  | 192     | NN      |

### Best per constellation

| Constellation | Best bps | B/s  | Spacing | Bins | Bitrate |
|--------------|----------|------|---------|------|---------|
| 2 amp × 1 ph | 3,550    | 443  | 10      | 79   | 256     |
| 2 amp × 2 ph | 7,100    | 887  | 10      | 79   | 256     |
| 4 amp × 1 ph | 7,100    | 887  | 10      | 79   | 256     |
| 4 amp × 2 ph | 10,650   | 1331 | 10      | 79   | 256     |
| 8 amp × 1 ph | 8,100    | 1012 | 10      | 60   | 256     |
| 8 amp × 2 ph | **12,200** | **1525** | 8 | 70   | 256     |

### Zero-error distribution by parameter

**By constellation (amp×phase):**
- 2×1: 383 configs (most robust)
- 2×2: 292
- 4×1: 234
- 4×2: 208
- 8×1: 82
- 8×2: 79

**By bitrate:**
- 256 kbps: 457
- 192 kbps: 445
- 128 kbps: 265
- 96 kbps: 100
- 64 kbps: 11

**By pilot spacing:**
- 8: 224
- 5: 222
- 10: 220
- 4: 217
- 6: 202
- 3: 193

## Key Insights

1. **8 amplitude levels work with pilots but NOT with CRAM.** CRAM maxed out at 2 amp levels
   because the stereo reference channel introduces coupling artifacts. Mono pilots avoid this.

2. **Phase encoding (×2) consistently helps.** Adding 2 phase levels doubles information density
   per bin with minimal error increase. 4 phase levels: zero zero-error configs found.

3. **Pilot spacing is surprisingly tolerant.** All spacings from 3 to 10 work well. Spacing=8
   wins for max throughput (fewer pilots = more data bins) while still providing adequate
   channel estimation.

4. **192 kbps nearly matches 256 kbps.** Many top configs work at both bitrates, suggesting
   192 kbps is sufficient for most pilot configurations.

5. **Both interpolation methods work equally well.** Linear and NearestNeighbor produce
   identical results for most configs (Opus's frequency response is smooth enough that
   simple interpolation suffices).

## Comparison with Previous Results

```
yip mono FSK baseline:    3,950 bps  (493 B/s)  — Phase 1
yip mono FSK optimized:   7,900 bps  (987 B/s)  — Phase 1 (Quad, 128kbps)
CRAM stereo best:         7,900 bps  (987 B/s)  — 2amp×2phase, 79bins, 256kbps
Pilot mono best:         12,200 bps (1,525 B/s)  — 8amp×2phase, 70bins, 256kbps  ← NEW
```

**Pilot vs CRAM: 1.55x improvement** (1,525 vs 987 B/s)
**Pilot vs mono FSK: 1.55x improvement** (same comparison — CRAM didn't beat FSK)

## Recommended Default Config

For general use, the robust sweet spot:

```rust
PilotConfig {
    config: Config { n_bins: 79, bin_spacing: 250.0, opus_bitrate: 192, .. },
    pilot_spacing: 10,
    amp_levels: 4,
    phase_levels: 2,
    pilot_amplitude: 1.0,
    interp_method: InterpMethod::Linear,
}
// 10,650 bps = 1,331 B/s — works at 192kbps, plenty of margin
```

For maximum throughput:

```rust
PilotConfig {
    config: Config { n_bins: 70, bin_spacing: 200.0, opus_bitrate: 256, .. },
    pilot_spacing: 8,
    amp_levels: 8,
    phase_levels: 2,
    pilot_amplitude: 1.0,
    interp_method: InterpMethod::NearestNeighbor,
}
// 12,200 bps = 1,525 B/s — requires 256kbps
```

## Next Steps

1. Stack with Technique 2 (Differential Encoding) for potential 1.3x additional gain
2. Stack with Technique 3 (Constellation Shaping) using this sweep's error data
3. Test at higher phase levels with error correction coding
4. With zstd on text: 1,525 B/s × 3-5x = 4.5-7.6 KB/s effective throughput

## Raw Data

Full CSV: `pilot_autoresearch_results.csv` (4,321 rows including header)
