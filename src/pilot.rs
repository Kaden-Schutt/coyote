/// Pilot Tone Interpolation — channel estimation via scattered pilots in MONO Opus.
///
/// Replaces CRAM's dedicated stereo reference channel with pilot tones interleaved
/// among data bins in a single mono stream. Every Nth bin is a pilot at known
/// amplitude/phase; the channel distortion measured at pilots is interpolated to
/// correct data bins.
///
/// Key advantage: works in MONO, eliminating the 50% stereo bandwidth overhead.
/// With pilot_spacing=5, overhead is only 20% (1 in 5 bins is a pilot).
///
/// Technique 1 from PLAN-MATH-STACK.md.

use crate::config::Config;
use std::f32::consts::PI;

// ── Configuration ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterpMethod {
    Linear,
    NearestNeighbor,
}

#[derive(Debug, Clone)]
pub struct PilotConfig {
    /// Base yip config (n_bins, sample_rate, frame_samples, etc.)
    pub config: Config,
    /// Every Nth bin is a pilot (e.g., 5 = every 5th bin: indices 0, 5, 10, ...)
    pub pilot_spacing: usize,
    /// Amplitude levels for data bins (2, 4, 8). Must be power of 2.
    pub amp_levels: usize,
    /// Phase levels for data bins (1, 2, 4, 8). 1 = amplitude-only mode.
    pub phase_levels: usize,
    /// Amplitude of pilot tones (1.0 = same as data max amplitude)
    pub pilot_amplitude: f32,
    /// Interpolation method for channel estimation between pilots
    pub interp_method: InterpMethod,
}

impl PilotConfig {
    /// Indices of pilot bins within the n_bins range.
    /// Pilots are placed at 0, pilot_spacing, 2*pilot_spacing, ...
    pub fn pilot_indices(&self) -> Vec<usize> {
        (0..self.config.n_bins)
            .step_by(self.pilot_spacing)
            .collect()
    }

    /// Indices of data (non-pilot) bins.
    pub fn data_indices(&self) -> Vec<usize> {
        let pilots: std::collections::HashSet<usize> =
            self.pilot_indices().into_iter().collect();
        (0..self.config.n_bins)
            .filter(|i| !pilots.contains(i))
            .collect()
    }

    /// Number of data bins per frame.
    pub fn data_bins(&self) -> usize {
        self.data_indices().len()
    }

    /// Bits per data bin (log2(amp_levels) + log2(phase_levels)).
    pub fn bits_per_bin(&self) -> usize {
        log2(self.amp_levels) + log2(self.phase_levels)
    }

    /// Total data bits per frame.
    pub fn bits_per_frame(&self) -> usize {
        self.data_bins() * self.bits_per_bin()
    }

    /// Bytes per frame (floor division).
    pub fn bytes_per_frame(&self) -> usize {
        self.bits_per_frame() / 8
    }

    /// Throughput in bits per second.
    pub fn throughput_bps(&self) -> usize {
        self.bits_per_frame() * self.config.frames_per_sec()
    }

    /// Throughput in bytes per second.
    pub fn throughput_bytes(&self) -> usize {
        self.throughput_bps() / 8
    }
}

fn log2(n: usize) -> usize {
    assert!(
        n.is_power_of_two() && n >= 1,
        "log2 requires power of 2, got {}",
        n
    );
    n.trailing_zeros() as usize
}

// ── I/Q extraction (same as cram.rs extract_iq) ──────────────────

/// Extract in-phase (I) and quadrature (Q) components at an exact frequency
/// using a single-bin DFT. O(N) per frequency.
fn extract_iq(samples: &[f32], freq: f32, sample_rate: f32) -> (f32, f32) {
    let omega = 2.0 * PI * freq / sample_rate;
    let mut i_sum = 0.0f32;
    let mut q_sum = 0.0f32;
    for (n, &s) in samples.iter().enumerate() {
        let phase = omega * n as f32;
        i_sum += s * phase.cos();
        q_sum += s * phase.sin();
    }
    (i_sum, q_sum)
}

// ── Modulation ───────────────────────────────────────────────────

