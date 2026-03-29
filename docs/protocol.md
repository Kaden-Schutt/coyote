# yip Protocol Specification

## Overview

yip encodes data into Opus audio using parallel Frequency-Shift Keying (FSK).
79 frequency bins each carry 2 bits per 20ms frame, yielding 7,900 bps at
128kbps Opus with zero bit errors.

## Modulation

Each data frame is 960 PCM samples at 48kHz (20ms). The encoder places one
sine tone per bin at a frequency determined by the 2-bit symbol:

```
bin_center = 200 + bin_index × 250 Hz
tone_freq  = bin_center + offset(symbol)
```

Symbols 0–3 map to four equally-spaced frequency offsets within each 250Hz bin.
The decoder computes DFT energy at all four candidate frequencies and picks
the argmax.

## Frequency Plan

| Parameter | Value |
|-----------|-------|
| Sample rate | 48,000 Hz |
| Frame size | 960 samples (20ms) |
| Base frequency | 200 Hz |
| Bin spacing | 250 Hz |
| Number of bins | 79 |
| Max frequency | ~19,950 Hz |
| Depth | Quad (4-FSK, 2 bits/bin) |

## Pilot Tones

Every Nth bin is reserved as a pilot (known symbol). The decoder uses pilots
to estimate and correct channel gain variations across the spectrum via
linear interpolation.

## Differential Encoding

Symbols are differentially encoded across bins to eliminate absolute-phase
ambiguity. The decoder compares adjacent bin phases rather than requiring
absolute frequency calibration.

## Wire Format (.yip container)

```
[Wire Header]        Version, depth, bins, compression flag, filename
[Sync Magic]         4-byte magic for frame alignment
[Length]              4-byte LE payload length
[CRC32]              4-byte LE checksum
[Compressed Data]    zstd-compressed payload
```

## Bitrate Tiers

| Opus Bitrate | yip Throughput | Bytes/sec |
|-------------|---------------|-----------|
| 64 kbps | 4,800 bps | 600 B/s |
| 96 kbps | 6,400 bps | 800 B/s |
| 128 kbps | 7,900 bps | 987 B/s |
| 192 kbps | 10,800 bps | 1,350 B/s |
| 256 kbps | 13,500 bps | 1,687 B/s |
