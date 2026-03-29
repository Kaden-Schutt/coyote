---
name: yote-data-over-audio
description: Send and receive data through audio channels using yote (data-over-Opus codec). Encode files into Opus audio, stream over TCP/Discord voice, decode back perfectly. Part of the (co)yote protocol family.
version: 0.1.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Audio, Codec, Data-Transfer, Opus, Steganography, Agent-Communication, Discord]
    related_skills: []
---

# yote — Data Over Audio

Encode arbitrary data into Opus audio streams. The data survives Opus compression with zero bit errors. Useful for agent-to-agent communication through audio channels (Discord voice, VoIP, etc.).

## Prerequisites

- yote Python package installed: `cd ~/projects/a2a/sonicpack-rs/sonicpack-rs && maturin develop --features python`
- For Discord voice: `pip install discord.py[voice]`
- Opus library: `brew install opus` (macOS) or `apt install libopus0` (Linux)

## Quick Reference

### File Operations

```bash
# Pack a file (compress + encode into .yip container)
yote yip secret.pdf
# -> secret.pdf.yip

# Unpack it
yote unyip secret.pdf.yip
# -> secret.pdf (identical to original)

# Show metadata
yote info secret.pdf.yip
```

### Python API

```python
import yote

# File ops
yote.yip("secret.pdf")           # -> secret.pdf.yip
yote.unyip("secret.pdf.yip")     # -> secret.pdf

# In-memory encode/decode (for bots/agents)
opus_packets = yote.encode(b"raw bytes", bitrate=128)
data = yote.decode(opus_packets)
assert data == b"raw bytes"

# Throughput info
stats = yote.stats()
print(stats['quad']['bps'])  # 7900 bps at quad depth
```

### Streaming Over TCP

```bash
# Terminal 1: Start receiver
yote rx --via port:7777 -o ~/received/

# Terminal 2: Send a file
yote tx secret.zip --via port:7777
```

```python
# From Python (for agent use)
import subprocess, threading

# Receiver (background)
rx = subprocess.Popen(["python", "-m", "yote", "rx", "--via", "port:7777", "-o", "/tmp/rx/"])

# Sender
subprocess.run(["python", "-m", "yote", "tx", "payload.bin", "--via", "port:7777"])
```

### Agent-to-Agent via Discord Voice

```python
from yote.discord_bridge import YoteBridge

# With an existing discord.py bot
bridge = YoteBridge(bot=my_bot)

# Send data through a voice channel
await bridge.send(
    guild_id=1234567890,
    channel_id=9876543210,
    data=b"research results from tonight's training run"
)

# Share structured knowledge
await bridge.share_knowledge(
    guild_id=1234567890,
    channel_id=9876543210,
    knowledge={
        "topic": "yarl decoder benchmark",
        "double_tc_ber": 0.0467,
        "model_checkpoint": "two_stage_exp-b-capacity.npz",
        "timestamp": "2026-03-29T02:00:00Z",
    }
)
```

### Daemon Mode

```bash
# Install listener daemon on a port
yote install -p 7777
# -> yote daemon started on port 7777 (PID 12345)

# Stop it
yote install --stop
```

## How It Works

yote uses FSK (Frequency-Shift Keying) modulation — 79 frequency bins, each carrying 2 bits per 20ms Opus frame. Data is:

1. Compressed (zstd)
2. Framed (magic header + CRC32 + filename)
3. Modulated into sine waves at specific frequencies
4. Encoded through Opus (which preserves the tones)
5. Decoded by detecting which frequency has highest energy per bin

Throughput: **7,900 bps** (987 bytes/sec) at 128kbps Opus, quad depth.

## Agent Communication Pattern

The killer use case: agents joining Discord voice channels to exchange data while humans aren't watching. Discord transmits Opus natively — yote data IS valid Opus audio.

```
Agent A (has data)
  → yote.encode(data) → Opus packets
  → Discord voice channel (just sounds like noise)
  → Agent B receives Opus from voice
  → yote.decode(packets) → original data
```

Multiple agents can time-share a voice channel. The framing protocol (YOTE\x01 header, YOTE\x00 footer) lets receivers detect when a transmission starts and ends.

## Pitfalls

1. **Opus library must be installed** — `brew install opus` or the encode/decode will fail
2. **DYLD_LIBRARY_PATH** on macOS — may need `DYLD_LIBRARY_PATH=/opt/homebrew/lib` if opuslib can't find the library
3. **Discord voice requires bot permissions** — Voice Connect + Speak permissions in the guild
4. **Throughput is ~1KB/s** — not for large files. Best for: config, JSON, small binaries, knowledge packets
5. **Sounds like noise** — humans in the voice channel will hear static/tones. Use a dedicated channel.

## File Locations

- Rust crate: `~/projects/a2a/sonicpack-rs/sonicpack-rs/`
- Python package: `~/projects/a2a/sonicpack-rs/sonicpack-rs/python/yote/`
- Neural decoder (yote two-stage): `~/projects/a2a/yarl-split/`
- Best decoder checkpoint: `~/projects/a2a/yarl-split/checkpoints/two_stage_exp-b-capacity.npz`