/// Map an amplitude symbol to a linear amplitude value.
/// Same mapping as CRAM: amp = 0.2 + 0.8 * (sym / (levels-1))
/// Range: [0.2, 1.0] for levels >= 2.
fn amp_sym_to_value(sym: usize, amp_levels: usize) -> f32 {
    if amp_levels <= 1 {
        1.0
    } else {
        0.2 + 0.8 * (sym as f32 / (amp_levels - 1) as f32)
    }
}

/// Map a phase symbol to a phase value in radians.
/// phase = 2π * sym / phase_levels
fn phase_sym_to_value(sym: usize, phase_levels: usize) -> f32 {
    if phase_levels <= 1 {
        0.0
    } else {
        2.0 * PI * (sym as f32 / phase_levels as f32)
    }
}

/// Modulate a single frame with pilot tones and data symbols.
///
/// `symbols` contains one (amp_sym, phase_sym) per DATA bin (not per total bin).
/// Pilot bins get the known pilot_amplitude at phase 0.
/// Data bins get the encoded amplitude and phase.
///
/// Returns `frame_samples` mono PCM samples.
pub fn modulate_pilot_frame(symbols: &[(usize, usize)], pilot_cfg: &PilotConfig) -> Vec<f32> {
    let config = &pilot_cfg.config;
    let n = config.frame_samples;
    let sr = config.sample_rate as f32;
    let data_idx = pilot_cfg.data_indices();
    let pilot_idx = pilot_cfg.pilot_indices();

    assert_eq!(
        symbols.len(),
        data_idx.len(),
        "symbols length ({}) must equal data_bins ({})",
        symbols.len(),
        data_idx.len()
    );

    let mut samples = vec![0.0f32; n];

    // Emit pilot tones: known amplitude, phase = 0
    for &bin in &pilot_idx {
        let freq = config.bin_center(bin);
        let amp = pilot_cfg.pilot_amplitude;
        let omega = 2.0 * PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += amp * (omega * i as f32).sin();
        }
    }

    // Emit data tones: encoded amplitude and phase
    for (sym_i, &bin) in data_idx.iter().enumerate() {
        let (amp_sym, phase_sym) = symbols[sym_i];
        let freq = config.bin_center(bin);
        let amp = amp_sym_to_value(amp_sym, pilot_cfg.amp_levels);
        let phase = phase_sym_to_value(phase_sym, pilot_cfg.phase_levels);
        let omega = 2.0 * PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += amp * (omega * i as f32 + phase).sin();
        }
    }

    // Normalize to [-0.95, 0.95]
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = 0.95 / peak;
        for s in samples.iter_mut() {
            *s *= scale;
        }
    }

    samples
}

// ── Demodulation ─────────────────────────────────────────────────

/// Distortion measured at a single bin: amplitude scale factor and phase offset.
#[derive(Debug, Clone, Copy)]
struct Distortion {
    amp_scale: f32,   // measured_amp / known_amp
    phase_offset: f32, // measured_phase - known_phase (radians)
}

/// Interpolate distortion values from pilot bins to a target bin index.
fn interpolate_distortion(
    target_bin: usize,
    pilot_indices: &[usize],
    pilot_distortions: &[Distortion],
    method: InterpMethod,
) -> Distortion {
    assert_eq!(pilot_indices.len(), pilot_distortions.len());
    assert!(!pilot_indices.is_empty());

    // Find bracketing pilots
    let mut left_i = None;
    let mut right_i = None;

    for (i, &pi) in pilot_indices.iter().enumerate() {
        if pi <= target_bin {
            left_i = Some(i);
        }
        if pi >= target_bin && right_i.is_none() {
            right_i = Some(i);
        }
    }

    match method {
        InterpMethod::NearestNeighbor => {
            // Pick whichever pilot is closest
            let idx = match (left_i, right_i) {
                (Some(l), Some(r)) => {
                    let dl = target_bin - pilot_indices[l];
                    let dr = pilot_indices[r] - target_bin;
                    if dl <= dr { l } else { r }
                }
                (Some(l), None) => l,
                (None, Some(r)) => r,
                (None, None) => 0,
            };
            pilot_distortions[idx]
        }
        InterpMethod::Linear => {
            match (left_i, right_i) {
                (Some(l), Some(r)) if l != r => {
                    // Linear interpolation between two bracketing pilots
                    let pl = pilot_indices[l] as f32;
                    let pr = pilot_indices[r] as f32;
                    let t = (target_bin as f32 - pl) / (pr - pl);
                    let dl = &pilot_distortions[l];
                    let dr = &pilot_distortions[r];
                    Distortion {
                        amp_scale: dl.amp_scale + t * (dr.amp_scale - dl.amp_scale),
                        phase_offset: dl.phase_offset + t * (dr.phase_offset - dl.phase_offset),
                    }
                }
                (Some(l), _) => pilot_distortions[l],
                (_, Some(r)) => pilot_distortions[r],
                (None, None) => pilot_distortions[0],
            }
        }
    }
}

