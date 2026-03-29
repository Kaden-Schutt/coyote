# yawp — Neural Error Correction

## Overview

yawp is an optional two-stage gated decoder that improves yip's error rate
on degraded audio channels. It runs after the standard FFT decoder and
corrects bins where FFT confidence is low.

## Architecture

```
PCM → FFT Decoder → symbols + confidence scores
                  ↓
              Neural Corrector (46K params, PyTorch)
                  ↓
              Gate: blend FFT and neural predictions
                  ↓
              Corrected symbols
```

**Stage 1 — FFT with Confidence:**
For each of 79 bins, compute DFT energy at 4 candidate tone frequencies.
The symbol is the argmax. Confidence = best / second-best energy ratio.

**Stage 2 — Neural Corrector:**
A small MLP (45→256→128→4) takes per-bin features:
- FFT symbol (one-hot, 4 values)
- Log confidence (1 value)
- Normalized magnitudes (4 values)
- Bitrate embedding (16 values)
- Neighbor context (±2 bins, 20 values)

A parallel gate MLP (45→32→1→sigmoid) outputs a value in [0,1]:
- gate ≈ 0 → trust FFT (clean audio)
- gate ≈ 1 → trust neural correction (degraded audio)

## Performance

| Condition | FFT-only | yawp (FFT + neural) |
|-----------|----------|-------------------|
| Clean Opus | 0.000% | 0.000% |
| Double transcode | 18.1% | 4.67% |
| Triple transcode | 46.1% | 45.6% |
| Noise SNR 10dB | 0.11% | 0.06% |
| Packet loss 10% | 2.8% | 2.7% |
| Resample 48k→16k→48k | 20.5% | 19.7% |

The gate guarantees yawp never performs worse than FFT on clean audio.

## Usage

yawp activates automatically when PyTorch is installed:

```bash
pip install torch    # enables yawp
```

Disable with:
```bash
export YOTE_NO_YAWP=1
# or
yote unyip file.yip --no-yawp
```

## Training

The model was trained with curriculum learning:
1. Clean Opus data (gate learns to pass through)
2. Synthetic degradation (noise, clipping, packet loss)
3. Real Opus transcode data (double/triple re-encoding)

Checkpoint selection used a multi-objective score balancing clean BER,
double-transcode BER, packet loss, resample, and time stretch performance.
