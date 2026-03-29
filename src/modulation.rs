/// Parallel Tone-Position FSK modulation/demodulation for yip.
///
/// Key insight: every bin ALWAYS emits a tone. The data determines WHICH tone.
/// This is why Opus preserves it perfectly — the codec always has tonal energy
/// to lock onto in every bin, regardless of the data pattern.
///
/// Binary (1-bit): 2 tones per bin — 3,950 bps at 79 bins
/// Quad   (2-bit): 4 tones per bin — 7,900 bps at 79 bins
/// Hex16  (4-bit): 16 tones per bin — no working configs found

use rustfft::{FftPlanner, num_complex::Complex};
use crate::config::Config;

/// Compute DFT magnitude at an exact (fractional) frequency.
/// O(N) per frequency. Used only for multi-level modes where
/// tone spacing is too tight for FFT bin resolution.
#[allow(dead_code)]
fn tone_magnitude(samples: &[f32], freq: f32, sample_rate: f32) -> f32 {
    let omega = 2.0 * std::f32::consts::PI * freq / sample_rate;
    let mut real = 0.0f32;
    let mut imag = 0.0f32;

    for (i, &s) in samples.iter().enumerate() {
        let phase = omega * i as f32;
        real += s * phase.cos();
        imag += s * phase.sin();
    }

    real * real + imag * imag
}

/// Modulate a single frame.
///
/// `symbols` is a slice of `n_bins` values, each in [0, depth.levels()).
/// Returns `frame_samples` f32 PCM samples.
pub fn modulate_frame(symbols: &[usize], config: &Config) -> Vec<f32> {
    debug_assert_eq!(symbols.len(), config.n_bins);

    let n = config.frame_samples;
    let sr = config.sample_rate as f32;
    let mut samples = vec![0.0f32; n];

    for (bin_idx, &symbol) in symbols.iter().enumerate() {
        let freq = config.tone_freq(bin_idx, symbol);
        let omega = 2.0 * std::f32::consts::PI * freq / sr;
        for (i, s) in samples.iter_mut().enumerate() {
            *s += (omega * i as f32).sin();
        }
    }

    // Normalize to [-1, 1] with headroom
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    if peak > 0.0 {
        let scale = 0.95 / peak;
        for s in samples.iter_mut() {
            *s *= scale;
        }
    }

    samples
}

/// Demodulate a single frame.
///
/// Returns `n_bins` symbols, each in [0, depth.levels()).
///
/// For binary/quad: uses windowed FFT magnitude comparison (Python-compatible).
/// This approach is tolerant of the phase/frequency shifts Opus introduces.
///
/// For hex16: uses exact-frequency DFT since tone spacing is too tight for FFT bins.
pub fn demodulate_frame(samples: &[f32], config: &Config) -> Vec<usize> {
    debug_assert_eq!(samples.len(), config.frame_samples);

    let n = config.frame_samples;
    let levels = config.depth.levels();

    if levels <= 2 {
        // FFT with windowed peak detection (Python-compatible, Opus-tolerant)
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let mut buffer: Vec<Complex<f32>> = samples
            .iter()
            .map(|&s| Complex::new(s, 0.0))
            .collect();
        fft.process(&mut buffer);

        let spectrum: Vec<f32> = buffer[..n / 2].iter().map(|c| c.norm()).collect();
        let freq_res = config.sample_rate as f32 / n as f32;

        let mut symbols = Vec::with_capacity(config.n_bins);
        for bin_idx in 0..config.n_bins {
            let mut best_symbol = 0;
            let mut best_mag = -1.0f32;

            for symbol in 0..levels {
                let target_freq = config.tone_freq(bin_idx, symbol);
                let fft_bin = (target_freq / freq_res).round() as usize;

                // Window: check ±1 bins around target (Python uses lo=bin-1, hi=bin+2)
                let lo = fft_bin.saturating_sub(1);
                let hi = (fft_bin + 2).min(spectrum.len());
                let mag = spectrum[lo..hi]
                    .iter()
                    .cloned()
                    .fold(0.0f32, f32::max);

                if mag > best_mag {
                    best_mag = mag;
                    best_symbol = symbol;
                }
            }
            symbols.push(best_symbol);
        }
        symbols
    } else {
        // Exact-frequency DFT for multi-level modes (quad, hex16)
        // The windowed FFT approach bleeds between adjacent tones
        // when tone spacing is close to FFT bin width.
        let mut symbols = Vec::with_capacity(config.n_bins);
        for bin_idx in 0..config.n_bins {
            let mut best_symbol = 0;
            let mut best_mag = -1.0f32;

            for symbol in 0..levels {
                let target_freq = config.tone_freq(bin_idx, symbol);
                let mag = tone_magnitude(samples, target_freq, config.sample_rate as f32);

                if mag > best_mag {
                    best_mag = mag;
                    best_symbol = symbol;
                }
            }
            symbols.push(best_symbol);
        }
        symbols
    }
}

