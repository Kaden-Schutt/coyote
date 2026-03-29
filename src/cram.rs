/// CRAM — Codec-Relative Amplitude Modulation for yip.
///
/// Uses stereo Opus: left channel = reference signal (known tones),
/// right channel = data signal. By comparing data to reference after Opus,
/// we measure what the codec did and extract amplitude/phase ratios.

use crate::config::Config;
use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct CramConfig {
    pub config: Config,
    pub amp_levels: usize,
    pub phase_levels: usize,
    pub ref_type: RefType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefType {
    Flat,
    Alternating,
}

impl CramConfig {
    pub fn bits_per_bin(&self) -> usize {
        log2(self.amp_levels) + log2(self.phase_levels)
    }
    pub fn bits_per_frame(&self) -> usize {
        self.config.n_bins * self.bits_per_bin()
    }
    pub fn bytes_per_frame(&self) -> usize {
        self.bits_per_frame() / 8
    }
    pub fn throughput_bps(&self) -> usize {
        self.bits_per_frame() * self.config.frames_per_sec()
    }
    pub fn throughput_bytes(&self) -> usize {
        self.throughput_bps() / 8
    }
}

fn log2(n: usize) -> usize {
    assert!(n.is_power_of_two() && n >= 1, "log2 requires power of 2, got {}", n);
    n.trailing_zeros() as usize
}

pub fn generate_reference_frame(cram: &CramConfig) -> Vec<f32> {
    let config = &cram.config;
    let n = config.frame_samples;
    let sr = config.sample_rate as f32;
    let mut samples = vec![0.0f32; n];
    for bin in 0..config.n_bins {
        let freq = config.bin_center(bin);
        let amp = match cram.ref_type {
            RefType::Flat => 1.0,
            RefType::Alternating => if bin % 2 == 0 { 1.0 } else { 0.5 },
        };
        let omega = 2.0 * PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += amp * (omega * i as f32).sin();
        }
    }
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = 0.95 / peak;
        for s in samples.iter_mut() { *s *= scale; }
    }
    samples
}

pub fn modulate_cram_frame(symbols: &[(usize, usize)], cram: &CramConfig) -> Vec<f32> {
    let config = &cram.config;
    let n = config.frame_samples;
    let sr = config.sample_rate as f32;
    let mut samples = vec![0.0f32; n];
    for (bin, &(amp_sym, phase_sym)) in symbols.iter().enumerate() {
        let freq = config.bin_center(bin);
        let amp = if cram.amp_levels <= 1 {
            1.0
        } else {
            0.2 + 0.8 * (amp_sym as f32 / (cram.amp_levels - 1) as f32)
        };
        let phase = if cram.phase_levels <= 1 {
            0.0
        } else {
            2.0 * PI * (phase_sym as f32 / cram.phase_levels as f32)
        };
        let omega = 2.0 * PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += amp * (omega * i as f32 + phase).sin();
        }
    }
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = 0.95 / peak;
        for s in samples.iter_mut() { *s *= scale; }
    }
    samples
}

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

pub fn demodulate_cram_frame(
    ref_samples: &[f32],
    data_samples: &[f32],
    cram: &CramConfig,
) -> Vec<(usize, usize)> {
    let config = &cram.config;
    let sr = config.sample_rate as f32;

    // First pass: compute all raw ratios and phases
    let mut raw_ratios = Vec::with_capacity(config.n_bins);
    let mut rel_phases = Vec::with_capacity(config.n_bins);

    for bin in 0..config.n_bins {
        let freq = config.bin_center(bin);
        let (i_ref, q_ref) = extract_iq(ref_samples, freq, sr);
        let (i_data, q_data) = extract_iq(data_samples, freq, sr);
        let amp_ref = (i_ref * i_ref + q_ref * q_ref).sqrt();
        let amp_data = (i_data * i_data + q_data * q_data).sqrt();
        let ratio = if amp_ref > 1e-6 { amp_data / amp_ref } else { 0.0 };
        raw_ratios.push(ratio);

        let phase_ref = q_ref.atan2(i_ref);
        let phase_data = q_data.atan2(i_data);
        let mut rel_phase = phase_data - phase_ref;
        while rel_phase < 0.0 { rel_phase += 2.0 * PI; }
        while rel_phase >= 2.0 * PI { rel_phase -= 2.0 * PI; }
        rel_phases.push(rel_phase);
    }

    // Normalize ratios by dividing by max (to cancel global scale differences)
    let ratio_max = raw_ratios.iter().cloned().fold(0.0f32, f32::max);

    let mut symbols = Vec::with_capacity(config.n_bins);
    for bin in 0..config.n_bins {
        let amp_sym = if cram.amp_levels <= 1 {
            0
        } else {
            let norm_ratio = if ratio_max > 1e-6 {
                raw_ratios[bin] / ratio_max
            } else {
                0.0
            };
            let mapped = ((norm_ratio - 0.2) / 0.8).clamp(0.0, 1.0);
            let sym = (mapped * (cram.amp_levels - 1) as f32).round() as usize;
            sym.min(cram.amp_levels - 1)
        };

        let phase_sym = if cram.phase_levels <= 1 {
            0
        } else {
            let normalized = rel_phases[bin] / (2.0 * PI);
            let sym = (normalized * cram.phase_levels as f32).round() as usize;
            sym % cram.phase_levels
        };

        symbols.push((amp_sym, phase_sym));
    }
    symbols
}

