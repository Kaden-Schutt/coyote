/// yawp — FFT decode with confidence + neural error correction, pure Rust.
///
/// Replaces the numpy/PyTorch pipeline entirely. The 46K-parameter corrector
/// MLP is baked into the binary via `include_bytes!`.

use crate::config::Config;

// ---------------------------------------------------------------------------
// Embedded weights (185 KB, baked into the binary at compile time)
// ---------------------------------------------------------------------------

static WEIGHTS_BIN: &[u8] = include_bytes!("yawp_weights.bin");

/// Parse the embedded weight blob into typed slices.
struct Weights {
    bitrate_embed: Vec<f32>,  // [5, 16]
    fc1_weight: Vec<f32>,     // [256, 45]
    fc1_bias: Vec<f32>,       // [256]
    fc2_weight: Vec<f32>,     // [128, 256]
    fc2_bias: Vec<f32>,       // [128]
    fc3_weight: Vec<f32>,     // [4, 128]
    fc3_bias: Vec<f32>,       // [4]
    gate_fc1_weight: Vec<f32>, // [32, 45]
    gate_fc1_bias: Vec<f32>,  // [32]
    gate_fc2_weight: Vec<f32>, // [1, 32]
    gate_fc2_bias: Vec<f32>,  // [1]
}

fn load_weights() -> Weights {
    let floats: Vec<f32> = WEIGHTS_BIN
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();

    let mut offset = 0;
    let mut take = |n: usize| -> Vec<f32> {
        let slice = floats[offset..offset + n].to_vec();
        offset += n;
        slice
    };

    Weights {
        bitrate_embed: take(5 * 16),
        fc1_weight: take(256 * 45),
        fc1_bias: take(256),
        fc2_weight: take(128 * 256),
        fc2_bias: take(128),
        fc3_weight: take(4 * 128),
        fc3_bias: take(4),
        gate_fc1_weight: take(32 * 45),
        gate_fc1_bias: take(32),
        gate_fc2_weight: take(1 * 32),
        gate_fc2_bias: take(1),
    }
}

// ---------------------------------------------------------------------------
// Math primitives
// ---------------------------------------------------------------------------

/// y = W @ x + b, where W is [out, inp] row-major.
fn linear(weight: &[f32], bias: &[f32], input: &[f32], out_dim: usize, in_dim: usize) -> Vec<f32> {
    let mut output = bias.to_vec();
    for i in 0..out_dim {
        let row = &weight[i * in_dim..(i + 1) * in_dim];
        for j in 0..in_dim {
            output[i] += row[j] * input[j];
        }
    }
    output
}

