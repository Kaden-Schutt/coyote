/// Differential Encoding — encode deltas between frames instead of absolutes.
///
/// Technique 2 from PLAN-MATH-STACK.md. Stacks on top of Pilot Tone Interpolation.
///
/// Key insight: Opus preserves temporal continuity — smooth transitions between
/// frames sound good. The DIFFERENCE between consecutive frames is preserved more
/// faithfully than absolute values. This means we can use more amplitude/phase
/// levels in the differential domain than in the absolute domain.
///
/// Architecture:
///   Frame 0: calibration frame (all symbols = midpoint, known baseline)
///   Frame 1: Δ₁ = symbols₁ - baseline  (encoded as delta symbols)
///   Frame N: Δₙ = symbolsₙ - symbolsₙ₋₁
///   Every `recal_interval` frames: re-emit calibration to prevent drift
///
/// The delta symbols are clamped to [-delta_range, +delta_range] and mapped to
/// the pilot's amp/phase constellation. The key question: does this unlock
/// more levels (16 amp? 4 phase?) than absolute encoding?

use crate::pilot::{
    bytes_to_pilot_symbols, demodulate_pilot_frame, modulate_pilot_frame, pilot_symbols_to_bytes,
    PilotConfig,
};

// ── Configuration ────────────────────────────────────────────────

/// Differential encoding mode — what gets delta-encoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffMode {
    /// Delta-encode amplitude only, absolute phase
    AmpOnly,
    /// Delta-encode phase only, absolute amplitude
    PhaseOnly,
    /// Delta-encode both amplitude and phase
    Both,
}

#[derive(Debug, Clone)]
pub struct DiffConfig {
    /// Underlying pilot config (defines bins, levels, spacing, etc.)
    pub pilot: PilotConfig,
    /// How many frames between re-calibration frames (0 = never recalibrate)
    pub recal_interval: usize,
    /// What to delta-encode
    pub diff_mode: DiffMode,
}

impl DiffConfig {
    /// Effective throughput accounts for calibration frame overhead.
    /// Every recal_interval+1 frames, 1 is calibration (carries no data).
    /// If recal_interval=0, only the first frame is calibration.
    pub fn effective_throughput_bps(&self) -> usize {
        let base = self.pilot.throughput_bps();
        if self.recal_interval == 0 {
            // Only initial calibration, amortized over many frames — negligible
            base
        } else {
            // Every recal_interval data frames, 1 calibration frame
            // Ratio: recal_interval / (recal_interval + 1)
            base * self.recal_interval / (self.recal_interval + 1)
        }
    }

    pub fn effective_throughput_bytes(&self) -> usize {
        self.effective_throughput_bps() / 8
    }
}

// ── Helpers ──────────────────────────────────────────────────────

/// Calibration symbol: midpoint of the constellation.
fn calibration_symbol(amp_levels: usize, phase_levels: usize) -> (usize, usize) {
    let amp_mid = if amp_levels > 1 {
        amp_levels / 2
    } else {
        0
    };
    let phase_mid = if phase_levels > 1 {
        phase_levels / 2
    } else {
        0
    };
    (amp_mid, phase_mid)
}

/// Encode a data symbol as a delta from the previous symbol.
/// The delta is wrapped modularly within the level range.
/// Returns the symbol to actually transmit (which represents the delta).
fn encode_delta(
    current: (usize, usize),
    previous: (usize, usize),
    amp_levels: usize,
    phase_levels: usize,
    mode: DiffMode,
) -> (usize, usize) {
    let amp_sym = match mode {
        DiffMode::PhaseOnly => current.0,
        _ => {
            // Delta = current - previous, wrapped into [0, amp_levels)
            let delta = (current.0 as isize - previous.0 as isize)
                .rem_euclid(amp_levels as isize) as usize;
            delta
        }
    };

    let phase_sym = match mode {
        DiffMode::AmpOnly => current.1,
        _ => {
            let delta = (current.1 as isize - previous.1 as isize)
                .rem_euclid(phase_levels as isize) as usize;
            delta
        }
    };

    (amp_sym, phase_sym)
}

