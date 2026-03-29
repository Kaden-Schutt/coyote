//! # yip
//!
//! Data-over-Opus codec using Parallel Tone-Position FSK.
//! Part of the (co)yote protocol family. Ride the codec, don't fight it.
//!
//! ## The Idea
//!
//! Opus preserves tonal peak positions — that's what makes music sound right.
//! Yip encodes data into those positions and recovers it with zero bit
//! errors after lossy compression. Every bin always emits a tone; the data
//! determines *which* tone. The codec always has energy to preserve.
//!
//! ## Modulation Depth (the quantization analogy)
//!
//! | Depth | Bits/bin | Throughput | Analogy | Risk |
//! |-------|----------|------------|---------|------|
//! | Binary | 1 | 3,950 bps (493 B/s) | INT8 | Zero errors proven |
//! | Quad | 2 | 7,900 bps (987 B/s) | INT4 | Zero errors at 128 kbps |
//! | Hex16 | 4 | — | INT2 | No working configs |
//!
//! ## Quick Start
//!
//! ```rust
//! use yip::config::Config;
//! use yip::quant::{wrap, unwrap};
//!
//! let config = Config::default();
//!
//! // Wrap: data → Opus-safe PCM
//! let pcm = wrap(b"hello from yip", &config);
//!
//! // Unwrap: PCM → data (works after Opus encode/decode too)
//! let data = unwrap(&pcm, &config).unwrap();
//! assert_eq!(data, b"hello from yip");
//! ```
//!
//! ## Native Opus (no ffmpeg)
//!
//! ```rust,no_run
//! use yip::config::Config;
//! use yip::quant::opus_roundtrip;
//!
//! let config = Config::default();
//! let data = b"survives Opus compression";
//! let recovered = opus_roundtrip(data, &config).unwrap();
//! assert_eq!(recovered, data);
//! ```
//!
//! ## Streaming
//!
//! ```rust,no_run
//! use yip::config::Config;
//! use yip::stream::StreamEncoder;
//!
//! let config = Config::default();
//! let mut encoder = StreamEncoder::new(config).unwrap();
//!
//! // Push data incrementally — get Opus packets out
//! let packets = encoder.push(b"token by token").unwrap();
//! let final_packets = encoder.finish().unwrap();
//! ```

pub mod config;
pub mod compress;
pub mod modulation;
pub mod framing;
pub mod opus;
pub mod stream;
pub mod quant;
pub mod io;
#[cfg(feature = "python")]
pub mod python;
pub mod cram;
pub mod pilot;
pub mod differential;
pub mod constellation;
pub mod tiers;
