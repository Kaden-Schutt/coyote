/// Constellation Shaping — replace uniform amp/phase grid with a custom point table.
///
/// Technique 3 from PLAN-MATH-STACK.md. Stacks on top of Pilot Tone Interpolation
/// and optionally Differential Encoding.
///
/// Key insight: not all (amplitude, phase) points survive Opus equally. A non-uniform
/// constellation that skips unreliable points and spaces reliable ones further apart
/// can improve error rates, potentially enabling more total points.
///
/// Architecture:
///   - ConstellationConfig holds a Vec of (amplitude, phase) points
///   - Modulation maps symbol indices to constellation points
///   - Demodulation finds nearest constellation point (min Euclidean distance)
///   - Preset factories: uniform_grid, nonuniform_amp, pruned_grid

use crate::config::Config;
use crate::pilot::InterpMethod;
use std::f32::consts::PI;

// ── Configuration ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConstellationConfig {
    pub config: Config,
    pub pilot_spacing: usize,
    pub pilot_amplitude: f32,
    pub interp_method: InterpMethod,
    /// The constellation points: Vec of (amplitude, phase_radians)
    /// Length must be a power of 2 for clean bit mapping (or use ceil(log2))
    pub points: Vec<(f32, f32)>,
    /// Whether to use differential encoding
    pub use_diff: bool,
    pub recal_interval: usize,
}

impl ConstellationConfig {
    /// Bits per symbol = floor(log2(points.len()))
    pub fn bits_per_symbol(&self) -> usize {
        if self.points.len() <= 1 {
            return 0;
        }
        (self.points.len() as f64).log2().floor() as usize
    }

    /// Number of usable symbols (2^bits_per_symbol, may be less than points.len())
    fn usable_symbols(&self) -> usize {
        1 << self.bits_per_symbol()
    }

    /// Pilot bin indices
    fn pilot_indices(&self) -> Vec<usize> {
        (0..self.config.n_bins)
            .step_by(self.pilot_spacing)
            .collect()
    }

    /// Data (non-pilot) bin indices
    fn data_indices(&self) -> Vec<usize> {
        let pilots: std::collections::HashSet<usize> =
            self.pilot_indices().into_iter().collect();
        (0..self.config.n_bins)
            .filter(|i| !pilots.contains(i))
            .collect()
    }

    /// Number of data bins per frame
    pub fn data_bins(&self) -> usize {
        self.data_indices().len()
    }

    /// Total data bits per frame
    pub fn bits_per_frame(&self) -> usize {
        self.data_bins() * self.bits_per_symbol()
    }

    /// Bytes per frame (floor division)
    pub fn bytes_per_frame(&self) -> usize {
        self.bits_per_frame() / 8
    }

    /// Throughput in bits per second
    pub fn throughput_bps(&self) -> usize {
        self.bits_per_frame() * self.config.frames_per_sec()
    }

    /// Throughput in bytes per second
    pub fn throughput_bytes(&self) -> usize {
        self.throughput_bps() / 8
    }
}

// ── Preset constellations ───────────────────────────────────────

/// Standard uniform grid (same as current pilot amp/phase grid).
/// Amplitude: [0.2, 1.0] linearly spaced, Phase: [0, 2π) linearly spaced.
pub fn uniform_grid(amp_levels: usize, phase_levels: usize) -> Vec<(f32, f32)> {
    let mut points = Vec::with_capacity(amp_levels * phase_levels);
    for a in 0..amp_levels {
        let amp = if amp_levels <= 1 {
            1.0
        } else {
            0.2 + 0.8 * (a as f32 / (amp_levels - 1) as f32)
        };
        for p in 0..phase_levels {
            let phase = if phase_levels <= 1 {
                0.0
            } else {
                2.0 * PI * (p as f32 / phase_levels as f32)
            };
            points.push((amp, phase));
        }
    }
    points
}

/// Amplitude-only with non-uniform spacing (wider gaps at extremes).
/// Uses sqrt spacing that matches Opus's psychoacoustic model —
/// more resolution in the mid-range where Opus preserves best.
pub fn nonuniform_amp(n_points: usize) -> Vec<(f32, f32)> {
    let mut points = Vec::with_capacity(n_points);
    for i in 0..n_points {
        // sqrt spacing: denser in the middle, wider at extremes
        let t = i as f32 / (n_points - 1).max(1) as f32;
        // Map [0,1] through sqrt curve, then to [0.2, 1.0]
        let amp = 0.2 + 0.8 * t.sqrt();
        points.push((amp, 0.0));
    }
    points
}

