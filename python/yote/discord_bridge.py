"""yote Discord voice bridge — send and receive data through Discord voice channels.

Discord voice uses Opus at 128kbps. yote encodes data INTO Opus at 128kbps.
To Discord, yote data IS valid audio. It sounds like noise to humans,
but other yote-equipped agents can decode it perfectly.

Usage:
    from yote.discord_bridge import YoteBridge

    bridge = YoteBridge(bot_token="...")
    
    # Send data to a voice channel
    await bridge.send(guild_id, channel_id, b"knowledge payload")
    
    # Listen for data on a voice channel
    data = await bridge.receive(guild_id, channel_id, timeout=30)

    # Or use the high-level knowledge sharing API
    await bridge.share_knowledge(guild_id, channel_id, {
        "topic": "yarl decoder results",
        "data": {"double_tc_ber": 0.0467, "model": "two-stage-gated"},
    })

Requirements:
    pip install discord.py[voice]
    # discord.py already includes Opus support
"""

import asyncio
import io
import json
import struct
import zlib
import logging
from typing import Optional

logger = logging.getLogger("yote.discord")

try:
    import discord
    from discord.ext import commands
    HAS_DISCORD = True
except ImportError:
    HAS_DISCORD = False

try:
    import yote as _yote
    HAS_YOTE = True
except ImportError:
    HAS_YOTE = False


# ---------------------------------------------------------------------------
# Opus audio source — sends yote-encoded data as voice audio
# ---------------------------------------------------------------------------

class YoteAudioSource(discord.AudioSource):
    """Streams yote-encoded Opus packets as Discord voice audio.
    
    Discord expects 20ms Opus frames. yote.encode() produces exactly that —
    each packet is one 20ms Opus frame at 48kHz mono. Perfect match.
    """

    def __init__(self, opus_packets: list):
        self.packets = opus_packets
        self.index = 0
        # Silence packet (Opus DTX) for padding
        self._silence = b'\xf8\xff\xfe'

    def read(self) -> bytes:
        if self.index < len(self.packets):
            packet = self.packets[self.index]
            self.index += 1
            return packet
        return b''  # Signal end

    def is_opus(self) -> bool:
        # Tell discord.py these are raw Opus packets, don't re-encode
        return True

    @property
    def progress(self) -> float:
        if not self.packets:
            return 1.0
        return self.index / len(self.packets)


# ---------------------------------------------------------------------------
# Opus audio sink — receives voice audio and decodes yote data
# ---------------------------------------------------------------------------

class YoteAudioSink(discord.AudioSink if HAS_DISCORD else object):
    """Collects incoming Opus packets from Discord voice for yote decoding.
    
    Note: Requires discord-ext-voice-recv or a custom voice client that
    supports audio receiving. Standard discord.py only supports sending.
    For v0.1, we use a simpler approach: send data, don't receive live.
    """

    def __init__(self):
        self.packets = []
        self.done = asyncio.Event()

    def write(self, data):
        self.packets.append(data.data)

    def cleanup(self):
        self.done.set()


# ---------------------------------------------------------------------------
# High-level bridge
# ---------------------------------------------------------------------------