/// Decode a delta symbol back to absolute, given the previous symbol.
fn decode_delta(
    received: (usize, usize),
    previous: (usize, usize),
    amp_levels: usize,
    phase_levels: usize,
    mode: DiffMode,
) -> (usize, usize) {
    let amp_sym = match mode {
        DiffMode::PhaseOnly => received.0,
        _ => {
            // Absolute = previous + delta, wrapped
            (previous.0 + received.0) % amp_levels
        }
    };

    let phase_sym = match mode {
        DiffMode::AmpOnly => received.1,
        _ => (previous.1 + received.1) % phase_levels,
    };

    (amp_sym, phase_sym)
}

// ── Multi-frame differential encode/decode ───────────────────────

/// Encode multiple frames of symbols using differential encoding.
///
/// Input: Vec of frames, each frame is Vec<(amp_sym, phase_sym)> with data_bins entries.
/// Output: Vec of frames to transmit (including calibration frames).
///
/// Calibration frames are inserted at position 0 and every recal_interval frames.
/// They contain the calibration symbol repeated for all data bins.
fn diff_encode_frames(
    data_frames: &[Vec<(usize, usize)>],
    diff_cfg: &DiffConfig,
) -> Vec<Vec<(usize, usize)>> {
    let amp_levels = diff_cfg.pilot.amp_levels;
    let phase_levels = diff_cfg.pilot.phase_levels;
    let data_bins = diff_cfg.pilot.data_bins();
    let cal_sym = calibration_symbol(amp_levels, phase_levels);
    let cal_frame: Vec<(usize, usize)> = vec![cal_sym; data_bins];

    let mut output_frames = Vec::new();
    let mut prev_frame: Vec<(usize, usize)> = cal_frame.clone();
    let mut frames_since_cal = 0;

    // Initial calibration frame
    output_frames.push(cal_frame.clone());

    for data_frame in data_frames {
        // Check if we need a recalibration frame
        if diff_cfg.recal_interval > 0 && frames_since_cal >= diff_cfg.recal_interval {
            output_frames.push(cal_frame.clone());
            prev_frame = cal_frame.clone();
            frames_since_cal = 0;
        }

        // Encode deltas
        let delta_frame: Vec<(usize, usize)> = data_frame
            .iter()
            .zip(prev_frame.iter())
            .map(|(&curr, &prev)| encode_delta(curr, prev, amp_levels, phase_levels, diff_cfg.diff_mode))
            .collect();

        output_frames.push(delta_frame);
        prev_frame = data_frame.clone();
        frames_since_cal += 1;
    }

    output_frames
}

/// Decode differentially-encoded frames back to absolute symbols.
///
/// Input: Vec of received frames (including calibration frames).
/// Output: Vec of data frames (calibration frames removed).
fn diff_decode_frames(
    received_frames: &[Vec<(usize, usize)>],
    diff_cfg: &DiffConfig,
) -> Vec<Vec<(usize, usize)>> {
    let amp_levels = diff_cfg.pilot.amp_levels;
    let phase_levels = diff_cfg.pilot.phase_levels;
    let data_bins = diff_cfg.pilot.data_bins();
    let cal_sym = calibration_symbol(amp_levels, phase_levels);
    let cal_frame: Vec<(usize, usize)> = vec![cal_sym; data_bins];

    let mut output_frames = Vec::new();
    let mut prev_frame: Vec<(usize, usize)> = cal_frame.clone();
    let mut frames_since_cal = 0;
    let mut is_first = true;

    for received in received_frames {
        if is_first {
            // First frame is always calibration — skip it, set baseline
            prev_frame = cal_frame.clone();
            is_first = false;
            frames_since_cal = 0;
            continue;
        }

        // Check if this is a recalibration frame
        if diff_cfg.recal_interval > 0 && frames_since_cal >= diff_cfg.recal_interval {
            // This frame is calibration — reset baseline, skip
            prev_frame = cal_frame.clone();
            frames_since_cal = 0;
            continue;
        }

        // Decode deltas to absolute
        let abs_frame: Vec<(usize, usize)> = received
            .iter()
            .zip(prev_frame.iter())
            .map(|(&recv, &prev)| decode_delta(recv, prev, amp_levels, phase_levels, diff_cfg.diff_mode))
            .collect();

        prev_frame = abs_frame.clone();
        output_frames.push(abs_frame);
        frames_since_cal += 1;
    }

    output_frames
}