/// Best-N from a larger uniform grid: generate amp_levels × phase_levels grid,
/// keep only the `keep` most reliable points (by distance from boundaries).
/// Points closer to center amplitude and away from phase boundaries are preferred.
pub fn pruned_grid(amp_levels: usize, phase_levels: usize, keep: usize) -> Vec<(f32, f32)> {
    let full = uniform_grid(amp_levels, phase_levels);
    if keep >= full.len() {
        return full;
    }

    // Score each point by "reliability": prefer mid-range amplitudes
    // and phases away from boundaries. Higher score = more reliable.
    let mut scored: Vec<(f32, (f32, f32))> = full
        .into_iter()
        .map(|(amp, phase)| {
            // Amplitude reliability: distance from boundaries [0.2, 1.0]
            let amp_center = 0.6; // midpoint of [0.2, 1.0]
            let amp_score = 1.0 - ((amp - amp_center) / 0.4).abs();

            // Phase reliability: distance from 0/2π boundary
            let phase_score = if phase_levels <= 1 {
                1.0
            } else {
                // Phase normalized to [0, 1), distance from nearest integer
                let pn = phase / (2.0 * PI);
                let dist_from_boundary = (pn * phase_levels as f32).fract();
                let dist = (dist_from_boundary - 0.5).abs();
                1.0 - 2.0 * dist
            };

            let score = amp_score + phase_score;
            (score, (amp, phase))
        })
        .collect();

    // Sort by score descending, keep top N
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(keep);

    // Sort remaining points by (amplitude, phase) for consistent ordering
    let mut points: Vec<(f32, f32)> = scored.into_iter().map(|(_, p)| p).collect();
    points.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    points
}

// ── I/Q extraction ──────────────────────────────────────────────

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

// ── Modulation ──────────────────────────────────────────────────

/// Modulate a frame: each data bin gets a constellation point by index.
/// `symbol_indices` has one index per data bin.
pub fn modulate_constellation_frame(
    symbol_indices: &[usize],
    cfg: &ConstellationConfig,
) -> Vec<f32> {
    let config = &cfg.config;
    let n = config.frame_samples;
    let sr = config.sample_rate as f32;
    let data_idx = cfg.data_indices();
    let pilot_idx = cfg.pilot_indices();

    assert_eq!(
        symbol_indices.len(),
        data_idx.len(),
        "symbol_indices length ({}) must equal data_bins ({})",
        symbol_indices.len(),
        data_idx.len()
    );

    let mut samples = vec![0.0f32; n];

    // Pilot tones
    for &bin in &pilot_idx {
        let freq = config.bin_center(bin);
        let amp = cfg.pilot_amplitude;
        let omega = 2.0 * PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += amp * (omega * i as f32).sin();
        }
    }

    // Data tones from constellation points
    for (sym_i, &bin) in data_idx.iter().enumerate() {
        let idx = symbol_indices[sym_i];
        let (amp, phase) = cfg.points[idx];
        let freq = config.bin_center(bin);
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

// ── Demodulation ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Distortion {
    amp_scale: f32,
    phase_offset: f32,
}