/// Demodulate a single frame using pilot-based channel estimation.
///
/// 1. Extract I/Q at ALL bins (pilots + data)
/// 2. At pilot bins: compute distortion = measured / known
/// 3. Interpolate distortion to data bins
/// 4. Correct data bins and map to symbols
///
/// Returns one (amp_sym, phase_sym) per data bin.
pub fn demodulate_pilot_frame(
    samples: &[f32],
    pilot_cfg: &PilotConfig,
) -> Vec<(usize, usize)> {
    let config = &pilot_cfg.config;
    let sr = config.sample_rate as f32;
    let pilot_idx = pilot_cfg.pilot_indices();
    let data_idx = pilot_cfg.data_indices();

    // Step 1: Extract I/Q at all bins
    let mut amps = vec![0.0f32; config.n_bins];
    let mut phases = vec![0.0f32; config.n_bins];

    for bin in 0..config.n_bins {
        let freq = config.bin_center(bin);
        let (i_val, q_val) = extract_iq(samples, freq, sr);
        amps[bin] = (i_val * i_val + q_val * q_val).sqrt();
        phases[bin] = q_val.atan2(i_val);
    }

    // Step 2: Compute distortion at pilot bins
    // Pilots were emitted at pilot_amplitude, phase = 0
    let known_phase = 0.0f32;
    let mut pilot_distortions = Vec::with_capacity(pilot_idx.len());

    for &pi in &pilot_idx {
        let measured_amp = amps[pi];
        let measured_phase = phases[pi];

        let amp_scale = if pilot_cfg.pilot_amplitude > 1e-9 {
            measured_amp / pilot_cfg.pilot_amplitude
        } else {
            1.0
        };

        let mut phase_offset = measured_phase - known_phase;
        // Normalize to [-π, π]
        while phase_offset > PI {
            phase_offset -= 2.0 * PI;
        }
        while phase_offset < -PI {
            phase_offset += 2.0 * PI;
        }

        pilot_distortions.push(Distortion {
            amp_scale,
            phase_offset,
        });
    }

    // Step 3 & 4: For each data bin, interpolate distortion and decode.
    //
    // The distortion amp_scale tells us: for a tone emitted at amplitude X,
    // we'd measure X * amp_scale after normalization + channel effects.
    // So the corrected amplitude = measured / amp_scale, which recovers the
    // original amplitude value. We normalize by pilot_amplitude (the known
    // reference scale) to get values in [0, 1] range matching the encoding.

    let mut symbols = Vec::with_capacity(data_idx.len());

    for &di in &data_idx {
        let dist = interpolate_distortion(di, &pilot_idx, &pilot_distortions, pilot_cfg.interp_method);

        // Correct amplitude: divide by distortion scale to get original amplitude
        // relative to the pilot. distortion = measured_pilot / pilot_amplitude,
        // so corrected = measured_data / distortion = measured_data * pilot_amplitude / measured_pilot
        let corrected_amp = if dist.amp_scale > 1e-9 {
            amps[di] / dist.amp_scale
        } else {
            0.0
        };

        // The corrected amplitude is now in the same units as the original encoding.
        // Max possible is 1.0 (amp_sym_to_value max), normalize relative to pilot_amplitude.
        let norm_amp = if pilot_cfg.pilot_amplitude > 1e-9 {
            corrected_amp / pilot_cfg.pilot_amplitude
        } else {
            0.0
        };

        // Amplitude symbol (inverse of amp_sym_to_value)
        let amp_sym = if pilot_cfg.amp_levels <= 1 {
            0
        } else {
            let mapped = ((norm_amp - 0.2) / 0.8).clamp(0.0, 1.0);
            let sym = (mapped * (pilot_cfg.amp_levels - 1) as f32).round() as usize;
            sym.min(pilot_cfg.amp_levels - 1)
        };

        // Correct phase: subtract distortion phase offset
        let mut corrected_phase = phases[di] - dist.phase_offset;
        // Normalize to [0, 2π)
        while corrected_phase < 0.0 {
            corrected_phase += 2.0 * PI;
        }
        while corrected_phase >= 2.0 * PI {
            corrected_phase -= 2.0 * PI;
        }

        // Phase symbol
        let phase_sym = if pilot_cfg.phase_levels <= 1 {
            0
        } else {
            let normalized = corrected_phase / (2.0 * PI);
            let sym = (normalized * pilot_cfg.phase_levels as f32).round() as usize;
            sym % pilot_cfg.phase_levels
        };

        symbols.push((amp_sym, phase_sym));
    }

    symbols
}