// ── Full Opus roundtrip with differential encoding ───────────────

/// Full differential + pilot Opus roundtrip:
///   data → frame → symbols → split into frame chunks → diff encode (insert cals)
///   → modulate each with pilots → concat mono PCM → Opus roundtrip
///   → demodulate each frame → diff decode (remove cals) → symbols → bytes → deframe
pub fn diff_pilot_opus_roundtrip(
    data: &[u8],
    diff_cfg: &DiffConfig,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pilot_cfg = &diff_cfg.pilot;
    let config = &pilot_cfg.config;
    let data_bins = pilot_cfg.data_bins();

    // Frame the data (adds sync magic, length, CRC)
    let framed = crate::framing::frame(data, config);

    // Convert framed bytes to symbols
    let all_symbols = bytes_to_pilot_symbols(&framed, pilot_cfg);

    // Split into per-frame symbol chunks
    let symbol_frames: Vec<Vec<(usize, usize)>> = all_symbols
        .chunks(data_bins)
        .map(|chunk| {
            let mut frame = chunk.to_vec();
            frame.resize(data_bins, (0, 0));
            frame
        })
        .collect();

    // Differential encode (inserts calibration frames)
    let tx_frames = diff_encode_frames(&symbol_frames, diff_cfg);

    // Build mono PCM
    let mut pcm = Vec::new();
    let silence = vec![0.0f32; config.frame_samples];

    for _ in 0..config.padding_frames {
        pcm.extend(&silence);
    }
    for frame_syms in &tx_frames {
        pcm.extend(modulate_pilot_frame(frame_syms, pilot_cfg));
    }
    for _ in 0..config.trailing_frames {
        pcm.extend(&silence);
    }

    // MONO Opus roundtrip
    let decoded_pcm = crate::opus::opus_roundtrip(&pcm, config)?;

    // Demodulate: try different frame offsets to find sync
    let n = config.frame_samples;
    let num_frames = decoded_pcm.len() / n;
    let expected_tx_frames = tx_frames.len();

    for offset in 0..config.max_search_offset.min(num_frames) {
        // Demodulate all available frames from this offset
        let mut rx_frames: Vec<Vec<(usize, usize)>> = Vec::new();
        for i in offset..num_frames {
            let start = i * n;
            if start + n > decoded_pcm.len() {
                break;
            }
            let frame_pcm = &decoded_pcm[start..start + n];
            rx_frames.push(demodulate_pilot_frame(frame_pcm, pilot_cfg));
        }

        if rx_frames.len() < expected_tx_frames {
            continue;
        }

        // Try diff-decoding with the expected number of tx frames
        let rx_subset = &rx_frames[..expected_tx_frames];
        let decoded_frames = diff_decode_frames(rx_subset, diff_cfg);

        // Flatten symbols and try to deframe
        let all_demod_symbols: Vec<(usize, usize)> =
            decoded_frames.into_iter().flatten().collect();
        let raw_bytes = pilot_symbols_to_bytes(&all_demod_symbols, pilot_cfg);

        if let Ok(payload) = crate::framing::deframe(&raw_bytes, config) {
            return Ok(payload);
        }
    }

    Err("Differential decode failed: sync not found".into())
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::pilot::InterpMethod;

    fn base_pilot_config() -> PilotConfig {
        PilotConfig {
            config: Config {
                opus_bitrate: 128,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            amp_levels: 2,
            phase_levels: 1,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        }
    }

    fn test_diff_config() -> DiffConfig {
        DiffConfig {
            pilot: base_pilot_config(),
            recal_interval: 10,
            diff_mode: DiffMode::Both,
        }
    }

    #[test]
    fn delta_encode_decode_roundtrip() {
        let amp_levels = 4;
        let phase_levels = 2;
        let mode = DiffMode::Both;

        let prev = (1, 0);
        let current = (3, 1);

        let delta = encode_delta(current, prev, amp_levels, phase_levels, mode);
        let recovered = decode_delta(delta, prev, amp_levels, phase_levels, mode);
        assert_eq!(recovered, current);
    }

    #[test]
    fn delta_wrap_around() {
        let amp_levels = 4;
        let phase_levels = 2;
        let mode = DiffMode::Both;

        // current=0, prev=3 -> delta = -3 mod 4 = 1
        let prev = (3, 1);
        let current = (0, 0);

        let delta = encode_delta(current, prev, amp_levels, phase_levels, mode);
        let recovered = decode_delta(delta, prev, amp_levels, phase_levels, mode);
        assert_eq!(recovered, current);
    }

    #[test]
    fn diff_frame_encode_decode_no_recal() {
        let pilot = PilotConfig {
            config: Config {
                opus_bitrate: 128,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            amp_levels: 4,
            phase_levels: 1,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        };
        let diff_cfg = DiffConfig {
            pilot: pilot.clone(),
            recal_interval: 0, // no recalibration
            diff_mode: DiffMode::AmpOnly,
        };

        let data_bins = pilot.data_bins();
        let frames: Vec<Vec<(usize, usize)>> = (0..5)
            .map(|f| {
                (0..data_bins)
                    .map(|i| ((i + f) % 4, 0))
                    .collect()
            })
            .collect();

        let encoded = diff_encode_frames(&frames, &diff_cfg);
        // First frame is calibration + 5 data frames = 6 total
        assert_eq!(encoded.len(), 6);

        let decoded = diff_decode_frames(&encoded, &diff_cfg);
        assert_eq!(decoded.len(), 5);
        assert_eq!(decoded, frames);
    }

    #[test]
    fn diff_frame_encode_decode_with_recal() {
        let pilot = PilotConfig {
            config: Config {
                opus_bitrate: 128,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            amp_levels: 4,
            phase_levels: 2,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        };
        let diff_cfg = DiffConfig {
            pilot: pilot.clone(),
            recal_interval: 3,
            diff_mode: DiffMode::Both,
        };

        let data_bins = pilot.data_bins();
        let frames: Vec<Vec<(usize, usize)>> = (0..10)
            .map(|f| {
                (0..data_bins)
                    .map(|i| ((i + f) % 4, (i + f) % 2))
                    .collect()
            })
            .collect();

        let encoded = diff_encode_frames(&frames, &diff_cfg);
        let decoded = diff_decode_frames(&encoded, &diff_cfg);
        assert_eq!(decoded.len(), frames.len());
        assert_eq!(decoded, frames);
    }

    #[test]
    fn diff_opus_roundtrip_binary() {
        let diff_cfg = test_diff_config();
        let data = b"Diff test!";
        let recovered = diff_pilot_opus_roundtrip(data, &diff_cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn diff_opus_roundtrip_4amp() {
        // Test with 4 amplitude levels — same as absolute pilot
        let diff_cfg = DiffConfig {
            pilot: PilotConfig {
                config: Config {
                    opus_bitrate: 256,
                    ..Config::conservative()
                },
                pilot_spacing: 5,
                amp_levels: 4,
                phase_levels: 1,
                pilot_amplitude: 1.0,
                interp_method: InterpMethod::Linear,
            },
            recal_interval: 10,
            diff_mode: DiffMode::AmpOnly,
        };
        let data = b"4-level diff";
        let recovered = diff_pilot_opus_roundtrip(data, &diff_cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn diff_opus_roundtrip_8amp_2phase() {
        // The pilot winner config — does diff maintain it?
        let diff_cfg = DiffConfig {
            pilot: PilotConfig {
                config: Config {
                    n_bins: 70,
                    bin_spacing: 200.0,
                    base_freq: 200.0,
                    opus_bitrate: 256,
                    ..Config::default()
                },
                pilot_spacing: 8,
                amp_levels: 8,
                phase_levels: 2,
                pilot_amplitude: 1.0,
                interp_method: InterpMethod::NearestNeighbor,
            },
            recal_interval: 20,
            diff_mode: DiffMode::Both,
        };
        let data = b"8amp 2phase diff";
        let recovered = diff_pilot_opus_roundtrip(data, &diff_cfg).unwrap();
        assert_eq!(recovered, data);
    }
}