/// Generate a silent frame (for padding).
pub fn silence_frame(config: &Config) -> Vec<f32> {
    vec![0.0f32; config.frame_samples]
}

// ── Bit packing ──────────────────────────────────────────────

/// Convert bytes to a list of symbols for the given modulation depth.
///
/// For Binary: each byte → 8 symbols (1 bit each)
/// For Quad:   each byte → 4 symbols (2 bits each)
/// For Hex16:  each byte → 2 symbols (4 bits each)
pub fn bytes_to_symbols(data: &[u8], config: &Config) -> Vec<usize> {
    let bpb = config.depth.bits_per_bin();
    let mask = (1usize << bpb) - 1;

    let mut symbols = Vec::with_capacity(data.len() * 8 / bpb);
    for &byte in data {
        let symbols_per_byte = 8 / bpb;
        for i in (0..symbols_per_byte).rev() {
            symbols.push(((byte as usize) >> (i * bpb)) & mask);
        }
    }
    symbols
}

/// Convert symbols back to bytes.
pub fn symbols_to_bytes(symbols: &[usize], config: &Config) -> Vec<u8> {
    let bpb = config.depth.bits_per_bin();
    let symbols_per_byte = 8 / bpb;

    symbols
        .chunks(symbols_per_byte)
        .filter(|chunk| chunk.len() == symbols_per_byte)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &sym) in chunk.iter().enumerate() {
                let shift = (symbols_per_byte - 1 - i) * bpb;
                byte |= (sym as u8) << shift;
            }
            byte
        })
        .collect()
}

/// Pack symbols into frames of n_bins each, padding the last frame.
pub fn symbols_to_frames(symbols: &[usize], config: &Config) -> Vec<Vec<usize>> {
    symbols
        .chunks(config.n_bins)
        .map(|chunk| {
            let mut frame = chunk.to_vec();
            // Pad with symbol 0 (arbitrary but consistent)
            frame.resize(config.n_bins, 0);
            frame
        })
        .collect()
}

// ── High-level multi-frame encode/decode ─────────────────────

/// Encode bytes into PCM samples (multi-frame, with padding).
pub fn encode_pcm(data: &[u8], config: &Config) -> Vec<f32> {
    let symbols = bytes_to_symbols(data, config);
    let frames = symbols_to_frames(&symbols, config);

    let mut pcm = Vec::new();

    // Leading silence for codec settling
    for _ in 0..config.padding_frames {
        pcm.extend(silence_frame(config));
    }

    // Data frames
    for frame_symbols in &frames {
        pcm.extend(modulate_frame(frame_symbols, config));
    }

    // Trailing silence
    for _ in 0..config.trailing_frames {
        pcm.extend(silence_frame(config));
    }

    pcm
}

/// Decode PCM samples back to bytes, scanning for sync header.
///
/// Tries different frame offsets to find the sync magic,
/// accounting for Opus codec delay shifting the frame boundaries.
pub fn decode_pcm(pcm: &[f32], config: &Config) -> Option<Vec<u8>> {
    let n = config.frame_samples;
    let num_frames = pcm.len() / n;

    for offset in 0..config.max_search_offset.min(num_frames) {
        if let Some(data) = try_decode_at_offset(pcm, offset, config) {
            return Some(data);
        }
    }
    None
}

