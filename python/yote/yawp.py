"""yawp — two-stage gated neural decoder (FFT + learned correction).

Ported from MLX (yarl-split) to PyTorch. Falls back to FFT-only if torch
is not installed.
"""

import os
import warnings
import numpy as np

# ---------------------------------------------------------------------------
# Constants (hardcoded from yarl config)
# ---------------------------------------------------------------------------

SAMPLE_RATE = 48000
FRAME_SAMPLES = 960
N_BINS = 79
BIN_SPACING = 250.0
BASE_FREQ = 200.0
DEPTH_LEVELS = 4
BITS_PER_BIN = 2
N_DATA_BITS = 158  # N_BINS * BITS_PER_BIN
N_BITRATES = 5

# ---------------------------------------------------------------------------
# Tone helpers (pure numpy, mirrors yip_teacher.tone_freq)
# ---------------------------------------------------------------------------


def tone_freq(bin_idx, symbol):
    center = BASE_FREQ + bin_idx * BIN_SPACING
    levels = float(DEPTH_LEVELS)
    offset = (symbol - (levels - 1.0) / 2.0) * (BIN_SPACING / (levels + 1.0))
    return center + offset


def symbols_to_bits(symbols):
    """[79] symbols -> [158] bits."""
    bits = np.zeros(N_DATA_BITS, dtype=np.float32)
    for i in range(N_BINS):
        s = int(symbols[i])
        bits[i * BITS_PER_BIN] = (s >> 1) & 1
        bits[i * BITS_PER_BIN + 1] = s & 1
    return bits


# ---------------------------------------------------------------------------
# Stage 1: FFT decode with confidence (numpy)
# ---------------------------------------------------------------------------


def fft_decode_with_confidence(pcm_np):
    """Per-bin DFT magnitude at 4 tone frequencies.

    Returns:
      symbols:      [79] int32
      confidences:  [79] float32
      magnitudes:   [79, 4] float32
    """
    t = np.arange(FRAME_SAMPLES, dtype=np.float64) / SAMPLE_RATE
    symbols = np.zeros(N_BINS, dtype=np.int32)
    confidences = np.zeros(N_BINS, dtype=np.float32)
    magnitudes = np.zeros((N_BINS, DEPTH_LEVELS), dtype=np.float32)

    for bin_idx in range(N_BINS):
        for sym in range(DEPTH_LEVELS):
            freq = tone_freq(bin_idx, sym)
            omega = 2.0 * np.pi * freq
            I = np.sum(pcm_np * np.cos(omega * t))
            Q = np.sum(pcm_np * np.sin(omega * t))
            magnitudes[bin_idx, sym] = I * I + Q * Q

        sorted_mags = np.sort(magnitudes[bin_idx])[::-1]
        symbols[bin_idx] = np.argmax(magnitudes[bin_idx])
        confidences[bin_idx] = sorted_mags[0] / (sorted_mags[1] + 1e-10)

    return symbols, confidences, magnitudes


def batch_fft_features(pcm_batch):
    """Vectorized FFT feature extraction.

    pcm_batch: [N, 960] float32
    Returns: symbols [N,79], confidences [N,79], magnitudes [N,79,4]
    """
    N = pcm_batch.shape[0]
    t = np.arange(FRAME_SAMPLES, dtype=np.float64) / SAMPLE_RATE

    all_freqs = np.zeros((N_BINS, DEPTH_LEVELS), dtype=np.float64)
    for b in range(N_BINS):
        for s in range(DEPTH_LEVELS):
            all_freqs[b, s] = tone_freq(b, s)

    freqs_flat = all_freqs.reshape(-1)
    omega_t = 2.0 * np.pi * freqs_flat[:, None] * t[None, :]
    cos_basis = np.cos(omega_t).astype(np.float32)
    sin_basis = np.sin(omega_t).astype(np.float32)

    I = pcm_batch @ cos_basis.T
    Q = pcm_batch @ sin_basis.T
    energy = (I * I + Q * Q).reshape(N, N_BINS, DEPTH_LEVELS)

    symbols = np.argmax(energy, axis=-1).astype(np.int32)
    sorted_energy = np.sort(energy, axis=-1)[:, :, ::-1]
    confidences = (sorted_energy[:, :, 0] / (sorted_energy[:, :, 1] + 1e-10)).astype(np.float32)

    return symbols, confidences, energy.astype(np.float32)


# ---------------------------------------------------------------------------
# Stage 2: YawpCorrector (PyTorch)
# ---------------------------------------------------------------------------

import torch
import torch.nn as nn
import torch.nn.functional as F


