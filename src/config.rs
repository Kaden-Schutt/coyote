/// Configuration for yip — part of the (co)yote protocol family.
///
/// Wire-compatible defaults optimized via autoresearch sweep:
/// Quad depth, 79 bins, 250 Hz spacing, 128 kbps Opus.

use std::fmt;

/// Modulation depth — how many bits each frequency bin carries per frame.
///
/// The quantization analogy:
///   Depth::Binary  = INT8  (safe, proven zero errors at 64kbps)
///   Depth::Quad    = INT4  (2x throughput, proven zero errors at 128kbps)
///   Depth::Hex16   = INT2  (4x throughput, no working configs found)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Depth {
    /// 1 bit per bin — two-tone FSK. Proven zero errors at 64 kbps.
    Binary = 1,
    /// 2 bits per bin — four-tone FSK. ~7,900 bps at 79 bins, 128 kbps.
    Quad = 2,
    /// 4 bits per bin — sixteen-tone FSK. No working configs found.
    Hex16 = 4,
}

impl Depth {
    /// Number of discrete tone levels per bin.
    pub fn levels(self) -> usize {
        1 << (self as u8)
    }

    /// Bits encoded per bin per frame.
    pub fn bits_per_bin(self) -> usize {
        self as u8 as usize
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Depth::Binary),
            2 => Some(Depth::Quad),
            4 => Some(Depth::Hex16),
            _ => None,
        }
    }
}

impl fmt::Display for Depth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Depth::Binary => write!(f, "binary (1-bit, 2-tone FSK)"),
            Depth::Quad => write!(f, "quad (2-bit, 4-tone FSK)"),
            Depth::Hex16 => write!(f, "hex16 (4-bit, 16-tone FSK)"),
        }
    }
}

/// Codec configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Sample rate in Hz. Must be 48000 for Opus.
    pub sample_rate: u32,
    /// Samples per frame (20ms at 48kHz = 960).
    pub frame_samples: usize,
    /// Number of frequency bins.
    pub n_bins: usize,
    /// Spacing between bin centers in Hz.
    pub bin_spacing: f32,
    /// Center frequency of the first bin in Hz.
    pub base_freq: f32,
    /// Modulation depth.
    pub depth: Depth,
    /// Opus bitrate in kbps.
    pub opus_bitrate: u32,
    /// Silence frames before data (codec settling).
    pub padding_frames: usize,
    /// Silence frames after data.
    pub trailing_frames: usize,
    /// Max frame offsets to scan for sync header.
    pub max_search_offset: usize,
    /// Sync magic bytes.
    pub sync_magic: [u8; 4],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            frame_samples: 960,
            n_bins: 79,
            bin_spacing: 250.0,
            base_freq: 200.0,
            depth: Depth::Quad,
            opus_bitrate: 128,
            padding_frames: 10,
            trailing_frames: 10,
            max_search_offset: 25,
            sync_magic: [0xDE, 0xAD, 0xBE, 0xEF],
        }
    }
}

impl Config {
    /// Conservative config: original sonicpack defaults for maximum compatibility.
    /// Binary, 48 bins, 250Hz, 64kbps. Proven zero errors at lowest bitrate.
    pub fn conservative() -> Self {
        Self {
            n_bins: 48,
            bin_spacing: 250.0,
            base_freq: 200.0,
            depth: Depth::Binary,
            opus_bitrate: 64,
            ..Self::default_layout()
        }
    }

    /// Low-bandwidth config: best Quad config at 96kbps.
    /// Good for constrained connections that can't do 128kbps.
    pub fn low_bandwidth() -> Self {
        Self {
            n_bins: 71,
            bin_spacing: 250.0,
            base_freq: 200.0,
            depth: Depth::Quad,
            opus_bitrate: 96,
            ..Self::default_layout()
        }
    }