fn try_decode_at_offset(pcm: &[f32], offset: usize, config: &Config) -> Option<Vec<u8>> {
    let n = config.frame_samples;
    let num_frames = pcm.len() / n;

    if offset >= num_frames {
        return None;
    }

    // Header: 4 (magic) + 4 (length) + 4 (crc32) = 12 bytes
    let header_bytes = 12;
    let symbols_per_byte = 8 / config.depth.bits_per_bin();
    let header_symbols = header_bytes * symbols_per_byte;
    let header_frames = (header_symbols + config.n_bins - 1) / config.n_bins;

    if offset + header_frames > num_frames {
        return None;
    }

    // Demodulate header frames
    let mut all_symbols = Vec::new();
    for i in 0..header_frames {
        let start = (offset + i) * n;
        let frame = &pcm[start..start + n];
        all_symbols.extend(demodulate_frame(frame, config));
    }

    let header_data = symbols_to_bytes(&all_symbols[..header_symbols], config);

    // Check sync magic
    if header_data.len() < 12 || header_data[..4] != config.sync_magic {
        return None;
    }

    let payload_len = u32::from_le_bytes([
        header_data[4], header_data[5], header_data[6], header_data[7],
    ]) as usize;
    let expected_crc = u32::from_le_bytes([
        header_data[8], header_data[9], header_data[10], header_data[11],
    ]);

    // Calculate total frames needed
    let total_bytes = header_bytes + payload_len;
    let total_symbols = total_bytes * symbols_per_byte;
    let total_frames = (total_symbols + config.n_bins - 1) / config.n_bins;

    if offset + total_frames > num_frames {
        return None;
    }

    // Demodulate remaining frames
    for i in header_frames..total_frames {
        let start = (offset + i) * n;
        let frame = &pcm[start..start + n];
        all_symbols.extend(demodulate_frame(frame, config));
    }

    let all_data = symbols_to_bytes(&all_symbols[..total_symbols], config);
    let payload = &all_data[header_bytes..header_bytes + payload_len];

    // Verify CRC
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(payload);
    let actual_crc = hasher.finalize();

    if actual_crc != expected_crc {
        return None;
    }

    Some(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Depth;

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn roundtrip_single_frame_binary() {
        let config = Config::conservative();
        let symbols: Vec<usize> = (0..config.n_bins).map(|i| i % 2).collect();
        let samples = modulate_frame(&symbols, &config);
        let recovered = demodulate_frame(&samples, &config);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn roundtrip_single_frame_all_zeros() {
        let config = default_config();
        let symbols = vec![0usize; config.n_bins];
        let samples = modulate_frame(&symbols, &config);
        let recovered = demodulate_frame(&samples, &config);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn roundtrip_single_frame_all_ones() {
        let config = default_config();
        let symbols = vec![1usize; config.n_bins];
        let samples = modulate_frame(&symbols, &config);
        let recovered = demodulate_frame(&samples, &config);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn roundtrip_quad_frame() {
        let config = Config::for_depth(Depth::Quad);
        let symbols: Vec<usize> = (0..config.n_bins).map(|i| i % 4).collect();
        let samples = modulate_frame(&symbols, &config);
        let recovered = demodulate_frame(&samples, &config);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn roundtrip_hex16_frame() {
        let config = Config::for_depth(Depth::Hex16);
        let symbols: Vec<usize> = (0..config.n_bins).map(|i| i % 16).collect();
        let samples = modulate_frame(&symbols, &config);
        let recovered = demodulate_frame(&samples, &config);
        assert_eq!(symbols, recovered);
    }

    #[test]
    fn bytes_symbols_roundtrip_binary() {
        let config = Config::conservative();
        let data = b"Hello, yip!";
        let symbols = bytes_to_symbols(data, &config);
        let recovered = symbols_to_bytes(&symbols, &config);
        assert_eq!(&recovered[..data.len()], data.as_slice());
    }

    #[test]
    fn bytes_symbols_roundtrip_quad() {
        let config = default_config();
        let data: Vec<u8> = (0..=255).collect();
        let symbols = bytes_to_symbols(&data, &config);
        let recovered = symbols_to_bytes(&symbols, &config);
        assert_eq!(recovered, data);
    }

    #[test]
    fn bytes_symbols_roundtrip_hex16() {
        let mut config = default_config();
        config.depth = Depth::Hex16;
        let data: Vec<u8> = (0..=255).collect();
        let symbols = bytes_to_symbols(&data, &config);
        let recovered = symbols_to_bytes(&symbols, &config);
        assert_eq!(recovered, data);
    }

    #[test]
    fn roundtrip_full_quad() {
        let config = default_config();
        let data = b"The salient frequencies survive.";
        let symbols = bytes_to_symbols(data, &config);
        let frames = symbols_to_frames(&symbols, &config);

        let mut all_recovered_symbols = Vec::new();
        for frame_symbols in &frames {
            let samples = modulate_frame(frame_symbols, &config);
            let recovered = demodulate_frame(&samples, &config);
            all_recovered_symbols.extend(recovered);
        }

        let symbols_per_byte = 8 / config.depth.bits_per_bin();
        let symbols_needed = data.len() * symbols_per_byte;
        let recovered = symbols_to_bytes(&all_recovered_symbols[..symbols_needed], &config);
        assert_eq!(&recovered[..], data.as_slice());
    }
}
