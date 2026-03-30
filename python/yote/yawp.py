"""yawp — two-stage gated neural decoder, fully in Rust.

The FFT decoder and 46K-parameter corrector MLP are both compiled into the
native extension. No numpy, no torch, no external weights file needed.
"""

from yote._yote import yawp_correct, yawp_decode, yawp_symbols_to_bits

N_BINS = 79
BITS_PER_BIN = 2
N_DATA_BITS = 158


def decode_frame(pcm, bitrate_idx=2, corrected=True):
    """Decode one PCM frame using yawp.

    Args:
        pcm: list of 960 float samples.
        bitrate_idx: bitrate tier index (0-4, default 2 = 128kbps).
        corrected: if True, apply neural correction. If False, FFT-only.

    Returns: bits [158] as list of floats (0.0 / 1.0).
    """
    if corrected:
        return yawp_correct(list(pcm), bitrate_idx)
    else:
        symbols, _, _ = yawp_decode(list(pcm))
        return yawp_symbols_to_bits(symbols, BITS_PER_BIN)
