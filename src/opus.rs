/// Native Opus encode/decode using audiopus (libopus bindings).
///
/// No more shelling out to ffmpeg. Frame-by-frame operation enables
/// streaming encode/decode with 20ms latency. Part of the yip codec.

use audiopus::{
    coder::{Encoder as OpusEncoder, Decoder as OpusDecoder},
    Application, Bitrate, Channels, SampleRate,
};
use thiserror::Error;

use crate::config::Config;

#[derive(Error, Debug)]
pub enum OpusError {
    #[error("opus encoder error: {0}")]
    Encoder(String),
    #[error("opus decoder error: {0}")]
    Decoder(String),
    #[error("unsupported sample rate: {0}")]
    BadSampleRate(u32),
}

fn sample_rate(config: &Config) -> Result<SampleRate, OpusError> {
    match config.sample_rate {
        48000 => Ok(SampleRate::Hz48000),
        24000 => Ok(SampleRate::Hz24000),
        16000 => Ok(SampleRate::Hz16000),
        12000 => Ok(SampleRate::Hz12000),
        8000 => Ok(SampleRate::Hz8000),
        other => Err(OpusError::BadSampleRate(other)),
    }
}

/// Encode PCM samples to Opus packets, frame by frame.
///
/// Returns a Vec of Opus packet bytes (one per frame).
pub fn encode_frames(pcm: &[f32], config: &Config) -> Result<Vec<Vec<u8>>, OpusError> {
    let sr = sample_rate(config)?;
    let mut encoder = OpusEncoder::new(sr, Channels::Mono, Application::Audio)
        .map_err(|e| OpusError::Encoder(e.to_string()))?;

    encoder
        .set_bitrate(Bitrate::BitsPerSecond((config.opus_bitrate * 1000) as i32))
        .map_err(|e| OpusError::Encoder(e.to_string()))?;

    let frame_size = config.frame_samples;
    let num_frames = pcm.len() / frame_size;
    let mut packets = Vec::with_capacity(num_frames);

    // Max Opus packet size for one frame
    let mut output_buf = vec![0u8; 4000];

    for i in 0..num_frames {
        let start = i * frame_size;
        let end = start + frame_size;
        let frame = &pcm[start..end];

        let len = encoder
            .encode_float(frame, &mut output_buf)
            .map_err(|e| OpusError::Encoder(e.to_string()))?;

        packets.push(output_buf[..len].to_vec());
    }

    Ok(packets)
}

/// Decode Opus packets back to PCM samples, frame by frame.
///
/// Returns interleaved f32 PCM samples.
pub fn decode_frames(packets: &[Vec<u8>], config: &Config) -> Result<Vec<f32>, OpusError> {
    let sr = sample_rate(config)?;
    let mut decoder = OpusDecoder::new(sr, Channels::Mono)
        .map_err(|e| OpusError::Decoder(e.to_string()))?;

    let frame_size = config.frame_samples;
    let mut pcm = Vec::with_capacity(packets.len() * frame_size);
    let mut output_buf = vec![0.0f32; frame_size];

    for packet in packets {
        let decoded = decoder
            .decode_float(Some(packet), &mut output_buf, false)
            .map_err(|e| OpusError::Decoder(e.to_string()))?;

        pcm.extend_from_slice(&output_buf[..decoded]);
    }

    Ok(pcm)
}

/// Query the Opus encoder's algorithmic lookahead in samples.
///
/// The encoder introduces this many samples of delay. After decoding,
/// the output PCM must be trimmed by this amount to restore frame alignment.
pub fn encoder_lookahead(config: &Config) -> Result<usize, OpusError> {
    let sr = sample_rate(config)?;
    let encoder = OpusEncoder::new(sr, Channels::Mono, Application::Audio)
        .map_err(|e| OpusError::Encoder(e.to_string()))?;
    Ok(encoder.lookahead().map_err(|e| OpusError::Encoder(e.to_string()))? as usize)
}

/// Full pipeline: PCM → Opus packets → PCM (roundtrip test).
///
/// Trims the encoder lookahead from the decoded output so that
/// frame boundaries in the output align with those of the input.
pub fn opus_roundtrip(pcm: &[f32], config: &Config) -> Result<Vec<f32>, OpusError> {
    let packets = encode_frames(pcm, config)?;
    let decoded = decode_frames(&packets, config)?;
    let skip = encoder_lookahead(config)?;
    if decoded.len() > skip {
        Ok(decoded[skip..].to_vec())
    } else {
        Ok(decoded)
    }
}