    // Helper to avoid recursion in conservative/low_bandwidth
    fn default_layout() -> Self {
        Self {
            sample_rate: 48_000,
            frame_samples: 960,
            n_bins: 0,  // overridden by caller
            bin_spacing: 0.0,
            base_freq: 0.0,
            depth: Depth::Binary,
            opus_bitrate: 0,
            padding_frames: 10,
            trailing_frames: 10,
            max_search_offset: 25,
            sync_magic: [0xDE, 0xAD, 0xBE, 0xEF],
        }
    }

    /// Frame duration in seconds.
    pub fn frame_duration(&self) -> f32 {
        self.frame_samples as f32 / self.sample_rate as f32
    }

    /// Total bits per frame.
    pub fn bits_per_frame(&self) -> usize {
        self.n_bins * self.depth.bits_per_bin()
    }

    /// Bytes per frame (floor division).
    pub fn bytes_per_frame(&self) -> usize {
        self.bits_per_frame() / 8
    }

    /// Maximum frequency used.
    pub fn max_freq(&self) -> f32 {
        self.base_freq + (self.n_bins as f32 - 1.0) * self.bin_spacing
    }

    /// Center frequency of a bin.
    pub fn bin_center(&self, bin: usize) -> f32 {
        self.base_freq + bin as f32 * self.bin_spacing
    }

    /// Frequency for a given symbol in a given bin.
    ///
    /// Two-tone FSK (Python-compatible):
    ///   symbol 0 → center - spacing/6
    ///   symbol 1 → center + spacing/6
    ///
    /// Generalized multi-level:
    ///   offset = (symbol - (levels-1)/2) * (spacing / (levels+1))
    pub fn tone_freq(&self, bin: usize, symbol: usize) -> f32 {
        let center = self.bin_center(bin);
        let levels = self.depth.levels() as f32;
        let offset = (symbol as f32 - (levels - 1.0) / 2.0)
            * (self.bin_spacing / (levels + 1.0));
        center + offset
    }

    /// Data throughput in bits per second.
    pub fn throughput_bps(&self) -> usize {
        self.bits_per_frame() * (1000 / (self.frame_samples * 1000 / self.sample_rate as usize))
    }

    /// Data throughput in bytes per second.
    pub fn throughput_bytes(&self) -> usize {
        self.throughput_bps() / 8
    }

    /// Frames per second.
    pub fn frames_per_sec(&self) -> usize {
        self.sample_rate as usize / self.frame_samples
    }

    /// Minimum bin spacing (Hz) needed to resolve all tone levels
    /// within one frame duration. Based on Rayleigh criterion: Δf > 1/T.
    pub fn min_tone_spacing(&self) -> f32 {
        self.sample_rate as f32 / self.frame_samples as f32
    }

    /// Minimum bin spacing for the current depth.
    /// The tone spacing within a bin is `bin_spacing / (levels + 1)`.
    /// This must exceed `min_tone_spacing()`.
    pub fn min_bin_spacing(&self) -> f32 {
        let levels = self.depth.levels() as f32;
        self.min_tone_spacing() * (levels + 1.0)
    }

    /// Check if the current configuration can resolve all tone levels.
    pub fn is_valid(&self) -> bool {
        let tone_spacing = self.bin_spacing / (self.depth.levels() as f32 + 1.0);
        tone_spacing >= self.min_tone_spacing()
    }

    /// Return an auto-tuned config for the given depth.
    /// Adjusts bin_spacing and n_bins to be valid while maximizing throughput.
    pub fn for_depth(depth: Depth) -> Self {
        let base = Self::default();
        let min_spacing = {
            let levels = depth.levels() as f32;
            let min_tone = base.sample_rate as f32 / base.frame_samples as f32;
            // Add 20% margin for Opus distortion
            (min_tone * (levels + 1.0) * 1.2).ceil()
        };

        let bin_spacing = if min_spacing > base.bin_spacing {
            min_spacing
        } else {
            base.bin_spacing
        };

        // Compute how many bins fit in the usable frequency range (200 - 20000 Hz)
        let max_usable_freq = 20000.0;
        let n_bins = ((max_usable_freq - base.base_freq) / bin_spacing).floor() as usize;
        let n_bins = n_bins.max(4); // minimum 4 bins

        Self {
            depth,
            bin_spacing,
            n_bins,
            ..base
        }
    }
}