class YawpCorrector(nn.Module):
    """Per-bin gated corrector. Port of LargerCorrector from MLX.

    Input per bin (45 features):
      one-hot FFT symbol (4) + log confidence (1) + normalized magnitudes (4)
      + bitrate embedding (16) + neighbor context ±2 bins × 5 (20)

    Corrector MLP: 45 → 256 → 128 → 4
    Gate MLP:      45 → 32 → 1
    Output: (1 - gate) * fft_onehot + gate * softmax(corrected)
    """

    def __init__(self, n_bins=N_BINS, depth_levels=DEPTH_LEVELS, n_bitrates=N_BITRATES):
        super().__init__()
        self.n_bins = n_bins
        self.depth = depth_levels

        self.bitrate_embed = nn.Embedding(n_bitrates, 16)

        input_dim = 45
        self.fc1 = nn.Linear(input_dim, 256)
        self.fc2 = nn.Linear(256, 128)
        self.fc3 = nn.Linear(128, depth_levels)

        self.gate_fc1 = nn.Linear(input_dim, 32)
        self.gate_fc2 = nn.Linear(32, 1)

    def forward(self, fft_symbols, fft_confidences, magnitudes, bitrate_idx):
        """
        fft_symbols:     [B, 79] long
        fft_confidences: [B, 79] float
        magnitudes:      [B, 79, 4] float
        bitrate_idx:     [B] long

        Returns: (blended [B, 79, 4], gate_values [B, 79, 1])
        """
        B = fft_symbols.shape[0]

        # One-hot FFT symbols: [B, 79, 4]
        fft_onehot = torch.zeros(B, self.n_bins, self.depth,
                                 device=fft_symbols.device, dtype=magnitudes.dtype)
        fft_onehot.scatter_(2, fft_symbols.unsqueeze(-1), 1.0)

        # Log-scaled confidence: [B, 79, 1]
        log_conf = torch.log(fft_confidences + 1e-6).unsqueeze(-1)

        # Normalized magnitudes: [B, 79, 4]
        mag_sum = magnitudes.sum(dim=-1, keepdim=True) + 1e-10
        mag_norm = magnitudes / mag_sum

        # Bitrate embedding: [B, 16] → [B, 79, 16]
        br = self.bitrate_embed(bitrate_idx)
        br = br.unsqueeze(1).expand(B, self.n_bins, 16)

        # Core features: [B, 79, 25]
        core = torch.cat([fft_onehot, log_conf, mag_norm, br], dim=-1)

        # Neighbor context: ±2 bins, each (onehot + conf) = 5
        sym_conf = torch.cat([fft_onehot, log_conf], dim=-1)  # [B, 79, 5]
        padded = F.pad(sym_conf, (0, 0, 2, 2))  # pad bin dim: [B, 83, 5]
        ctx_m2 = padded[:, 0:self.n_bins, :]
        ctx_m1 = padded[:, 1:self.n_bins + 1, :]
        ctx_p1 = padded[:, 3:self.n_bins + 3, :]
        ctx_p2 = padded[:, 4:self.n_bins + 4, :]
        context = torch.cat([ctx_m2, ctx_m1, ctx_p1, ctx_p2], dim=-1)  # [B, 79, 20]

        # Full: [B, 79, 45]
        full = torch.cat([core, context], dim=-1)

        # Corrector MLP
        x = F.relu(self.fc1(full))
        x = F.relu(self.fc2(x))
        corrected_logits = self.fc3(x)

        # Gate MLP
        g = F.relu(self.gate_fc1(full))
        gate_values = torch.sigmoid(self.gate_fc2(g))

        # Blend
        corrected_probs = F.softmax(corrected_logits, dim=-1)
        blended = (1.0 - gate_values) * fft_onehot + gate_values * corrected_probs

        return blended, gate_values

    def load_checkpoint(self, path=None):
        """Load weights from MLX .npz checkpoint."""
        if path is None:
            path = _find_weights()
        if path is None:
            raise FileNotFoundError("No yawp weights found")

        data = np.load(path)
        state = self.state_dict()
        for key in state:
            if key not in data:
                raise KeyError(f"Weight '{key}' not found in checkpoint")
            state[key] = torch.from_numpy(data[key].copy())
        self.load_state_dict(state)


# ---------------------------------------------------------------------------
# Weight discovery
# ---------------------------------------------------------------------------


def _find_weights():
    """Search for weights in priority order."""
    candidates = [
        os.path.expanduser("~/.yote/yawp_weights.npz"),
        os.path.join(os.path.dirname(__file__), "yawp_weights.npz"),
        "/Users/kadenschutt/projects/a2a/yarl-split/checkpoints/two_stage_exp-b-capacity.npz",
    ]
    for p in candidates:
        if os.path.isfile(p):
            return p
    return None


# ---------------------------------------------------------------------------
# TwoStageDecoder wrapper
# ---------------------------------------------------------------------------


class TwoStageDecoder:
    """Full two-stage decoder: FFT + neural correction."""

    def __init__(self, model=None):
        if model is None:
            model = YawpCorrector()
            model.load_checkpoint()
            model.eval()
        self.corrector = model

    @torch.no_grad()
    def decode(self, pcm_np, bitrate_idx=2):
        """Decode one PCM frame. Returns bits [158] float32."""
        symbols, confidences, magnitudes = fft_decode_with_confidence(pcm_np)

        fft_sym = torch.from_numpy(symbols[None, :]).long()
        fft_conf = torch.from_numpy(confidences[None, :]).float()
        mags = torch.from_numpy(magnitudes[None, :]).float()
        br = torch.tensor([bitrate_idx], dtype=torch.long)

        blended, _ = self.corrector(fft_sym, fft_conf, mags, br)
        corrected_symbols = blended[0].argmax(dim=-1).numpy()
        return symbols_to_bits(corrected_symbols)


# ---------------------------------------------------------------------------
# Convenience API
# ---------------------------------------------------------------------------

_default_decoder = None


def decode_frame(pcm, bitrate_idx=2, model=None):
    """Decode one PCM frame using yawp (FFT + neural correction).

    Returns bits [158] as float32 array.
    If model is None, loads default checkpoint (lazy singleton).
    Falls back to FFT-only if PyTorch not available.
    """
    global _default_decoder
    if model is not None:
        dec = TwoStageDecoder(model=model)
    else:
        if _default_decoder is None:
            _default_decoder = TwoStageDecoder()
        dec = _default_decoder
    return dec.decode(pcm, bitrate_idx=bitrate_idx)