pub fn bytes_to_cram_symbols(data: &[u8], cram: &CramConfig) -> Vec<(usize, usize)> {
    let bpb = cram.bits_per_bin();
    let amp_bits = log2(cram.amp_levels);
    let phase_bits = log2(cram.phase_levels);
    let bits: Vec<bool> = data.iter()
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

pub fn cram_symbols_to_bytes(symbols: &[(usize, usize)], cram: &CramConfig) -> Vec<u8> {
    let amp_bits = log2(cram.amp_levels);
    let phase_bits = log2(cram.phase_levels);
    let bits: Vec<bool> = symbols.iter()
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
            c.iter().enumerate()
                .fold(0u8, |acc, (i, &b)| acc | ((b as u8) << (7 - i)))
        })
        .collect()
}

fn bits_to_usize(bits: &[bool]) -> usize {
    bits.iter().fold(0usize, |acc, &b| (acc << 1) | (b as usize))
}

pub fn cram_opus_roundtrip(
    data: &[u8],
    cram: &CramConfig,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let config = &cram.config;
    let framed = crate::framing::frame(data, config);
    let all_symbols = bytes_to_cram_symbols(&framed, cram);
    let symbol_frames: Vec<Vec<(usize, usize)>> = all_symbols
        .chunks(config.n_bins)
        .map(|chunk| {
            let mut frame = chunk.to_vec();
            frame.resize(config.n_bins, (0, 0));
            frame
        })
        .collect();

    let ref_frame = generate_reference_frame(cram);
    let mut left_pcm = Vec::new();
    let mut right_pcm = Vec::new();

    let silence = vec![0.0f32; config.frame_samples];
    for _ in 0..config.padding_frames {
        left_pcm.extend(&ref_frame);
        right_pcm.extend(&silence);
    }
    for frame_syms in &symbol_frames {
        left_pcm.extend(&ref_frame);
        right_pcm.extend(modulate_cram_frame(frame_syms, cram));
    }
    for _ in 0..config.trailing_frames {
        left_pcm.extend(&ref_frame);
        right_pcm.extend(&silence);
    }

    let (left_out, right_out) = crate::opus::stereo_opus_roundtrip(&left_pcm, &right_pcm, config)?;

    let n = config.frame_samples;
    let num_frames = left_out.len().min(right_out.len()) / n;

    for offset in 0..config.max_search_offset.min(num_frames) {
        let mut all_demod_symbols = Vec::new();
        for i in offset..num_frames {
            let start = i * n;
            if start + n > left_out.len() || start + n > right_out.len() { break; }
            let ref_frame_pcm = &left_out[start..start + n];
            let data_frame_pcm = &right_out[start..start + n];
            let frame_symbols = demodulate_cram_frame(ref_frame_pcm, data_frame_pcm, cram);
            all_demod_symbols.extend(frame_symbols);
        }
        let raw_bytes = cram_symbols_to_bytes(&all_demod_symbols, cram);
        if let Ok(payload) = crate::framing::deframe(&raw_bytes, config) {
            return Ok(payload);
        }
    }

    Err("CRAM decode failed: sync not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// CRAM needs stereo Opus, which requires more bitrate than mono.
    /// Conservative config is 48 bins / 64kbps (mono). For CRAM we bump to 128kbps.
    fn cram_test_config() -> Config {
        Config {
            opus_bitrate: 128,
            ..Config::conservative()
        }
    }

    fn default_cram() -> CramConfig {
        CramConfig {
            config: cram_test_config(),
            amp_levels: 4,
            phase_levels: 1,
            ref_type: RefType::Flat,
        }
    }

    #[test]
    fn symbol_packing_roundtrip() {
        let cram = default_cram();
        let data: Vec<u8> = (0..100).map(|i| (i * 7 + 13) as u8).collect();
        let symbols = bytes_to_cram_symbols(&data, &cram);
        let recovered = cram_symbols_to_bytes(&symbols, &cram);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn symbol_packing_roundtrip_with_phase() {
        let cram = CramConfig {
            config: cram_test_config(),
            amp_levels: 4,
            phase_levels: 4,
            ref_type: RefType::Flat,
        };
        let data: Vec<u8> = (0..100).map(|i| (i * 7 + 13) as u8).collect();
        let symbols = bytes_to_cram_symbols(&data, &cram);
        let recovered = cram_symbols_to_bytes(&symbols, &cram);
        assert_eq!(recovered[..data.len()], data[..]);
    }

    #[test]
    fn reference_frame_not_silent() {
        let cram = default_cram();
        let ref_pcm = generate_reference_frame(&cram);
        assert_eq!(ref_pcm.len(), cram.config.frame_samples);
        let peak = ref_pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.5, "reference frame should have significant energy, peak={}", peak);
    }

    #[test]
    fn modulate_demodulate_no_opus() {
        let cram = default_cram();
        let symbols: Vec<(usize, usize)> = (0..cram.config.n_bins)
            .map(|i| (i % cram.amp_levels, 0))
            .collect();
        let ref_pcm = generate_reference_frame(&cram);
        let data_pcm = modulate_cram_frame(&symbols, &cram);
        let recovered = demodulate_cram_frame(&ref_pcm, &data_pcm, &cram);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn modulate_demodulate_binary_no_opus() {
        let cram = CramConfig {
            config: cram_test_config(),
            amp_levels: 2,
            phase_levels: 1,
            ref_type: RefType::Flat,
        };
        let symbols: Vec<(usize, usize)> = (0..cram.config.n_bins)
            .map(|i| (i % 2, 0))
            .collect();
        let ref_pcm = generate_reference_frame(&cram);
        let data_pcm = modulate_cram_frame(&symbols, &cram);
        let recovered = demodulate_cram_frame(&ref_pcm, &data_pcm, &cram);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn cram_opus_roundtrip_amplitude_only() {
        // Binary CRAM at 128kbps works reliably
        let cram = CramConfig {
            config: cram_test_config(),
            amp_levels: 2,
            phase_levels: 1,
            ref_type: RefType::Flat,
        };
        let data = b"CRAM test!";
        let recovered = cram_opus_roundtrip(data, &cram).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn cram_throughput_calc() {
        let cram = CramConfig {
            config: cram_test_config(),
            amp_levels: 4,
            phase_levels: 1,
            ref_type: RefType::Flat,
        };
        assert_eq!(cram.bits_per_bin(), 2);
        assert_eq!(cram.bits_per_frame(), 96);
        assert_eq!(cram.bytes_per_frame(), 12);
        assert_eq!(cram.throughput_bps(), 4800);
    }

    #[test]
    fn cram_throughput_calc_with_phase() {
        let cram = CramConfig {
            config: Config::default(),
            amp_levels: 8,
            phase_levels: 8,
            ref_type: RefType::Flat,
        };
        assert_eq!(cram.bits_per_bin(), 6);
        assert_eq!(cram.bits_per_frame(), 474);
        assert_eq!(cram.throughput_bps(), 23700);
    }

    #[test]
    fn alternating_reference_not_silent() {
        let cram = CramConfig {
            config: cram_test_config(),
            amp_levels: 4,
            phase_levels: 1,
            ref_type: RefType::Alternating,
        };
        let ref_pcm = generate_reference_frame(&cram);
        let peak = ref_pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.5, "alternating reference should have energy");
    }
}