/// Wire header prepended to the first data frame.
///
/// v2 format (8 bytes, little-endian):
///   [version: u8] [depth: u8] [n_bins: u16] [bin_spacing: u16] [base_freq: u16]
///
/// v3 format (10 + filename_len bytes):
///   [version: u8] [depth: u8] [n_bins: u16] [bin_spacing: u16] [base_freq: u16]
///   [compression: u8] [filename_len: u8] [filename: N bytes]
///
/// This lets any decoder auto-detect the encoding parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireHeader {
    pub version: u8,
    pub depth: Depth,
    pub n_bins: u16,
    pub bin_spacing_hz: u16,
    pub base_freq_hz: u16,
    /// Compression: 0 = none, 1 = zstd.
    pub compression: u8,
    /// Original filename (stored in .yip files for unyip).
    pub filename: String,
}

impl WireHeader {
    /// Base size of the fixed fields (v2-compatible portion).
    pub const BASE_SIZE: usize = 8;
    pub const VERSION: u8 = 3; // v3 = yote release with compression + filename

    pub fn from_config(config: &Config) -> Self {
        Self {
            version: Self::VERSION,
            depth: config.depth,
            n_bins: config.n_bins as u16,
            bin_spacing_hz: config.bin_spacing as u16,
            base_freq_hz: config.base_freq as u16,
            compression: 0,
            filename: String::new(),
        }
    }

    /// Serialized size in bytes.
    pub fn serialized_size(&self) -> usize {
        Self::BASE_SIZE + 2 + self.filename.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let fname = self.filename.as_bytes();
        let mut buf = Vec::with_capacity(self.serialized_size());
        buf.push(self.version);
        buf.push(self.depth as u8);
        buf.extend_from_slice(&self.n_bins.to_le_bytes());
        buf.extend_from_slice(&self.bin_spacing_hz.to_le_bytes());
        buf.extend_from_slice(&self.base_freq_hz.to_le_bytes());
        buf.push(self.compression);
        buf.push(fname.len() as u8);
        buf.extend_from_slice(fname);
        buf
    }

    /// Parse a wire header from a byte slice. Returns (header, bytes_consumed).
    /// Handles both v2 (8 bytes) and v3 (10 + filename_len bytes) formats.
    pub fn from_bytes(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < Self::BASE_SIZE {
            return None;
        }
        let version = buf[0];
        let depth = Depth::from_u8(buf[1])?;
        let n_bins = u16::from_le_bytes([buf[2], buf[3]]);
        let bin_spacing_hz = u16::from_le_bytes([buf[4], buf[5]]);
        let base_freq_hz = u16::from_le_bytes([buf[6], buf[7]]);

        if version <= 2 {
            return Some((Self {
                version,
                depth,
                n_bins,
                bin_spacing_hz,
                base_freq_hz,
                compression: 0,
                filename: String::new(),
            }, Self::BASE_SIZE));
        }

        // v3+: compression flag + filename
        if buf.len() < Self::BASE_SIZE + 2 {
            return None;
        }
        let compression = buf[8];
        let fname_len = buf[9] as usize;
        if buf.len() < Self::BASE_SIZE + 2 + fname_len {
            return None;
        }
        let filename = String::from_utf8_lossy(&buf[10..10 + fname_len]).to_string();

        Some((Self {
            version,
            depth,
            n_bins,
            bin_spacing_hz,
            base_freq_hz,
            compression,
            filename,
        }, Self::BASE_SIZE + 2 + fname_len))
    }

    /// Build a Config from this wire header.
    pub fn to_config(&self) -> Config {
        Config {
            n_bins: self.n_bins as usize,
            bin_spacing: self.bin_spacing_hz as f32,
            base_freq: self.base_freq_hz as f32,
            depth: self.depth,
            ..Config::default()
        }
    }
}
