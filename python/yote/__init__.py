"""yote — data-over-audio codec. Smuggle data through Opus audio channels."""

import os

from yote._yote import yip, unyip, info, encode, decode, encode_frame, decode_frame, stats

try:
    if os.environ.get("YOTE_NO_YAWP") == "1":
        raise ImportError("yawp disabled by YOTE_NO_YAWP")
    from yote.yawp import YawpCorrector, decode_frame as yawp_decode
    HAS_YAWP = True
except ImportError:
    HAS_YAWP = False

__all__ = [
    "yip", "unyip", "info", "encode", "decode", "encode_frame", "decode_frame", "stats",
    "HAS_YAWP",
]
