/// File I/O for yip.
///
/// Supports:
///   - WAV files (via hound) for testing without Opus
///   - Yip native packet format (.yip) — native Opus, no ffmpeg needed
///   - Standard .opus files via ffmpeg (optional fallback)

use hound::{WavReader, WavSpec, WavWriter, SampleFormat};
use std::path::Path;
use std::process::Command;
use thiserror::Error;

use crate::config::Config;
use crate::opus;

#[derive(Error, Debug)]
pub enum IoError {
    #[error("WAV error: {0}")]
    Wav(#[from] hound::Error),
    #[error("Opus error: {0}")]
    Opus(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg not found — use .yip format for ffmpeg-free operation")]
    NoFfmpeg,
    #[error("ffmpeg error: {0}")]
    Ffmpeg(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("invalid file format")]
    BadFormat,
}

// ── WAV I/O ──────────────────────────────────────────────────

pub fn write_wav(path: &Path, samples: &[f32], config: &Config) -> Result<(), IoError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: config.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for &s in samples {
        let sample = (s * 30000.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}

pub fn read_wav(path: &Path) -> Result<Vec<f32>, IoError> {
    let reader = WavReader::open(path)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()?,
        SampleFormat::Int => {
            let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_val))
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    Ok(samples)
}

// ── Yip native format (.yip) ────────────────────────────────
// No ffmpeg needed. Pure Rust Opus encode/decode.

/// Encode data to yip native format (.yip file).
/// Format: [magic: "YIP\x03"] [wire_header] [opus_packets...]
///
/// If `compress` is true, data is zstd-compressed before encoding.
/// The `filename` is stored in the wire header for recovery on decode.
pub fn encode_to_yip(
    data: &[u8],
    path: &Path,
    config: &Config,
    compress: bool,
    filename: &str,
) -> Result<(), IoError> {
    let payload = if compress {
        crate::compress::compress(data)
    } else {
        data.to_vec()
    };

    let pcm = crate::quant::wrap(&payload, config);
    let packets = opus::encode_frames(&pcm, config)
        .map_err(|e| IoError::Opus(e.to_string()))?;

    let mut wire = crate::config::WireHeader::from_config(config);
    wire.compression = if compress { 1 } else { 0 };
    wire.filename = filename.to_string();
    let packet_bytes = opus::packets_to_bytes(&packets);

    let mut out = Vec::new();
    out.extend_from_slice(b"YIP\x03"); // magic + version
    out.extend_from_slice(&wire.to_bytes());
    out.extend_from_slice(&packet_bytes);

    std::fs::write(path, &out)?;
    Ok(())
}

/// Decode data from yip native format (.yip file).
/// Returns (data, wire_header) so the caller can access filename/compression info.
pub fn decode_from_yip(path: &Path) -> Result<(Vec<u8>, crate::config::WireHeader), IoError> {
    let file_data = std::fs::read(path)?;

    if file_data.len() < 12 || &file_data[..3] != b"YIP" {
        return Err(IoError::BadFormat);
    }

    let _file_version = file_data[3];
    let (wire, wire_size) = crate::config::WireHeader::from_bytes(&file_data[4..])
        .ok_or(IoError::BadFormat)?;
    let config = wire.to_config();

    let data_offset = 4 + wire_size;
    let packets = opus::bytes_to_packets(&file_data[data_offset..])
        .ok_or(IoError::BadFormat)?;
    let pcm = opus::decode_frames(&packets, &config)
        .map_err(|e| IoError::Opus(e.to_string()))?;

    // Trim encoder lookahead to restore frame alignment
    let skip = opus::encoder_lookahead(&config)
        .map_err(|e| IoError::Opus(e.to_string()))?;
    let trimmed = if pcm.len() > skip { &pcm[skip..] } else { &pcm };

    let payload = crate::quant::unwrap(trimmed, &config)
        .map_err(|e| IoError::Decode(e.to_string()))?;

    // Decompress if the wire header says it was compressed
    let data = if wire.compression == 1 {
        crate::compress::decompress(&payload)
            .map_err(|e| IoError::Decode(format!("zstd decompress: {}", e)))?
    } else {
        payload
    };

    Ok((data, wire))
}

// ── Standard Opus via ffmpeg (optional) ──────────────────────

/// Encode data to standard .opus file via ffmpeg.
pub fn encode_to_opus(
    data: &[u8],
    path: &Path,
    config: &Config,
) -> Result<(), IoError> {
    check_ffmpeg()?;
    let pcm = crate::quant::wrap(data, config);
    let tmp_wav = path.with_extension("tmp.wav");
    write_wav(&tmp_wav, &pcm, config)?;

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i", tmp_wav.to_str().unwrap(),
            "-c:a", "libopus",
            "-b:a", &format!("{}k", config.opus_bitrate),
            "-application", "audio",
            "-ar", &config.sample_rate.to_string(),
            "-ac", "1",
            path.to_str().unwrap(),
        ])
        .output()?;

    std::fs::remove_file(&tmp_wav)?;

    if !output.status.success() {
        return Err(IoError::Ffmpeg(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(())
}

/// Decode data from standard .opus file via ffmpeg.
pub fn decode_from_opus(
    path: &Path,
    config: &Config,
) -> Result<Vec<u8>, IoError> {
    check_ffmpeg()?;
    let tmp_wav = path.with_extension("tmp.wav");

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i", path.to_str().unwrap(),
            "-ar", &config.sample_rate.to_string(),
            "-ac", "1",
            "-c:a", "pcm_s16le",
            tmp_wav.to_str().unwrap(),
        ])
        .output()?;

    if !output.status.success() {
        return Err(IoError::Ffmpeg(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let pcm = read_wav(&tmp_wav)?;
    std::fs::remove_file(&tmp_wav)?;

    crate::quant::unwrap(&pcm, config)
        .map_err(|e| IoError::Decode(e.to_string()))
}

fn check_ffmpeg() -> Result<(), IoError> {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|_| ())
        .map_err(|_| IoError::NoFfmpeg)
}