// ── Symbol packing ───────────────────────────────────────────────

fn bits_to_usize(bits: &[bool]) -> usize {
    bits.iter()
        .fold(0usize, |acc, &b| (acc << 1) | (b as usize))
}

/// Convert bytes to pilot symbols (one per data bin).
/// Each symbol is (amp_sym, phase_sym).
pub fn bytes_to_pilot_symbols(data: &[u8], pilot_cfg: &PilotConfig) -> Vec<(usize, usize)> {
    let bpb = pilot_cfg.bits_per_bin();
    let amp_bits = log2(pilot_cfg.amp_levels);
    let phase_bits = log2(pilot_cfg.phase_levels);

    let bits: Vec<bool> = data
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| (byte >> i) & 1 == 1))
        .collect();

    bits.chunks(bpb)
        .map(|chunk| {
            let mut padded = chunk.to_vec();
            padded.resize(bpb, false);
            let amp_sym = bits_to_usize(&padded[..amp_bits]);
            let phase_sym = if phase_bits > 0 {
                bits_to_usize(&padded[amp_bits..amp_bits + phase_bits])
            } else {
                0
            };
            (amp_sym, phase_sym)
        })
        .collect()
}

/// Convert pilot symbols back to bytes.
pub fn pilot_symbols_to_bytes(
    symbols: &[(usize, usize)],
    pilot_cfg: &PilotConfig,
) -> Vec<u8> {
    let amp_bits = log2(pilot_cfg.amp_levels);
    let phase_bits = log2(pilot_cfg.phase_levels);

    let bits: Vec<bool> = symbols
        .iter()
        .flat_map(|&(amp, phase)| {
            let mut sym_bits = Vec::with_capacity(amp_bits + phase_bits);
            for i in (0..amp_bits).rev() {
                sym_bits.push((amp >> i) & 1 == 1);
            }
            for i in (0..phase_bits).rev() {
                sym_bits.push((phase >> i) & 1 == 1);
            }
            sym_bits
        })
        .collect();

    bits.chunks(8)
        .filter(|c| c.len() == 8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |acc, (i, &b)| acc | ((b as u8) << (7 - i)))
        })
        .collect()
}

// ── Full Opus roundtrip ──────────────────────────────────────────

