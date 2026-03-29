/// Zstd compression for yote file packing.
///
/// Default compression level 3 — good balance of speed and ratio.
/// Compression is applied before framing so the CRC covers compressed data.

use std::io;

/// Compress data with zstd at level 3.
pub fn compress(data: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(data, 3).expect("zstd compression should not fail on valid input")
}

/// Decompress zstd-compressed data.
pub fn decompress(data: &[u8]) -> io::Result<Vec<u8>> {
    zstd::stream::decode_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data = b"hello from yote compression";
        let compressed = compress(data);
        let recovered = decompress(&compressed).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn roundtrip_binary() {
        let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let compressed = compress(&data);
        let recovered = decompress(&compressed).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![0xAA; 10_000];
        let compressed = compress(&data);
        assert!(compressed.len() < data.len() / 10, "repetitive data should compress well");
    }

    #[test]
    fn empty_data() {
        let compressed = compress(b"");
        let recovered = decompress(&compressed).unwrap();
        assert!(recovered.is_empty());
    }
}