fn interpolate_distortion(
    target_bin: usize,
    pilot_indices: &[usize],
    pilot_distortions: &[Distortion],
    method: InterpMethod,
) -> Distortion {
    assert_eq!(pilot_indices.len(), pilot_distortions.len());
    assert!(!pilot_indices.is_empty());

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

/// Demodulate a frame: find nearest constellation point for each data bin.
pub fn demodulate_constellation_frame(
    samples: &[f32],
    cfg: &ConstellationConfig,
) -> Vec<usize> {
    let config = &cfg.config;
    let sr = config.sample_rate as f32;
    let pilot_idx = cfg.pilot_indices();
    let data_idx = cfg.data_indices();

    // Extract I/Q at all bins
    let mut amps = vec![0.0f32; config.n_bins];
    let mut phases = vec![0.0f32; config.n_bins];

    for bin in 0..config.n_bins {
        let freq = config.bin_center(bin);
        let (i_val, q_val) = extract_iq(samples, freq, sr);
        amps[bin] = (i_val * i_val + q_val * q_val).sqrt();
        phases[bin] = q_val.atan2(i_val);
    }

    // Compute distortion at pilot bins
    let known_phase = 0.0f32;
    let mut pilot_distortions = Vec::with_capacity(pilot_idx.len());

    for &pi in &pilot_idx {
        let measured_amp = amps[pi];
        let measured_phase = phases[pi];

        let amp_scale = if cfg.pilot_amplitude > 1e-9 {
            measured_amp / cfg.pilot_amplitude
        } else {
            1.0
        };

        let mut phase_offset = measured_phase - known_phase;
        while phase_offset > PI {
            phase_offset -= 2.0 * PI;
        }
        while phase_offset < -PI {
            phase_offset += 2.0 * PI;
        }

        pilot_distortions.push(Distortion { amp_scale, phase_offset });
    }

    // For each data bin, correct and find nearest constellation point
    let usable = cfg.usable_symbols();
    let mut indices = Vec::with_capacity(data_idx.len());

    for &di in &data_idx {
        let dist = interpolate_distortion(di, &pilot_idx, &pilot_distortions, cfg.interp_method);

        let corrected_amp = if dist.amp_scale > 1e-9 {
            amps[di] / dist.amp_scale
        } else {
            0.0
        };

        let norm_amp = if cfg.pilot_amplitude > 1e-9 {
            corrected_amp / cfg.pilot_amplitude
        } else {
            0.0
        };

        let mut corrected_phase = phases[di] - dist.phase_offset;
        while corrected_phase < 0.0 {
            corrected_phase += 2.0 * PI;
        }
        while corrected_phase >= 2.0 * PI {
            corrected_phase -= 2.0 * PI;
        }

        // Find nearest constellation point (Euclidean distance in amp/phase space)
        let mut best_idx = 0;
        let mut best_dist = f32::MAX;

        for (idx, &(pt_amp, pt_phase)) in cfg.points.iter().enumerate().take(usable) {
            let da = norm_amp - pt_amp;
            // Phase distance: wrapped
            let mut dp = corrected_phase - pt_phase;
            while dp > PI {
                dp -= 2.0 * PI;
            }
            while dp < -PI {
                dp += 2.0 * PI;
            }
            // Weight phase distance by amplitude to make it comparable
            let phase_weight = norm_amp.max(pt_amp).max(0.1);
            let d = da * da + (dp * phase_weight) * (dp * phase_weight);
            if d < best_dist {
                best_dist = d;
                best_idx = idx;
            }
        }

        indices.push(best_idx);
    }

    indices
}

// ── Symbol packing ──────────────────────────────────────────────

fn bits_to_usize(bits: &[bool]) -> usize {
    bits.iter()
        .fold(0usize, |acc, &b| (acc << 1) | (b as usize))
}

/// Convert bytes to constellation symbol indices.
pub fn bytes_to_constellation_symbols(data: &[u8], cfg: &ConstellationConfig) -> Vec<usize> {
    let bps = cfg.bits_per_symbol();
    if bps == 0 {
        return vec![0; data.len() * 8]; // degenerate
    }

    let bits: Vec<bool> = data
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| (byte >> i) & 1 == 1))
        .collect();

    bits.chunks(bps)
        .map(|chunk| {
            let mut padded = chunk.to_vec();
            padded.resize(bps, false);
            let idx = bits_to_usize(&padded);
            idx.min(cfg.usable_symbols() - 1)
        })
        .collect()
}