fn relu(x: &mut [f32]) {
    for v in x.iter_mut() {
        *v = v.max(0.0);
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn softmax(x: &[f32]) -> Vec<f32> {
    let max = x.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = x.iter().map(|&v| (v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|&e| e / sum).collect()
}

// ---------------------------------------------------------------------------
// FFT decode with confidence
// ---------------------------------------------------------------------------

/// Result of FFT decoding a single frame with confidence information.
pub struct FftDecodeResult {
    /// Best symbol per bin [n_bins].
    pub symbols: Vec<i32>,
    /// Confidence ratio per bin [n_bins] (top / second magnitude).
    pub confidences: Vec<f32>,
    /// Raw magnitudes per bin per symbol [n_bins][depth_levels].
    pub magnitudes: Vec<Vec<f32>>,
}

/// Decode a PCM frame using per-bin DFT, returning symbols, confidences, and magnitudes.
pub fn fft_decode_with_confidence(pcm: &[f32], config: &Config) -> FftDecodeResult {
    let n = config.frame_samples;
    let levels = config.depth.levels();
    let sample_rate = config.sample_rate as f64;

    let mut symbols = Vec::with_capacity(config.n_bins);
    let mut confidences = Vec::with_capacity(config.n_bins);
    let mut magnitudes = Vec::with_capacity(config.n_bins);

    for bin_idx in 0..config.n_bins {
        let mut bin_mags = vec![0.0f32; levels];

        for sym in 0..levels {
            let freq = config.tone_freq(bin_idx, sym) as f64;
            let omega = 2.0 * std::f64::consts::PI * freq;
            let mut i_acc = 0.0f64;
            let mut q_acc = 0.0f64;

            for t_idx in 0..n {
                let t = t_idx as f64 / sample_rate;
                let s = pcm[t_idx] as f64;
                i_acc += s * (omega * t).cos();
                q_acc += s * (omega * t).sin();
            }

            bin_mags[sym] = (i_acc * i_acc + q_acc * q_acc) as f32;
        }

        let mut best_sym = 0i32;
        let mut best_mag = bin_mags[0];
        for (s, &m) in bin_mags.iter().enumerate().skip(1) {
            if m > best_mag {
                best_mag = m;
                best_sym = s as i32;
            }
        }

        let mut sorted = bin_mags.clone();
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let confidence = sorted[0] / (sorted[1] + 1e-10);

        symbols.push(best_sym);
        confidences.push(confidence);
        magnitudes.push(bin_mags);
    }

    FftDecodeResult { symbols, confidences, magnitudes }
}

// ---------------------------------------------------------------------------
// Neural corrector forward pass
// ---------------------------------------------------------------------------

const N_BINS: usize = 79;
const DEPTH_LEVELS: usize = 4;

/// Run the full YawpCorrector forward pass for one frame.
///
/// Returns corrected symbols [79].
pub fn correct(
    symbols: &[i32],
    confidences: &[f32],
    magnitudes: &[Vec<f32>],
    bitrate_idx: usize,
) -> Vec<i32> {
    let w = load_weights();

    // Build per-bin feature vectors and run MLP independently per bin.
    // This mirrors the batched PyTorch version but bin-by-bin.

    // Precompute one-hot + log_conf for all bins (needed for neighbor context)
    let mut all_onehot_conf = vec![[0.0f32; 5]; N_BINS]; // [4 onehot + 1 log_conf]
    for bin in 0..N_BINS {
        let sym = symbols[bin] as usize;
        all_onehot_conf[bin][sym] = 1.0;
        all_onehot_conf[bin][4] = (confidences[bin] + 1e-6).ln();
    }

    // Bitrate embedding [16]
    let br_start = bitrate_idx * 16;
    let br_embed = &w.bitrate_embed[br_start..br_start + 16];

    let mut corrected = Vec::with_capacity(N_BINS);

    for bin in 0..N_BINS {
        // Build 45-dim feature vector:
        // [one_hot(4) | log_conf(1) | mag_norm(4) | bitrate(16) | ctx_m2(5) | ctx_m1(5) | ctx_p1(5) | ctx_p2(5)]
        let mut feat = [0.0f32; 45];

        // One-hot FFT symbol [0..4]
        feat[..4].copy_from_slice(&all_onehot_conf[bin][..4]);

        // Log confidence [4]
        feat[4] = all_onehot_conf[bin][4];

        // Normalized magnitudes [5..9]
        let mag_sum: f32 = magnitudes[bin].iter().sum::<f32>() + 1e-10;
        for i in 0..DEPTH_LEVELS {
            feat[5 + i] = magnitudes[bin][i] / mag_sum;
        }

        // Bitrate embedding [9..25]
        feat[9..25].copy_from_slice(br_embed);

        // Neighbor context: ±2 bins, each 5 values (onehot + log_conf)
        // Padded with zeros at edges (same as F.pad in PyTorch)
        let neighbors: [isize; 4] = [
            bin as isize - 2,
            bin as isize - 1,
            bin as isize + 1,
            bin as isize + 2,
        ];
        for (ni, &nb) in neighbors.iter().enumerate() {
            if nb >= 0 && (nb as usize) < N_BINS {
                let src = &all_onehot_conf[nb as usize];
                feat[25 + ni * 5..25 + ni * 5 + 5].copy_from_slice(src);
            }
            // else: stays zero (padding)
        }

        // Corrector MLP: 45 → 256 → 128 → 4
        let mut h1 = linear(&w.fc1_weight, &w.fc1_bias, &feat, 256, 45);
        relu(&mut h1);
        let mut h2 = linear(&w.fc2_weight, &w.fc2_bias, &h1, 128, 256);
        relu(&mut h2);
        let logits = linear(&w.fc3_weight, &w.fc3_bias, &h2, 4, 128);

        // Gate MLP: 45 → 32 → 1
        let mut g1 = linear(&w.gate_fc1_weight, &w.gate_fc1_bias, &feat, 32, 45);
        relu(&mut g1);
        let gate_raw = linear(&w.gate_fc2_weight, &w.gate_fc2_bias, &g1, 1, 32);
        let gate = sigmoid(gate_raw[0]);

        // Blend: (1 - gate) * onehot + gate * softmax(logits)
        let probs = softmax(&logits);
        let mut blended = [0.0f32; DEPTH_LEVELS];
        for i in 0..DEPTH_LEVELS {
            blended[i] = (1.0 - gate) * feat[i] + gate * probs[i];
        }

        // Argmax
        let best = blended
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i as i32)
            .unwrap_or(0);

        corrected.push(best);
    }

    corrected
}

// ---------------------------------------------------------------------------
// Full decode pipeline
// ---------------------------------------------------------------------------

/// Minimum confidence ratio below which the corrector MLP is invoked.
/// Above this threshold, FFT alone is reliable — skip the MLP entirely.
const CONFIDENCE_THRESHOLD: f32 = 10.0;

/// Decode a PCM frame with neural correction. Returns bits [n_bins * bits_per_bin].
///
/// If all bins have confidence above `CONFIDENCE_THRESHOLD`, the corrector
/// is skipped entirely (FFT-only fast path).
pub fn decode_frame_corrected(pcm: &[f32], config: &Config, bitrate_idx: usize) -> Vec<f32> {
    let result = fft_decode_with_confidence(pcm, config);

    let needs_correction = result.confidences.iter().any(|&c| c < CONFIDENCE_THRESHOLD);
    if needs_correction {
        let corrected = correct(&result.symbols, &result.confidences, &result.magnitudes, bitrate_idx);
        symbols_to_bits(&corrected, config.depth.bits_per_bin())
    } else {
        symbols_to_bits(&result.symbols, config.depth.bits_per_bin())
    }
}

/// Convert symbols [n_bins] to bits [n_bins * bits_per_bin].
pub fn symbols_to_bits(symbols: &[i32], bits_per_bin: usize) -> Vec<f32> {
    let n_bits = symbols.len() * bits_per_bin;
    let mut bits = vec![0.0f32; n_bits];

    for (i, &s) in symbols.iter().enumerate() {
        for b in 0..bits_per_bin {
            let bit_idx = i * bits_per_bin + b;
            bits[bit_idx] = ((s >> (bits_per_bin - 1 - b)) & 1) as f32;
        }
    }

    bits
}
