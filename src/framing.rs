/// Packet framing for yip.
///
/// Wire format (little-endian, Python-compatible):
///   [0xDEADBEEF magic: 4B] [payload_length: u32 LE] [crc32: u32 LE] [payload...]
///
/// Optional wire header can be prepended for auto-detection of encoding params.

use crc32fast::Hasher;
use thiserror::Error;

use crate::config::{Config, WireHeader};

#[derive(Error, Debug)]
pub enum FrameError {
    #[error("sync magic not found")]
    NoSync,
    #[error("CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}")]
    CrcMismatch { expected: u32, actual: u32 },
    #[error("payload too short: need {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },
    #[error("invalid wire header")]
    BadWireHeader,
    #[error("decompression failed: {0}")]
    DecompressError(String),
}

/// Packet header size: 4 (magic) + 4 (length) + 4 (crc32) = 12 bytes.
pub const HEADER_SIZE: usize = 12;

/// Build a framed packet: sync + length + crc32 + payload.
/// All multi-byte values are LITTLE-ENDIAN (Python-compatible).
pub fn frame(payload: &[u8], config: &Config) -> Vec<u8> {
    let crc = compute_crc(payload);
    let len = payload.len() as u32;

    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&config.sync_magic);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Build a framed packet WITH wire header prepended.
/// If `compress` is true, payload is zstd-compressed before framing.
/// Format: [wire_header] [sync: 4B] [length: 4B] [crc32: 4B] [payload...]
pub fn frame_with_header(payload: &[u8], config: &Config, compress: bool, filename: &str) -> Vec<u8> {
    let processed = if compress {
        crate::compress::compress(payload)
    } else {
        payload.to_vec()
    };

    let mut wire = WireHeader::from_config(config);
    wire.compression = if compress { 1 } else { 0 };
    wire.filename = filename.to_string();

    let crc = compute_crc(&processed);
    let len = processed.len() as u32;

    let wire_bytes = wire.to_bytes();
    let mut out = Vec::with_capacity(wire_bytes.len() + HEADER_SIZE + processed.len());
    out.extend_from_slice(&wire_bytes);
    out.extend_from_slice(&config.sync_magic);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&processed);
    out
}

/// Extract and verify a framed payload from raw bytes.
pub fn deframe(data: &[u8], config: &Config) -> Result<Vec<u8>, FrameError> {
    // Find sync magic
    let sync_pos = find_sync(data, &config.sync_magic).ok_or(FrameError::NoSync)?;

    let remaining = &data[sync_pos..];
    if remaining.len() < HEADER_SIZE {
        return Err(FrameError::TooShort {
            need: HEADER_SIZE,
            got: remaining.len(),
        });
    }

    let length = u32::from_le_bytes([
        remaining[4], remaining[5], remaining[6], remaining[7],
    ]) as usize;
    let expected_crc = u32::from_le_bytes([
        remaining[8], remaining[9], remaining[10], remaining[11],
    ]);

    let payload_start = HEADER_SIZE;
    let payload_end = payload_start + length;

    if remaining.len() < payload_end {
        return Err(FrameError::TooShort {
            need: payload_end,
            got: remaining.len(),
        });
    }

    let payload = &remaining[payload_start..payload_end];
    let actual_crc = compute_crc(payload);

    if actual_crc != expected_crc {
        return Err(FrameError::CrcMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }

    Ok(payload.to_vec())
}

/// Try to extract a wire header from raw bytes (before the sync magic).
pub fn extract_wire_header(data: &[u8]) -> Option<(WireHeader, usize)> {
    WireHeader::from_bytes(data)
}

/// Extract wire header, deframe, and decompress (if compressed).
/// Returns (wire_header, original_payload).
pub fn deframe_with_header(data: &[u8], config: &Config) -> Result<(WireHeader, Vec<u8>), FrameError> {
    let (wire, offset) = extract_wire_header(data).ok_or(FrameError::BadWireHeader)?;
    let payload = deframe(&data[offset..], config)?;
    let result = if wire.compression == 1 {
        crate::compress::decompress(&payload)
            .map_err(|e| FrameError::DecompressError(e.to_string()))?
    } else {
        payload
    };
    Ok((wire, result))
}

fn find_sync(data: &[u8], magic: &[u8; 4]) -> Option<usize> {
    data.windows(4).position(|w| w == magic)
}

fn compute_crc(data: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn roundtrip() {
        let config = default_config();
        let payload = b"test payload";
        let framed = frame(payload, &config);
        let recovered = deframe(&framed, &config).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn detect_corruption() {
        let config = default_config();
        let payload = b"important data";
        let mut framed = frame(payload, &config);
        let last = framed.len() - 1;
        framed[last] ^= 0xFF;
        assert!(matches!(deframe(&framed, &config), Err(FrameError::CrcMismatch { .. })));
    }

    #[test]
    fn missing_sync() {
        let config = default_config();
        let garbage = vec![0u8; 100];
        assert!(matches!(deframe(&garbage, &config), Err(FrameError::NoSync)));
    }

    #[test]
    fn with_leading_garbage() {
        let config = default_config();
        let payload = b"hello";
        let framed = frame(payload, &config);
        let mut with_garbage = vec![0xAA, 0xBB, 0xCC];
        with_garbage.extend_from_slice(&framed);
        let recovered = deframe(&with_garbage, &config).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn little_endian_compat() {
        let config = default_config();
        let payload = b"endian test";
        let framed = frame(payload, &config);
        // Verify little-endian length encoding
        let len_bytes = &framed[4..8];
        let len = u32::from_le_bytes(len_bytes.try_into().unwrap());
        assert_eq!(len, payload.len() as u32);
    }

    #[test]
    fn wire_header_roundtrip() {
        let config = default_config();
        let header = WireHeader::from_config(&config);
        let bytes = header.to_bytes();
        let (recovered, size) = WireHeader::from_bytes(&bytes).unwrap();
        assert_eq!(header, recovered);
        assert_eq!(size, header.serialized_size());
    }

    #[test]
    fn framed_with_wire_header() {
        let config = default_config();
        let payload = b"with header";
        let framed = frame_with_header(payload, &config, false, "");

        let (wire, offset) = extract_wire_header(&framed).unwrap();
        assert_eq!(wire.version, WireHeader::VERSION);
        assert_eq!(wire.n_bins, 79);
        assert_eq!(wire.compression, 0);

        let recovered = deframe(&framed[offset..], &config).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn framed_with_compression() {
        let config = default_config();
        let payload = b"compressed payload test data for yote";
        let framed = frame_with_header(payload, &config, true, "test.txt");

        let (wire, decompressed) = deframe_with_header(&framed, &config).unwrap();
        assert_eq!(wire.compression, 1);
        assert_eq!(wire.filename, "test.txt");
        assert_eq!(decompressed, payload);
    }
}