/// Full pilot-tone Opus roundtrip:
///   data → frame → symbols → modulate with pilots → MONO Opus roundtrip
///   → demodulate with pilots → symbols → bytes → deframe
///
/// Uses mono Opus (not stereo). The pilots replace the reference channel.
pub fn pilot_opus_roundtrip(
    data: &[u8],
    pilot_cfg: &PilotConfig,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let config = &pilot_cfg.config;
    let data_bins = pilot_cfg.data_bins();

    // Frame the data (adds sync magic, length, CRC)
    let framed = crate::framing::frame(data, config);

    // Convert framed bytes to symbols
    let all_symbols = bytes_to_pilot_symbols(&framed, pilot_cfg);

    // Split into per-frame symbol chunks (data_bins symbols per frame)
    let symbol_frames: Vec<Vec<(usize, usize)>> = all_symbols
        .chunks(data_bins)
        .map(|chunk| {
            let mut frame = chunk.to_vec();
            frame.resize(data_bins, (0, 0));
            frame
        })
        .collect();

    // Build mono PCM: padding + data frames + trailing
    let mut pcm = Vec::new();
    let silence = vec![0.0f32; config.frame_samples];

    // Leading silence for codec settling
    for _ in 0..config.padding_frames {
        pcm.extend(&silence);
    }

    // Data frames (with pilots interleaved)
    for frame_syms in &symbol_frames {
        pcm.extend(modulate_pilot_frame(frame_syms, pilot_cfg));
    }

    // Trailing silence
    for _ in 0..config.trailing_frames {
        pcm.extend(&silence);
    }

    // MONO Opus roundtrip
    let decoded_pcm = crate::opus::opus_roundtrip(&pcm, config)?;

    // Demodulate: try different frame offsets to find sync
    let n = config.frame_samples;
    let num_frames = decoded_pcm.len() / n;

    for offset in 0..config.max_search_offset.min(num_frames) {
        let mut all_demod_symbols = Vec::new();
        for i in offset..num_frames {
            let start = i * n;
            if start + n > decoded_pcm.len() {
                break;
            }
            let frame_pcm = &decoded_pcm[start..start + n];
            let frame_symbols = demodulate_pilot_frame(frame_pcm, pilot_cfg);
            all_demod_symbols.extend(frame_symbols);
        }
        let raw_bytes = pilot_symbols_to_bytes(&all_demod_symbols, pilot_cfg);
        if let Ok(payload) = crate::framing::deframe(&raw_bytes, config) {
            return Ok(payload);
        }
    }

    Err("Pilot decode failed: sync not found".into())
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// Pilot config for tests: conservative 48 bins, 128kbps mono, spacing=5, binary amplitude.
    fn test_pilot_config() -> PilotConfig {
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

    #[test]
    fn pilot_indices_correct() {
        let pc = test_pilot_config();
        let pilots = pc.pilot_indices();
        // With 48 bins, spacing=5: indices 0, 5, 10, 15, 20, 25, 30, 35, 40, 45
        assert_eq!(pilots, vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45]);
        assert_eq!(pilots.len(), 10);
    }

    #[test]
    fn data_indices_correct() {
        let pc = test_pilot_config();
        let data = pc.data_indices();
        let pilots = pc.pilot_indices();
        // Data bins = all bins that aren't pilots
        assert_eq!(data.len(), 48 - pilots.len());
        assert_eq!(data.len(), 38);
        // No overlap
        for d in &data {
            assert!(!pilots.contains(d));
        }
    }

    #[test]
    fn bits_per_frame_correct() {
        let pc = test_pilot_config();
        // 38 data bins * 1 bit/bin (log2(2) + log2(1)) = 38
        assert_eq!(pc.bits_per_bin(), 1);
        assert_eq!(pc.bits_per_frame(), 38);
        assert_eq!(pc.bytes_per_frame(), 4); // 38/8 = 4 (floor)
    }

    #[test]
    fn throughput_calc() {
        let pc = test_pilot_config();
        // 48kHz / 960 samples = 50 fps
        // 38 bits/frame * 50 fps = 1900 bps = 237 B/s
        assert_eq!(pc.throughput_bps(), 1900);
        assert_eq!(pc.throughput_bytes(), 237);
    }

    #[test]
    fn throughput_calc_4amp_2phase() {
        let pc = PilotConfig {
            config: Config::default(), // 79 bins
            pilot_spacing: 5,
            amp_levels: 4,
            phase_levels: 2,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        };
        // 79 bins, 16 pilots (0,5,10,...,75), 63 data bins
        // 3 bits/bin * 63 = 189 bits/frame
        // 189 * 50 = 9450 bps = 1181 B/s
        assert_eq!(pc.bits_per_bin(), 3);
        let pilots = pc.pilot_indices();
        assert_eq!(pilots.len(), 16);
        assert_eq!(pc.data_bins(), 63);
        assert_eq!(pc.bits_per_frame(), 189);
        assert_eq!(pc.throughput_bps(), 9450);
    }

    #[test]
    fn symbol_packing_roundtrip() {
        let pc = test_pilot_config();
        let data: Vec<u8> = (0..50).map(|i| (i * 7 + 13) as u8).collect();
        let symbols = bytes_to_pilot_symbols(&data, &pc);
        let recovered = pilot_symbols_to_bytes(&symbols, &pc);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn symbol_packing_roundtrip_with_phase() {
        let pc = PilotConfig {
            config: Config::default(),
            pilot_spacing: 5,
            amp_levels: 4,
            phase_levels: 4,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        };
        let data: Vec<u8> = (0..100).map(|i| (i * 7 + 13) as u8).collect();
        let symbols = bytes_to_pilot_symbols(&data, &pc);
        let recovered = pilot_symbols_to_bytes(&symbols, &pc);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn modulate_demodulate_no_opus() {
        let pc = test_pilot_config();
        let data_bins = pc.data_bins();
        // Create symbols: alternating 0 and 1
        let symbols: Vec<(usize, usize)> = (0..data_bins)
            .map(|i| (i % pc.amp_levels, 0))
            .collect();

        let pcm = modulate_pilot_frame(&symbols, &pc);
        assert_eq!(pcm.len(), pc.config.frame_samples);

        let recovered = demodulate_pilot_frame(&pcm, &pc);
        assert_eq!(recovered.len(), data_bins);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn modulate_demodulate_no_opus_all_high() {
        let pc = test_pilot_config();
        let data_bins = pc.data_bins();
        // All max amplitude
        let symbols: Vec<(usize, usize)> = vec![(pc.amp_levels - 1, 0); data_bins];

        let pcm = modulate_pilot_frame(&symbols, &pc);
        let recovered = demodulate_pilot_frame(&pcm, &pc);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn modulate_demodulate_no_opus_all_low() {
        let pc = test_pilot_config();
        let data_bins = pc.data_bins();
        // All min amplitude
        let symbols: Vec<(usize, usize)> = vec![(0, 0); data_bins];

        let pcm = modulate_pilot_frame(&symbols, &pc);
        let recovered = demodulate_pilot_frame(&pcm, &pc);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn modulate_demodulate_4_levels_no_opus() {
        let pc = PilotConfig {
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
        let data_bins = pc.data_bins();
        let symbols: Vec<(usize, usize)> = (0..data_bins)
            .map(|i| (i % pc.amp_levels, 0))
            .collect();

        let pcm = modulate_pilot_frame(&symbols, &pc);
        let recovered = demodulate_pilot_frame(&pcm, &pc);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn interp_nearest_neighbor() {
        let pc = PilotConfig {
            interp_method: InterpMethod::NearestNeighbor,
            ..test_pilot_config()
        };
        let data_bins = pc.data_bins();
        let symbols: Vec<(usize, usize)> = (0..data_bins)
            .map(|i| (i % pc.amp_levels, 0))
            .collect();

        let pcm = modulate_pilot_frame(&symbols, &pc);
        let recovered = demodulate_pilot_frame(&pcm, &pc);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn pilot_opus_roundtrip_binary() {
        // Binary amplitude (2 levels), pilot_spacing=5, mono 128kbps
        let pc = test_pilot_config();
        let data = b"Pilot test!";
        let recovered = pilot_opus_roundtrip(data, &pc).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn pilot_opus_roundtrip_default_bins() {
        // Use default config (79 bins) for higher throughput
        let pc = PilotConfig {
            config: Config {
                opus_bitrate: 128,
                ..Config::default()
            },
            pilot_spacing: 5,
            amp_levels: 2,
            phase_levels: 1,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
        };
        let data = b"Pilot 79 bins";
        let recovered = pilot_opus_roundtrip(data, &pc).unwrap();
        assert_eq!(recovered, data);
    }
}