/// Serialize Opus packets to a simple container format.
///
/// Format: [num_packets: u32 LE] then for each packet: [len: u16 LE] [data...]
/// This is NOT OGG — it's a minimal container for testing.
/// For real files, use the OGG writer in io.rs.
pub fn packets_to_bytes(packets: &[Vec<u8>]) -> Vec<u8> {
    let total_size: usize = 4 + packets.iter().map(|p| 2 + p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total_size);

    out.extend_from_slice(&(packets.len() as u32).to_le_bytes());
    for packet in packets {
        out.extend_from_slice(&(packet.len() as u16).to_le_bytes());
        out.extend_from_slice(packet);
    }
    out
}

/// Deserialize Opus packets from the simple container format.
pub fn bytes_to_packets(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    if data.len() < 4 {
        return None;
    }
    let num = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut offset = 4;
    let mut packets = Vec::with_capacity(num);

    for _ in 0..num {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        packets.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    Some(packets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn encode_decode_silence() {
        let config = Config::default();
        let pcm = vec![0.0f32; config.frame_samples * 5];
        let packets = encode_frames(&pcm, &config).unwrap();
        assert_eq!(packets.len(), 5);
        let recovered = decode_frames(&packets, &config).unwrap();
        assert_eq!(recovered.len(), pcm.len());
    }

    #[test]
    fn encode_decode_tone() {
        let config = Config::default();
        let freq = 1000.0;
        let pcm: Vec<f32> = (0..config.frame_samples * 3)
            .map(|i| {
                let t = i as f32 / config.sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let packets = encode_frames(&pcm, &config).unwrap();
        let recovered = decode_frames(&packets, &config).unwrap();
        // Opus is lossy — just verify length and reasonable values
        assert_eq!(recovered.len(), pcm.len());
        assert!(recovered.iter().any(|&s| s.abs() > 0.1));
    }

    #[test]
    fn packet_serialization_roundtrip() {
        let packets = vec![
            vec![1, 2, 3, 4, 5],
            vec![10, 20],
            vec![100; 50],
        ];
        let bytes = packets_to_bytes(&packets);
        let recovered = bytes_to_packets(&bytes).unwrap();
        assert_eq!(recovered, packets);
    }

    #[test]
    fn stereo_encode_decode_tone() {
        let config = Config::default();
        let freq = 1000.0;
        let num_frames = 10;
        let n = config.frame_samples * num_frames;
        let left: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / config.sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let right: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / config.sample_rate as f32;
                (2.0 * std::f32::consts::PI * 2000.0 * t).sin()
            })
            .collect();
        let packets = encode_stereo_frames(&left, &right, &config).unwrap();
        assert_eq!(packets.len(), num_frames);
        let (dec_l, dec_r) = decode_stereo_frames(&packets, &config).unwrap();
        assert_eq!(dec_l.len(), left.len());
        assert_eq!(dec_r.len(), right.len());
        // Verify both channels have non-trivial content
        assert!(dec_l.iter().any(|s| s.abs() > 0.1));
        assert!(dec_r.iter().any(|s| s.abs() > 0.1));
    }

    #[test]
    fn stereo_roundtrip_tone() {
        let config = Config::default();
        let freq = 1000.0;
        let num_frames = 20;
        let n = config.frame_samples * num_frames;
        let left: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / config.sample_rate as f32;
                0.8 * (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let right: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / config.sample_rate as f32;
                0.5 * (2.0 * std::f32::consts::PI * 2000.0 * t).sin()
            })
            .collect();
        let (out_l, out_r) = stereo_opus_roundtrip(&left, &right, &config).unwrap();
        // After trimming lookahead, output should be slightly shorter
        assert!(out_l.len() > 0);
        assert!(out_r.len() > 0);
        // Check that left channel is stronger than right (0.8 vs 0.5)
        let rms_l: f32 = (out_l.iter().map(|s| s * s).sum::<f32>() / out_l.len() as f32).sqrt();
        let rms_r: f32 = (out_r.iter().map(|s| s * s).sum::<f32>() / out_r.len() as f32).sqrt();
        assert!(rms_l > rms_r, "left ({}) should be louder than right ({})", rms_l, rms_r);
    }
}

// ── Stereo Opus encode/decode for CRAM ─────────────────────────

/// Encode stereo PCM to Opus packets frame by frame.
/// left and right must be same length and a multiple of frame_samples.
pub fn encode_stereo_frames(left: &[f32], right: &[f32], config: &Config) -> Result<Vec<Vec<u8>>, OpusError> {
    assert_eq!(left.len(), right.len(), "left and right must be same length");
    let sr = sample_rate(config)?;
    let mut encoder = OpusEncoder::new(sr, Channels::Stereo, Application::Audio)
        .map_err(|e| OpusError::Encoder(e.to_string()))?;

    encoder
        .set_bitrate(Bitrate::BitsPerSecond((config.opus_bitrate * 1000) as i32))
        .map_err(|e| OpusError::Encoder(e.to_string()))?;

    let frame_size = config.frame_samples;
    let num_frames = left.len() / frame_size;
    let mut packets = Vec::with_capacity(num_frames);

    // Max Opus packet size for one frame
    let mut output_buf = vec![0u8; 8000];

    // Interleaved buffer: [L0, R0, L1, R1, ...]
    let mut interleaved = vec![0.0f32; frame_size * 2];

    for i in 0..num_frames {
        let start = i * frame_size;

        // Interleave L/R
        for j in 0..frame_size {
            interleaved[j * 2] = left[start + j];
            interleaved[j * 2 + 1] = right[start + j];
        }

        // frame_size for stereo encode_float is frame_samples (not *2)
        // but the input buffer has frame_samples * 2 interleaved samples
        let len = encoder
            .encode_float(&interleaved, &mut output_buf)
            .map_err(|e| OpusError::Encoder(e.to_string()))?;

        packets.push(output_buf[..len].to_vec());
    }

    Ok(packets)
}

/// Decode stereo Opus packets back to separate left/right channels.
pub fn decode_stereo_frames(packets: &[Vec<u8>], config: &Config) -> Result<(Vec<f32>, Vec<f32>), OpusError> {
    let sr = sample_rate(config)?;
    let mut decoder = OpusDecoder::new(sr, Channels::Stereo)
        .map_err(|e| OpusError::Decoder(e.to_string()))?;

    let frame_size = config.frame_samples;
    let mut left = Vec::with_capacity(packets.len() * frame_size);
    let mut right = Vec::with_capacity(packets.len() * frame_size);
    // Output buffer: frame_samples * 2 for stereo interleaved
    let mut output_buf = vec![0.0f32; frame_size * 2];

    for packet in packets {
        // decode_float returns number of samples per channel
        let decoded = decoder
            .decode_float(Some(packet), &mut output_buf, false)
            .map_err(|e| OpusError::Decoder(e.to_string()))?;

        // De-interleave
        for j in 0..decoded {
            left.push(output_buf[j * 2]);
            right.push(output_buf[j * 2 + 1]);
        }
    }

    Ok((left, right))
}

/// Stereo encoder lookahead in samples (per channel).
pub fn stereo_encoder_lookahead(config: &Config) -> Result<usize, OpusError> {
    let sr = sample_rate(config)?;
    let encoder = OpusEncoder::new(sr, Channels::Stereo, Application::Audio)
        .map_err(|e| OpusError::Encoder(e.to_string()))?;
    Ok(encoder.lookahead().map_err(|e| OpusError::Encoder(e.to_string()))? as usize)
}

/// Full stereo roundtrip: left+right PCM -> Opus -> left+right PCM.
/// Trims encoder lookahead from both channels.
pub fn stereo_opus_roundtrip(left: &[f32], right: &[f32], config: &Config) -> Result<(Vec<f32>, Vec<f32>), OpusError> {
    let packets = encode_stereo_frames(left, right, config)?;
    let (dec_left, dec_right) = decode_stereo_frames(&packets, config)?;
    let skip = stereo_encoder_lookahead(config)?;
    let left_out = if dec_left.len() > skip {
        dec_left[skip..].to_vec()
    } else {
        dec_left
    };
    let right_out = if dec_right.len() > skip {
        dec_right[skip..].to_vec()
    } else {
        dec_right
    };
    Ok((left_out, right_out))
}
