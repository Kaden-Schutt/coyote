# Bitrate Tiers — Per-Bitrate Optimal Configs

## Overview

The yip math stack provides zero-error data-over-Opus at every common bitrate from
64-256 kbps. Each tier has a sweep-validated configuration that maximizes throughput
while maintaining zero bit errors.

## Tier Table

| Tier | Opus bitrate | Throughput | B/s | Bits/bin | Amp×Phase | Differential | Config |
|------|-------------|-----------|------|---------|-----------|-------------|--------|
| **ultra** | 256 kbps | 13,500 bps | 1,687 | 5 | 16×2 | ✅ recal=0 | ps=10 60bins 200Hz NN |
| **high** | 192 kbps | 12,200 bps | 1,525 | 4 | 8×2 | ✅ recal=0 | ps=8 70bins 200Hz NN |
| **standard** | 128 kbps | 9,450 bps | 1,181 | 3 | 4×2 | ❌ | ps=10 70bins 200Hz Linear |
| **medium** | 96 kbps | 6,100 bps | 762 | 2 | 2×2 | ❌ | ps=8 70bins 200Hz Linear |
| **low** | 64 kbps | 2,700 bps | 337 | 1 | 2×1 | ❌ | ps=10 60bins 250Hz Linear |
| **minimal** | 48 kbps | ~2,600 bps* | ~325* | 1 | 2×1 | ❌ | ps=8 60bins 200Hz NN |

*\* minimal is EXPERIMENTAL — passes 50B payloads only, fails at larger sizes.*

## Use Cases

| Tier | Channel | Example |
|------|---------|---------|
| ultra | High-quality files, podcasts | Podcast distribution, file sharing |
| high | Discord music bot, WebRTC | AI assistant voice channels |
| standard | Discord voice, Zoom, Meet | General VoIP, the safe default |
| medium | Mobile VoIP, Google Duo | Phone-quality apps |
| low | Traditional phone calls | PSTN, low-bandwidth links |
| minimal | Emergency/worst-case | Satellite phone, extreme bandwidth |

## With zstd Compression

For text/JSON payloads, zstd typically achieves 3-5x compression:

| Tier | Raw B/s | With zstd (text) | With zstd (JSON) |
|------|---------|-----------------|-----------------|
| ultra | 1,687 | 5.0-8.4 KB/s | 8.4-16.9 KB/s |
| high | 1,525 | 4.6-7.6 KB/s | 7.6-15.3 KB/s |
| standard | 1,181 | 3.5-5.9 KB/s | 5.9-11.8 KB/s |
| medium | 762 | 2.3-3.8 KB/s | 3.8-7.6 KB/s |
| low | 337 | 1.0-1.7 KB/s | 1.7-3.4 KB/s |

## API

```rust
use yip::tiers::Tier;

// Auto-select best tier for your bitrate
let tier = Tier::for_bitrate(128);  // -> Standard
println!("{}", tier);  // "standard (128kbps): 9,450 bps = 1,181 B/s"

// Direct tier selection
let data = b"hello from yip";
let recovered = Tier::Ultra.roundtrip(data)?;
assert_eq!(recovered, data);

// List all tiers
for tier in Tier::all() {
    println!("{}: {} B/s", tier.name(), tier.throughput_bytes());
}
```

## Sweep Data Sources

| Sweep | Trials | CSV |
|-------|--------|-----|
| Pilot (absolute) | 4,320 | pilot_autoresearch_results.csv |
| Differential | 2,200 | diff_autoresearch_results.csv |
| Constellation | 2,304 | constellation_autoresearch_results.csv |
| Tier mini-sweep (48/64/128) | 3,456 | tier_sweep_results.csv |
| **Total** | **12,280** | |

## Key Design Decisions

1. **Differential only at 192+ kbps.** At 128kbps and below, the pilot-only approach
   is more reliable. Diff adds overhead (calibration frames) that hurts more than the
   extra amplitude levels help at lower bitrates.

2. **NearestNeighbor interpolation at high bitrates, Linear at low.** At high bitrates
   Opus preserves frequency response cleanly so NN's simplicity wins. At low bitrates
   the channel is noisier and Linear interpolation smooths estimation errors.

3. **Phase=2 everywhere except 64kbps.** Binary phase (0 or π) is robust and doubles
   throughput. At 64kbps, even phase=2 is too much — amplitude-only (phase=1) is needed.

4. **48kbps is experimental.** It works for tiny payloads but CRC failures at larger
   sizes make it unreliable for production use. Include it for completeness but warn users.