/// Convert constellation symbol indices back to bytes.
pub fn constellation_symbols_to_bytes(indices: &[usize], cfg: &ConstellationConfig) -> Vec<u8> {
    let bps = cfg.bits_per_symbol();
    if bps == 0 {
        return Vec::new();
    }

    let bits: Vec<bool> = indices
        .iter()
        .flat_map(|&idx| {
            (0..bps).rev().map(move |i| (idx >> i) & 1 == 1)
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

// ── Differential helpers ────────────────────────────────────────

fn calibration_index(n_symbols: usize) -> usize {
    n_symbols / 2
}

fn diff_encode_indices(
    data_frames: &[Vec<usize>],
    n_symbols: usize,
    recal_interval: usize,
) -> Vec<Vec<usize>> {
    let cal_idx = calibration_index(n_symbols);
    let data_bins = if data_frames.is_empty() { 0 } else { data_frames[0].len() };
    let cal_frame: Vec<usize> = vec![cal_idx; data_bins];

    let mut output = Vec::new();
    let mut prev_frame = cal_frame.clone();
    let mut frames_since_cal = 0;

    // Initial calibration
    output.push(cal_frame.clone());

    for data_frame in data_frames {
        if recal_interval > 0 && frames_since_cal >= recal_interval {
            output.push(cal_frame.clone());
            prev_frame = cal_frame.clone();
            frames_since_cal = 0;
        }

        let delta_frame: Vec<usize> = data_frame
            .iter()
            .zip(prev_frame.iter())
            .map(|(&curr, &prev)| {
                (curr as isize - prev as isize).rem_euclid(n_symbols as isize) as usize
            })
            .collect();

        output.push(delta_frame);
        prev_frame = data_frame.clone();
        frames_since_cal += 1;
    }

    output
}

fn diff_decode_indices(
    received_frames: &[Vec<usize>],
    n_symbols: usize,
    recal_interval: usize,
) -> Vec<Vec<usize>> {
    let cal_idx = calibration_index(n_symbols);
    let data_bins = if received_frames.is_empty() { 0 } else { received_frames[0].len() };
    let cal_frame: Vec<usize> = vec![cal_idx; data_bins];

    let mut output = Vec::new();
    let mut prev_frame = cal_frame.clone();
    let mut frames_since_cal = 0;
    let mut is_first = true;

    for received in received_frames {
        if is_first {
            prev_frame = cal_frame.clone();
            is_first = false;
            frames_since_cal = 0;
            continue;
        }

        if recal_interval > 0 && frames_since_cal >= recal_interval {
            prev_frame = cal_frame.clone();
            frames_since_cal = 0;
            continue;
        }

        let abs_frame: Vec<usize> = received
            .iter()
            .zip(prev_frame.iter())
            .map(|(&recv, &prev)| (prev + recv) % n_symbols)
            .collect();

        prev_frame = abs_frame.clone();
        output.push(abs_frame);
        frames_since_cal += 1;
    }

    output
}

// ── Full Opus roundtrip ─────────────────────────────────────────

pub fn constellation_opus_roundtrip(
    data: &[u8],
    cfg: &ConstellationConfig,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let config = &cfg.config;
    let data_bins = cfg.data_bins();
    let usable = cfg.usable_symbols();

    // Frame the data
    let framed = crate::framing::frame(data, config);

    // Convert to constellation indices
    let all_indices = bytes_to_constellation_symbols(&framed, cfg);

    // Split into per-frame chunks
    let index_frames: Vec<Vec<usize>> = all_indices
        .chunks(data_bins)
        .map(|chunk| {
            let mut frame = chunk.to_vec();
            frame.resize(data_bins, 0);
            frame
        })
        .collect();

    // Optionally differential-encode
    let tx_frames = if cfg.use_diff {
        diff_encode_indices(&index_frames, usable, cfg.recal_interval)
    } else {
        index_frames.clone()
    };

    // Build mono PCM
    let mut pcm = Vec::new();
    let silence = vec![0.0f32; config.frame_samples];

    for _ in 0..config.padding_frames {
        pcm.extend(&silence);
    }
    for frame_indices in &tx_frames {
        pcm.extend(modulate_constellation_frame(frame_indices, cfg));
    }
    for _ in 0..config.trailing_frames {
        pcm.extend(&silence);
    }

    // MONO Opus roundtrip
    let decoded_pcm = crate::opus::opus_roundtrip(&pcm, config)?;

    // Demodulate
    let n = config.frame_samples;
    let num_frames = decoded_pcm.len() / n;
    let expected_tx_frames = tx_frames.len();

    for offset in 0..config.max_search_offset.min(num_frames) {
        let mut rx_frames: Vec<Vec<usize>> = Vec::new();
        for i in offset..num_frames {
            let start = i * n;
            if start + n > decoded_pcm.len() {
                break;
            }
            let frame_pcm = &decoded_pcm[start..start + n];
            rx_frames.push(demodulate_constellation_frame(frame_pcm, cfg));
        }

        // Decode: differential or direct
        let decoded_frames = if cfg.use_diff {
            if rx_frames.len() < expected_tx_frames {
                continue;
            }
            let rx_subset = &rx_frames[..expected_tx_frames];
            diff_decode_indices(rx_subset, usable, cfg.recal_interval)
        } else {
            rx_frames
        };

        let all_demod_indices: Vec<usize> = decoded_frames.into_iter().flatten().collect();
        let raw_bytes = constellation_symbols_to_bytes(&all_demod_indices, cfg);

        if let Ok(payload) = crate::framing::deframe(&raw_bytes, config) {
            return Ok(payload);
        }
    }

    Err("Constellation decode failed: sync not found".into())
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn base_config() -> Config {
        Config {
            opus_bitrate: 128,
            ..Config::conservative()
        }
    }

    fn test_constellation_cfg(points: Vec<(f32, f32)>) -> ConstellationConfig {
        ConstellationConfig {
            config: base_config(),
            pilot_spacing: 5,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
            points,
            use_diff: false,
            recal_interval: 0,
        }
    }

    #[test]
    fn symbol_packing_roundtrip() {
        let points = uniform_grid(4, 2); // 8 points = 3 bits/sym
        let cfg = test_constellation_cfg(points);
        assert_eq!(cfg.bits_per_symbol(), 3);

        let data: Vec<u8> = (0..50).map(|i| (i * 7 + 13) as u8).collect();
        let indices = bytes_to_constellation_symbols(&data, &cfg);
        let recovered = constellation_symbols_to_bytes(&indices, &cfg);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn symbol_packing_roundtrip_power_of_2() {
        let points = uniform_grid(8, 1); // 8 points = 3 bits/sym
        let cfg = test_constellation_cfg(points);
        assert_eq!(cfg.bits_per_symbol(), 3);

        let data: Vec<u8> = (0..100).map(|i| (i * 3 + 5) as u8).collect();
        let indices = bytes_to_constellation_symbols(&data, &cfg);
        let recovered = constellation_symbols_to_bytes(&indices, &cfg);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn modulate_demodulate_no_opus_uniform() {
        // 4 amp × 1 phase = 4 points = 2 bits/sym (same density as pilot 4amp)
        let points = uniform_grid(4, 1);
        let cfg = test_constellation_cfg(points);
        let data_bins = cfg.data_bins();

        let indices: Vec<usize> = (0..data_bins).map(|i| i % 4).collect();
        let pcm = modulate_constellation_frame(&indices, &cfg);
        assert_eq!(pcm.len(), cfg.config.frame_samples);

        let recovered = demodulate_constellation_frame(&pcm, &cfg);
        assert_eq!(recovered.len(), data_bins);
        assert_eq!(indices, recovered);
    }

    #[test]
    fn modulate_demodulate_no_opus_2amp() {
        let points = uniform_grid(2, 1);
        let cfg = test_constellation_cfg(points);
        let data_bins = cfg.data_bins();

        let indices: Vec<usize> = (0..data_bins).map(|i| i % 2).collect();
        let pcm = modulate_constellation_frame(&indices, &cfg);
        let recovered = demodulate_constellation_frame(&pcm, &cfg);
        assert_eq!(indices, recovered);
    }

    #[test]
    fn opus_roundtrip_uniform_2amp() {
        let points = uniform_grid(2, 1);
        let cfg = test_constellation_cfg(points);
        let data = b"Constellation!";
        let recovered = constellation_opus_roundtrip(data, &cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_uniform_4amp() {
        let points = uniform_grid(4, 1);
        let cfg = ConstellationConfig {
            config: Config {
                opus_bitrate: 256,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
            points,
            use_diff: false,
            recal_interval: 0,
        };
        let data = b"4amp constellation";
        let recovered = constellation_opus_roundtrip(data, &cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_nonuniform_amp() {
        // 8 points with nonuniform spacing = 3 bits/sym
        let points = nonuniform_amp(8);
        let cfg = ConstellationConfig {
            config: Config {
                opus_bitrate: 256,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
            points,
            use_diff: false,
            recal_interval: 0,
        };
        let data = b"nonuniform";
        let recovered = constellation_opus_roundtrip(data, &cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_pruned_grid() {
        // 8x2 grid pruned to 8 points = 3 bits/sym
        let points = pruned_grid(8, 2, 8);
        assert_eq!(points.len(), 8);
        let cfg = ConstellationConfig {
            config: Config {
                opus_bitrate: 256,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::NearestNeighbor,
            points,
            use_diff: false,
            recal_interval: 0,
        };
        let data = b"pruned grid";
        let recovered = constellation_opus_roundtrip(data, &cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_with_diff() {
        let points = uniform_grid(4, 1);
        let cfg = ConstellationConfig {
            config: Config {
                opus_bitrate: 256,
                ..Config::conservative()
            },
            pilot_spacing: 5,
            pilot_amplitude: 1.0,
            interp_method: InterpMethod::Linear,
            points,
            use_diff: true,
            recal_interval: 10,
        };
        let data = b"diff constellation";
        let recovered = constellation_opus_roundtrip(data, &cfg).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn uniform_grid_sizes() {
        assert_eq!(uniform_grid(2, 1).len(), 2);
        assert_eq!(uniform_grid(4, 2).len(), 8);
        assert_eq!(uniform_grid(8, 2).len(), 16);
        assert_eq!(uniform_grid(16, 2).len(), 32);
    }

    #[test]
    fn nonuniform_amp_sizes() {
        let pts = nonuniform_amp(8);
        assert_eq!(pts.len(), 8);
        // All phases should be 0
        for &(_, p) in &pts {
            assert_eq!(p, 0.0);
        }
        // Amplitudes should be in [0.2, 1.0] and monotonically increasing
        for i in 1..pts.len() {
            assert!(pts[i].0 > pts[i - 1].0);
        }
        assert!((pts[0].0 - 0.2).abs() < 1e-6);
        assert!((pts[pts.len() - 1].0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pruned_grid_sizes() {
        let pts = pruned_grid(16, 2, 16);
        assert_eq!(pts.len(), 16);

        let pts = pruned_grid(8, 2, 8);
        assert_eq!(pts.len(), 8);

        // Keep more than available -> returns all
        let pts = pruned_grid(4, 1, 100);
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn bits_per_symbol_correct() {
        let cfg2 = test_constellation_cfg(uniform_grid(2, 1));
        assert_eq!(cfg2.bits_per_symbol(), 1);

        let cfg4 = test_constellation_cfg(uniform_grid(4, 1));
        assert_eq!(cfg4.bits_per_symbol(), 2);

        let cfg8 = test_constellation_cfg(uniform_grid(8, 1));
        assert_eq!(cfg8.bits_per_symbol(), 3);

        let cfg16 = test_constellation_cfg(uniform_grid(16, 1));
        assert_eq!(cfg16.bits_per_symbol(), 4);

        // Non-power-of-2: 12 points -> floor(log2(12)) = 3
        let cfg12 = test_constellation_cfg(pruned_grid(8, 2, 12));
        assert_eq!(cfg12.bits_per_symbol(), 3);
    }

    #[test]
    fn diff_encode_decode_roundtrip() {
        let n_symbols = 8;
        let frames: Vec<Vec<usize>> = (0..5)
            .map(|f| (0..10).map(|i| (i + f) % n_symbols).collect())
            .collect();

        let encoded = diff_encode_indices(&frames, n_symbols, 0);
        assert_eq!(encoded.len(), 6); // 1 cal + 5 data

        let decoded = diff_decode_indices(&encoded, n_symbols, 0);
        assert_eq!(decoded.len(), 5);
        assert_eq!(decoded, frames);
    }

    #[test]
    fn diff_encode_decode_with_recal() {
        let n_symbols = 16;
        let frames: Vec<Vec<usize>> = (0..10)
            .map(|f| (0..8).map(|i| (i + f) % n_symbols).collect())
            .collect();

        let encoded = diff_encode_indices(&frames, n_symbols, 3);
        let decoded = diff_decode_indices(&encoded, n_symbols, 3);
        assert_eq!(decoded.len(), frames.len());
        assert_eq!(decoded, frames);
    }
}
