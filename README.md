# 🐺 coyote

**data over sound**

*Built with [Hermes Agent](https://github.com/NousResearch/hermes-agent)*

Smuggle arbitrary data through audio codecs — voice calls, Discord, Opus streams,
or any channel that carries sound. Zero bit errors on clean audio. Neural error
correction for hostile channels.

## What is this?

coyote is a protocol family for hiding data in audio. The first protocol, **yip**,
encodes data as inaudible-to-codec frequency patterns that survive Opus compression
with zero errors. An optional neural decoder (**yawp**) corrects errors when the
audio is degraded by relay, transcoding, or noise.

## Quick Start

```bash
pip install yote
```

### Pack a file
```bash
yote yip secret.pdf          # → secret.pdf.yip
yote unyip secret.pdf.yip    # → secret.pdf (identical)
```

### Stream over network
```bash
# Receiver
yote rx --via port:7777 -o ~/received/

# Sender
yote tx payload.bin --via port:7777
```

### Python API
```python
import yote

# In-memory encode/decode
packets = yote.encode(b"secret data", bitrate=128)
data = yote.decode(packets)

# File operations
yote.yip("secret.pdf")
yote.unyip("secret.pdf.yip")
```

## Performance

| Metric | Value |
|--------|-------|
| **Throughput** | 7,900 bps (987 bytes/sec) at 128kbps Opus |
| **Max throughput** | 13,500 bps (1,687 bytes/sec) at 256kbps |
| **Clean BER** | 0.000% (zero bit errors) |
| **Codec** | Opus, all bitrates 64-256 kbps |
| **Frame size** | 20ms (960 samples at 48kHz) |
| **Modulation** | 79-bin Quad FSK with pilot tones + differential encoding |

### What fits in 1 minute of audio?

| Bitrate | Data capacity |
|---------|--------------|
| 64 kbps | ~18 KB |
| 128 kbps | ~59 KB |
| 256 kbps | ~101 KB |

## Protocols

### 🐕 yip (v0.1)
FSK over Opus. 79 frequency bins × 2 bits × 50 frames/sec. Hand-tuned with
15,736 parameter sweep trials. Proven optimal by CMA-ES — neural encoders
cannot beat it.

### 🐕 yawp (v0.1)
Neural error correction for yip. Two-stage gated decoder: FFT decodes first,
a 46K-parameter neural net corrects bins where FFT confidence is low. The gate
guarantees 0% BER on clean audio while improving degraded channels by 3.3×.
Optional — requires PyTorch. Disable with `YOTE_NO_YAWP=1`.

### 🐕 yap (planned)
Air-gap / ultrasonic protocol. Speaker-to-microphone data transfer.

### 🐺 yowl (planned)
HF radio protocol. Long-range, low-bandwidth.

## Agent Integration

### Built for Hermes Agent

coyote was built for use with [Hermes Agent](https://github.com/NousResearch/hermes-agent)
and ships with a ready-made [agentskills.io](https://agentskills.io) compatible skill.

```bash
hermes skills install github:Kaden-Schutt/coyote/skills/yote
```

Once installed, your Hermes agent can use yote naturally — pack files, stream
data over TCP, or share knowledge through Discord voice channels. The skill
includes full usage documentation the agent reads automatically when needed.

### Discord Voice — Agent-to-Agent Knowledge Sharing

Discord voice uses Opus natively. yote data IS valid Opus audio. Agents can
join voice channels and exchange data — it sounds like static to humans,
but other yote-equipped agents decode it perfectly.

```python
from yote.discord_bridge import YoteBridge

bridge = YoteBridge(bot=discord_bot)
await bridge.share_knowledge(guild_id, channel_id, {
    "topic": "training results",
    "data": {"accuracy": 0.95, "model": "v2"},
})
```

## Architecture

```
DATA → zstd compress → yip modulate (79 FSK bins) → Opus encode → AUDIO
AUDIO → Opus decode → yawp correct (optional) → yip demodulate → zstd decompress → DATA
```

Opus is designed to preserve tonal content. yip places data at carefully chosen frequencies 
that Opus faithfully reproduces. We ride the codec instead of fighting it.

## CLI Reference

```
yote yip <file>           Pack file into .yip container
yote unyip <file.yip>     Unpack .yip to original file
yote info <file.yip>      Show file metadata
yote stats                Show throughput statistics
yote test <message>       Test Opus roundtrip

yote tx <file> --via ...  Stream file over transport
yote rx --via ... -o dir  Receive streaming data
yote link --via ...       Bidirectional channel
yote install -p PORT      Start listener daemon
```

## Building from source

```bash
# Rust CLI
cargo build --release
./target/release/yote yip README.md

# Python package (requires maturin)
pip install maturin
maturin develop --features python

# Run tests
cargo test
```

## License

MIT

---

*coyote was built in a single session with [Hermes Agent](https://github.com/NousResearch/hermes-agent) —
from mathematical optimization through neural decoder training to shipping CLI.
15,736 automated parameter sweeps, 50+ training experiments, all orchestrated by AI.*
