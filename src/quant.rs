/// Quantization-inspired wrap/unwrap API for yip.
///
/// The mental model:
///   Full precision → INT4:  weights survive.  Same model works.
///   Raw PCM → Opus 128k:   tone positions survive.  Same data decodes.
///
/// `wrap()` = "quantize": binary data → framed → modulated PCM
/// `unwrap()` = "dequantize": PCM → demodulated → deframed → data
///
/// Depth controls the tradeoff:
///   Binary (1-bit) = safe, 493 B/s     (INT8 equivalent)
///   Quad   (2-bit) = 2x,   987 B/s     (INT4 equivalent)
///   Hex16  (4-bit) = —     no working configs

use crate::config::{Config, Depth};
use crate::framing::{self, FrameError};
use crate::modulation;
use crate::opus;

/// Wrap arbitrary bytes into Opus-safe PCM samples.
///
/// Pipeline: data → frame(sync+crc) → symbols → modulate → PCM (with padding)
pub fn wrap(data: &[u8], config: &Config) -> Vec<f32> {
    let framed = framing::frame(data, config);
    modulation::encode_pcm(&framed, config)
}

/// Unwrap PCM samples back to original bytes.
///
/// Pipeline: PCM → demodulate → symbols → bytes → deframe(verify crc) → data
pub fn unwrap(pcm: &[f32], config: &Config) -> Result<Vec<u8>, FrameError> {
    modulation::decode_pcm(pcm, config).ok_or(FrameError::NoSync)
}

/// Full Opus pipeline: data → PCM → Opus → PCM → data.
///
/// This is the proof that the data survives lossy compression.
pub fn opus_roundtrip(
    data: &[u8],
    config: &Config,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pcm = wrap(data, config);
    let opus_pcm = opus::opus_roundtrip(&pcm, config)?;
    let recovered = unwrap(&opus_pcm, config)?;
    Ok(recovered)
}

/// Full pipeline: data → Opus packets (for streaming/storage).
pub fn encode_to_packets(
    data: &[u8],
    config: &Config,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let pcm = wrap(data, config);
    let packets = opus::encode_frames(&pcm, config)?;
    Ok(packets)
}

/// Full pipeline: Opus packets → data.
pub fn decode_from_packets(
    packets: &[Vec<u8>],
    config: &Config,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pcm = opus::decode_frames(packets, config)?;
    let skip = opus::encoder_lookahead(config)?;
    let trimmed = if pcm.len() > skip { &pcm[skip..] } else { &pcm };
    let data = unwrap(trimmed, config)?;
    Ok(data)
}

/// Throughput stats for a given configuration.
#[derive(Debug)]
pub struct ThroughputInfo {
    pub depth: Depth,
    pub n_bins: usize,
    pub bps: usize,
    pub bytes_per_sec: usize,
    pub tokens_per_sec_utf8: usize,
    pub tokens_per_sec_id16: usize,
    pub frames_per_sec: usize,
}

pub fn throughput(config: &Config) -> ThroughputInfo {
    let bps = config.throughput_bps();
    let bytes_per_sec = bps / 8;
    ThroughputInfo {
        depth: config.depth,
        n_bins: config.n_bins,
        bps,
        bytes_per_sec,
        tokens_per_sec_utf8: bytes_per_sec / 4,
        tokens_per_sec_id16: bytes_per_sec / 2,
        frames_per_sec: config.frames_per_sec(),
    }
}

/// Estimate Opus overhead for a given payload size.
pub fn overhead(payload_len: usize, config: &Config) -> (usize, f32) {
    let framed_len = payload_len + framing::HEADER_SIZE;
    let symbols_per_byte = 8 / config.depth.bits_per_bin();
    let total_symbols = framed_len * symbols_per_byte;
    let num_data_frames = (total_symbols + config.n_bins - 1) / config.n_bins;
    let total_frames = num_data_frames + config.padding_frames + config.trailing_frames;
    let duration_ms = total_frames * 20;
    let opus_bytes = (config.opus_bitrate as usize * 1000 / 8) * duration_ms / 1000;
    let ratio = if payload_len > 0 {
        opus_bytes as f32 / payload_len as f32
    } else {
        0.0
    };
    (opus_bytes, ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_unwrap_quad() {
        let config = Config::default();
        let data = b"The salient frequencies survive.";
        let pcm = wrap(data, &config);
        let recovered = unwrap(&pcm, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn wrap_unwrap_all_bytes() {
        let config = Config::default();
        let data: Vec<u8> = (0..=255).collect();
        let pcm = wrap(&data, &config);
        let recovered = unwrap(&pcm, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn wrap_unwrap_binary() {
        let config = Config::conservative();
        let data = b"binary mode test data";
        let pcm = wrap(data, &config);
        let recovered = unwrap(&pcm, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn wrap_unwrap_hex16() {
        let config = Config::for_depth(Depth::Hex16);
        let data = b"hex16 mode test";
        let pcm = wrap(data, &config);
        let recovered = unwrap(&pcm, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn wrap_unwrap_large() {
        let config = Config::default();
        let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let pcm = wrap(&data, &config);
        let recovered = unwrap(&pcm, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_default() {
        let config = Config::default();
        let data = b"Opus roundtrip test!";
        let recovered = opus_roundtrip(data, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_longer() {
        let config = Config::default();
        let data = b"The quick brown fox jumps over the lazy dog. Testing longer payloads through Opus.";
        let recovered = opus_roundtrip(data, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn opus_roundtrip_binary_data() {
        let config = Config::default();
        let data: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
        let recovered = opus_roundtrip(&data, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn packet_roundtrip() {
        let config = Config::default();
        let data = b"packet roundtrip";
        let packets = encode_to_packets(data, &config).unwrap();
        let recovered = decode_from_packets(&packets, &config).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn throughput_default() {
        let config = Config::default();
        let info = throughput(&config);
        assert_eq!(info.bps, 7900);
        assert_eq!(info.bytes_per_sec, 987);
        assert_eq!(info.tokens_per_sec_utf8, 246);
    }

    #[test]
    fn throughput_quad() {
        let config = Config::for_depth(Depth::Quad);
        let info = throughput(&config);
        // Quad: 2 bits per bin, auto-tuned bins
        assert_eq!(info.bps, config.n_bins * 2 * 50);
        assert_eq!(info.bytes_per_sec, info.bps / 8);
    }

    #[test]
    fn throughput_hex16() {
        let config = Config::for_depth(Depth::Hex16);
        let info = throughput(&config);
        // Hex16: 4 bits per bin, auto-tuned bins (fewer, wider spacing)
        assert_eq!(info.bps, config.n_bins * 4 * 50);
        assert_eq!(info.bytes_per_sec, info.bps / 8);
    }
}
