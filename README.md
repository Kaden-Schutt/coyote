# coyote

**Send data through sound.**

*Built with [Hermes Agent](https://github.com/NousResearch/hermes-agent)*

---

coyote is a protocol family for encoding arbitrary data into audio that survives lossy codec compression. The first protocol, **yip**, achieves zero bit errors through Opus at rates up to 13,500 bps. An optional neural decoder, **yawp**, extends reliability to degraded channels — relay hops, transcoding, noise.

The motivating use case is autonomous agent communication through existing audio infrastructure: voice channels, VoIP, broadcast, or any medium that carries sound.

## Install

```bash
pip install yote
```

Or build from source:
```bash
cargo build --release    # Rust CLI
maturin develop --features python  # Python bindings
```

## Usage

### Files

```bash
yote yip document.pdf        # → document.pdf.yip (compressed, encoded)
yote unyip document.pdf.yip  # → document.pdf (identical to original)
yote info document.pdf.yip   # metadata
```

### Streaming

```bash
# Receiver
yote rx --via port:7777 -o ~/incoming/

# Sender
yote tx payload.bin --via port:7777

# Bidirectional
yote link --via port:7777

# Background listener
yote install -p 7777
```

### Python

```python
import yote

# In-memory
packets = yote.encode(b"payload", bitrate=128)
data = yote.decode(packets)

# Files
yote.yip("document.pdf")
yote.unyip("document.pdf.yip")
```

## How it works

yip uses parallel Frequency-Shift Keying across 79 frequency bins. Each bin carries a 2-bit symbol encoded as one of four tone positions. Opus is designed to preserve tonal content — yip places tones at frequencies the codec faithfully reproduces, then detects them via matched DFT on the other end.

```
encode: data → zstd → FSK modulate (79 bins × 2 bits) → Opus
decode: Opus → FFT demodulate → [yawp correct] → zstd → data
```

The frequency plan, bin spacing, and amplitude normalization were optimized over 15,736 automated parameter sweep trials. CMA-ES independently confirmed the hand-tuned configuration is a local optimum — neural encoders cannot improve on it.

## Performance

| | Value |
|---|---|
| Throughput (128 kbps Opus) | 7,900 bps · 987 B/s |
| Throughput (256 kbps Opus) | 13,500 bps · 1,687 B/s |
| Bit error rate (clean) | 0.000% |
| Bit error rate (double transcode, yawp) | 4.67% |
| Bit error rate (double transcode, FFT only) | 18.1% |
| Frame duration | 20 ms (960 samples, 48 kHz) |
| Modulation | 79-bin Quad FSK, pilot tones, differential encoding |

**Capacity per minute of audio:**

| Opus bitrate | Payload |
|---|---|
| 64 kbps | ~18 KB |
| 128 kbps | ~59 KB |
| 256 kbps | ~101 KB |

## Protocols

**yip** — FSK over Opus. Shipping in v0.1. Zero-error on clean channels, 6 bitrate tiers, zstd compression, CRC32 integrity checks.

**yawp** — Neural error correction for yip. Shipping in v0.1. A 46K-parameter two-stage gated decoder (PyTorch, optional). FFT decodes first; a neural corrector overrides only on low-confidence bins. The gate guarantees clean performance is never degraded. Reduces double-transcode errors by 3.3×. Enable by installing PyTorch; disable with `YOTE_NO_YAWP=1` or `--no-yawp`.

**yowl** — HF radio protocol. Planned.

**yap** — Air-gap and ultrasonic protocol. Speaker-to-microphone data transfer. Planned.

## Agent integration

### Hermes Agent

coyote ships with an [agentskills.io](https://agentskills.io)-compatible skill:

```bash
hermes skills install github:Kaden-Schutt/coyote/skills/yote
```

### Discord voice

Discord voice uses Opus natively. yote-encoded data is valid Opus audio — it transmits through voice channels without modification. To other listeners it sounds like noise; to a yote-equipped receiver it's data.

```python
from yote.discord_bridge import YoteBridge

bridge = YoteBridge(bot=discord_bot)
await bridge.share_knowledge(guild_id, channel_id, {
    "topic": "training results",
    "data": {"accuracy": 0.95, "checkpoint": "v2"},
})
```

## CLI reference

```
yote yip <file>            Encode file to .yip container
yote unyip <file.yip>      Decode .yip to original file
yote info <file.yip>        Show container metadata
yote stats                  Throughput and capacity info
yote test <message>         Opus roundtrip verification

yote tx <file> --via ...    Stream data over transport
yote rx --via ... [-o dir]  Receive streamed data
yote link --via ...         Bidirectional channel
yote install -p PORT        Start persistent listener
```

**Transports:** `port:HOST:PORT` (TCP), `pipe` (stdin/stdout).

## Documentation

- [Protocol specification](docs/protocol.md) — modulation, framing, wire format
- [yawp decoder](docs/yawp.md) — neural architecture, training, benchmarks
- [Bitrate tiers](docs/bitrate-tiers.md) — per-bitrate configurations

## License

MIT

---

*coyote was developed with [Hermes Agent](https://github.com/NousResearch/hermes-agent) — from parameter optimization through neural decoder training to shipping package, orchestrated across 15,736 sweep trials and 50+ training experiments.*
