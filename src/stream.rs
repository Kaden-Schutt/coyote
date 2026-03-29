/// Streaming encoder/decoder for real-time data-over-Opus.
///
/// The StreamEncoder accepts bytes incrementally and emits Opus packets.
/// The StreamDecoder accepts Opus packets and emits decoded bytes.
/// Both operate frame-by-frame with 20ms latency.
///
/// This is how you'd use yip as a real-time transport:
///   - Feed LLM tokens into StreamEncoder as they're generated
///   - Send Opus packets over WebRTC/Discord/phone call
///   - StreamDecoder on the other end recovers the tokens

use audiopus::{
    coder::{Encoder as OpusEncoder, Decoder as OpusDecoder},
    Application, Bitrate, Channels, SampleRate,
};

use crate::config::Config;
use crate::modulation::{
    bytes_to_symbols, modulate_frame, silence_frame,
};

/// Streaming encoder: bytes in → Opus packets out.
pub struct StreamEncoder {
    config: Config,
    opus: OpusEncoder,
    symbol_buffer: Vec<usize>,
    opus_buf: Vec<u8>,
    started: bool,
    padding_sent: usize,
}

impl StreamEncoder {
    pub fn new(config: Config) -> Result<Self, String> {
        let sr = match config.sample_rate {
            48000 => SampleRate::Hz48000,
            _ => return Err(format!("unsupported sample rate: {}", config.sample_rate)),
        };

        let mut opus = OpusEncoder::new(sr, Channels::Mono, Application::Audio)
            .map_err(|e| e.to_string())?;

        opus.set_bitrate(Bitrate::BitsPerSecond((config.opus_bitrate * 1000) as i32))
            .map_err(|e| e.to_string())?;

        Ok(Self {
            config,
            opus,
            symbol_buffer: Vec::new(),
            opus_buf: vec![0u8; 4000],
            started: false,
            padding_sent: 0,
        })
    }

    /// Push bytes into the encoder. Returns any complete Opus packets.
    ///
    /// Packets are emitted as soon as enough data fills a frame.
    /// Leading padding frames are sent automatically on first call.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let mut packets = Vec::new();

        // Send leading padding on first push
        if !self.started {
            self.started = true;
            while self.padding_sent < self.config.padding_frames {
                let silence = silence_frame(&self.config);
                let pkt = self.encode_pcm_frame(&silence)?;
                packets.push(pkt);
                self.padding_sent += 1;
            }
        }

        // Convert bytes to symbols and buffer
        let symbols = bytes_to_symbols(data, &self.config);
        self.symbol_buffer.extend(symbols);

        // Emit frames for every complete n_bins chunk
        while self.symbol_buffer.len() >= self.config.n_bins {
            let frame_symbols: Vec<usize> =
                self.symbol_buffer.drain(..self.config.n_bins).collect();
            let pcm = modulate_frame(&frame_symbols, &self.config);
            let pkt = self.encode_pcm_frame(&pcm)?;
            packets.push(pkt);
        }

        Ok(packets)
    }

    /// Flush remaining data and send trailing padding.
    /// Call this when all data has been pushed.
    pub fn finish(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let mut packets = Vec::new();

        // Flush remaining symbols (pad to frame boundary)
        if !self.symbol_buffer.is_empty() {
            self.symbol_buffer.resize(self.config.n_bins, 0);
            let frame_symbols: Vec<usize> =
                self.symbol_buffer.drain(..).collect();
            let pcm = modulate_frame(&frame_symbols, &self.config);
            let pkt = self.encode_pcm_frame(&pcm)?;
            packets.push(pkt);
        }

        // Trailing padding
        for _ in 0..self.config.trailing_frames {
            let silence = silence_frame(&self.config);
            let pkt = self.encode_pcm_frame(&silence)?;
            packets.push(pkt);
        }

        Ok(packets)
    }

    fn encode_pcm_frame(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String> {
        let len = self
            .opus
            .encode_float(pcm, &mut self.opus_buf)
            .map_err(|e| e.to_string())?;
        Ok(self.opus_buf[..len].to_vec())
    }
}

/// Streaming decoder: Opus packets in → bytes out.
pub struct StreamDecoder {
    config: Config,
    opus: OpusDecoder,
    pcm_buf: Vec<f32>,
    frame_count: usize,
    skip_samples: usize,
    _synced: bool,
    _sync_offset: usize,
}

impl StreamDecoder {
    pub fn new(config: Config) -> Result<Self, String> {
        let sr = match config.sample_rate {
            48000 => SampleRate::Hz48000,
            _ => return Err(format!("unsupported sample rate: {}", config.sample_rate)),
        };

        let opus = OpusDecoder::new(sr, Channels::Mono)
            .map_err(|e| e.to_string())?;

        let skip_samples = crate::opus::encoder_lookahead(&config)
            .map_err(|e| e.to_string())? as usize;

        Ok(Self {
            config,
            opus,
            pcm_buf: Vec::new(),
            frame_count: 0,
            skip_samples,
            _synced: false,
            _sync_offset: 0,
        })
    }

    /// Push an Opus packet. Returns any decoded data bytes.
    ///
    /// The decoder accumulates PCM (trimming encoder lookahead) until
    /// it can find the sync header, then streams decoded bytes as frames arrive.
    pub fn push(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let mut output = vec![0.0f32; self.config.frame_samples];
        let decoded = self
            .opus
            .decode_float(Some(packet), &mut output, false)
            .map_err(|e| e.to_string())?;

        self.pcm_buf.extend_from_slice(&output[..decoded]);
        self.frame_count += 1;

        // Trim the encoder lookahead before attempting decode
        let trimmed = if self.pcm_buf.len() > self.skip_samples {
            &self.pcm_buf[self.skip_samples..]
        } else {
            return Ok(None);
        };

        if let Some(data) = crate::modulation::decode_pcm(trimmed, &self.config) {
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing;

    #[test]
    fn stream_roundtrip() {
        let config = Config::default();
        let payload = b"streaming test data!";
        let framed = framing::frame(payload, &config);

        // Encode
        let mut encoder = StreamEncoder::new(config.clone()).unwrap();
        let mut all_packets = encoder.push(&framed).unwrap();
        all_packets.extend(encoder.finish().unwrap());

        // Decode
        let mut decoder = StreamDecoder::new(config).unwrap();
        let mut result = None;
        for pkt in &all_packets {
            if let Ok(Some(data)) = decoder.push(pkt) {
                result = Some(data);
            }
        }

        assert_eq!(result.unwrap(), payload);
    }
}