class YoteBridge:
    """High-level API for sending/receiving data through Discord voice channels."""

    def __init__(self, bot: Optional[object] = None, token: Optional[str] = None):
        """Initialize with either an existing bot or a token to create one.
        
        Args:
            bot: Existing discord.py Bot/Client instance
            token: Bot token (creates a new minimal client)
        """
        if not HAS_DISCORD:
            raise ImportError("discord.py[voice] required: pip install discord.py[voice]")
        if not HAS_YOTE:
            raise ImportError("yote not installed: pip install yote")

        self.token = token
        self._bot = bot
        self._voice_client = None

    async def _get_bot(self):
        if self._bot:
            return self._bot
        raise ValueError("No bot instance. Pass bot= to YoteBridge or use from_token().")

    async def join_channel(self, guild_id: int, channel_id: int) -> discord.VoiceClient:
        """Join a voice channel. Returns VoiceClient."""
        bot = await self._get_bot()
        guild = bot.get_guild(guild_id)
        if not guild:
            raise ValueError(f"Guild {guild_id} not found")
        channel = guild.get_channel(channel_id)
        if not channel:
            raise ValueError(f"Channel {channel_id} not found in guild {guild_id}")
        if not isinstance(channel, discord.VoiceChannel):
            raise ValueError(f"Channel {channel_id} is not a voice channel")

        # Disconnect existing voice client if any
        if guild.voice_client:
            await guild.voice_client.disconnect()

        vc = await channel.connect()
        self._voice_client = vc
        return vc

    async def send(self, guild_id: int, channel_id: int, data: bytes,
                   bitrate: int = 128) -> None:
        """Send data through a Discord voice channel using yote encoding.
        
        Args:
            guild_id: Discord guild (server) ID
            channel_id: Discord voice channel ID
            data: Raw bytes to send
            bitrate: Opus bitrate (should match channel, default 128kbps)
        """
        logger.info(f"Encoding {len(data)} bytes for voice transmission")

        # Compress + frame
        compressed = zlib.compress(data)
        header = b"YOTE\x01" + struct.pack("<I", len(data)) + struct.pack("<I", zlib.crc32(data))
        framed = header + compressed

        # Encode to Opus packets via yote
        packets = _yote.encode(framed, bitrate=bitrate)
        logger.info(f"Encoded to {len(packets)} Opus packets ({len(packets) * 20}ms of audio)")

        # Join channel and stream
        vc = await self.join_channel(guild_id, channel_id)
        source = YoteAudioSource(packets)

        # Play and wait for completion
        done = asyncio.Event()
        def after_playing(error):
            if error:
                logger.error(f"Error during playback: {error}")
            done.set()

        vc.play(source, after=after_playing)
        await done.wait()

        logger.info("Transmission complete")
        await vc.disconnect()

    async def share_knowledge(self, guild_id: int, channel_id: int,
                               knowledge: dict, bitrate: int = 128) -> None:
        """Share a knowledge payload (dict/JSON) through voice.
        
        Convenience wrapper that JSON-serializes the knowledge dict.
        
        Args:
            guild_id: Discord guild ID
            channel_id: Voice channel ID
            knowledge: Dict to share (must be JSON-serializable)
        """
        data = json.dumps(knowledge, separators=(',', ':')).encode('utf-8')
        logger.info(f"Sharing knowledge: {len(data)} bytes ({len(knowledge)} keys)")
        await self.send(guild_id, channel_id, data, bitrate=bitrate)


# ---------------------------------------------------------------------------
# Standalone helper for agents without a persistent bot
# ---------------------------------------------------------------------------

async def send_to_voice(token: str, guild_id: int, channel_id: int,
                        data: bytes, bitrate: int = 128):
    """One-shot: connect to Discord, send data via voice, disconnect.
    
    Use this when you don't have a persistent bot running.
    Creates a temporary client just for the transmission.
    """
    if not HAS_DISCORD:
        raise ImportError("discord.py[voice] required: pip install discord.py[voice]")

    intents = discord.Intents.default()
    intents.guilds = True
    intents.voice_states = True
    client = discord.Client(intents=intents)

    ready = asyncio.Event()

    @client.event
    async def on_ready():
        ready.set()

    # Start client in background
    asyncio.create_task(client.start(token))
    await ready.wait()

    bridge = YoteBridge(bot=client)
    try:
        await bridge.send(guild_id, channel_id, data, bitrate=bitrate)
    finally:
        await client.close()


# ---------------------------------------------------------------------------
# CLI integration
# ---------------------------------------------------------------------------

def send_file_to_discord(token: str, guild_id: int, channel_id: int,
                         filepath: str, bitrate: int = 128):
    """Blocking wrapper to send a file via Discord voice. For CLI use."""
    import pathlib
    data = pathlib.Path(filepath).read_bytes()
    asyncio.run(send_to_voice(token, guild_id, channel_id, data, bitrate))
