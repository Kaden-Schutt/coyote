/// PyO3 bindings for the yip/yote codec.
///
/// Exposes the core Rust API to Python as the `_yote` native module.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use std::path::Path;

use crate::config::{Config, Depth};
use crate::io;
use crate::modulation;
use crate::opus;
use crate::quant;
use crate::yawp;

fn parse_depth(depth: &str) -> PyResult<Depth> {
    match depth.to_lowercase().as_str() {
        "binary" => Ok(Depth::Binary),
        "quad" => Ok(Depth::Quad),
        "hex16" => Ok(Depth::Hex16),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            format!("invalid depth '{}': use 'binary', 'quad', or 'hex16'", depth),
        )),
    }
}

fn make_config(bitrate: u32, depth: &str) -> PyResult<Config> {
    let d = parse_depth(depth)?;
    let mut config = match d {
        Depth::Binary => Config::conservative(),
        Depth::Quad => Config::default(),
        Depth::Hex16 => Config::for_depth(Depth::Hex16),
    };
    config.opus_bitrate = bitrate;
    Ok(config)
}

/// Compress + encode a file to .yip format.
/// Returns the output file path.
#[pyfunction]
#[pyo3(signature = (path, bitrate = 128, depth = "quad"))]
fn yip(path: &str, bitrate: u32, depth: &str) -> PyResult<String> {
    let input = Path::new(path);
    let data = std::fs::read(input).map_err(|e| {
        pyo3::exceptions::PyIOError::new_err(format!("reading {}: {}", path, e))
    })?;

    let filename = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data");

    let config = make_config(bitrate, depth)?;
    let output = format!("{}.yip", path);

    io::encode_to_yip(&data, Path::new(&output), &config, true, filename).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("encode error: {}", e))
    })?;

    Ok(output)
}

/// Decode + decompress a .yip file back to its original.
/// Returns the restored file path.
#[pyfunction]
fn unyip(path: &str) -> PyResult<String> {
    let input = Path::new(path);
    let (data, wire) = io::decode_from_yip(input).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("decode error: {}", e))
    })?;

    let output_name = if !wire.filename.is_empty() {
        wire.filename.clone()
    } else {
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string()
    };

    let output_path = input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&output_name);

    std::fs::write(&output_path, &data).map_err(|e| {
        pyo3::exceptions::PyIOError::new_err(format!("writing {}: {}", output_path.display(), e))
    })?;

    Ok(output_path.to_string_lossy().to_string())
}

/// Return metadata dict from a .yip file.
#[pyfunction]
fn info(py: Python<'_>, path: &str) -> PyResult<Py<PyDict>> {
    let file_data = std::fs::read(path).map_err(|e| {
        pyo3::exceptions::PyIOError::new_err(format!("reading {}: {}", path, e))
    })?;

    if file_data.len() < 12 || &file_data[..3] != b"YIP" {
        return Err(pyo3::exceptions::PyValueError::new_err("not a .yip file"));
    }

    let file_version = file_data[3];
    let (wire, _) = crate::config::WireHeader::from_bytes(&file_data[4..]).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err("invalid wire header")
    })?;

    let dict = PyDict::new_bound(py);
    dict.set_item("file_version", file_version)?;
    dict.set_item("wire_version", wire.version)?;
    dict.set_item("depth", format!("{}", wire.depth))?;
    dict.set_item("n_bins", wire.n_bins)?;
    dict.set_item("bin_spacing_hz", wire.bin_spacing_hz)?;
    dict.set_item("base_freq_hz", wire.base_freq_hz)?;
    dict.set_item("compression", match wire.compression {
        0 => "none",
        1 => "zstd",
        _ => "unknown",
    })?;
    dict.set_item("filename", &wire.filename)?;
    dict.set_item("file_size", file_data.len())?;

    Ok(dict.into())
}

/// Encode bytes to a list of Opus packets.
#[pyfunction]
#[pyo3(signature = (data, bitrate = 128))]
fn encode(data: &[u8], bitrate: u32) -> PyResult<Vec<Vec<u8>>> {
    let mut config = Config::default();
    config.opus_bitrate = bitrate;
    quant::encode_to_packets(data, &config).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("encode error: {}", e))
    })
}

/// Decode a list of Opus packets back to bytes.
#[pyfunction]
fn decode(py: Python<'_>, packets: Vec<Vec<u8>>) -> PyResult<Py<PyBytes>> {
    let config = Config::default();
    let data = quant::decode_from_packets(&packets, &config).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("decode error: {}", e))
    })?;
    Ok(PyBytes::new_bound(py, &data).into())
}

/// Modulate + Opus encode one frame of symbols. Returns an Opus packet.
#[pyfunction]
fn encode_frame(symbols: Vec<usize>) -> PyResult<Vec<u8>> {
    let config = Config::default();
    let pcm = modulation::modulate_frame(&symbols, &config);

    let packets = opus::encode_frames(&pcm, &config).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("opus encode error: {}", e))
    })?;

    packets
        .into_iter()
        .next()
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("no packet produced"))
}

/// Opus decode + demodulate one frame. Returns symbol list.
#[pyfunction]
fn decode_frame(packet: Vec<u8>) -> PyResult<Vec<usize>> {
    let config = Config::default();
    let pcm = opus::decode_frames(&[packet], &config).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("opus decode error: {}", e))
    })?;

    Ok(modulation::demodulate_frame(&pcm, &config))
}

/// Return throughput stats as a dict.
#[pyfunction]
fn stats(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new_bound(py);

    for (name, config) in [
        ("binary", Config::conservative()),
        ("quad", Config::default()),
        ("hex16", Config::for_depth(Depth::Hex16)),
    ] {
        let info = quant::throughput(&config);
        let inner = PyDict::new_bound(py);
        inner.set_item("depth", format!("{}", info.depth))?;
        inner.set_item("n_bins", info.n_bins)?;
        inner.set_item("bps", info.bps)?;
        inner.set_item("bytes_per_sec", info.bytes_per_sec)?;
        inner.set_item("tokens_per_sec_utf8", info.tokens_per_sec_utf8)?;
        inner.set_item("tokens_per_sec_id16", info.tokens_per_sec_id16)?;
        inner.set_item("frames_per_sec", info.frames_per_sec)?;
        dict.set_item(name, inner)?;
    }

    Ok(dict.into())
}

/// FFT decode a PCM frame with confidence scores.
///
/// Returns (symbols, confidences, magnitudes):
///   symbols:      list[int]   — best symbol per bin [79]
///   confidences:  list[float] — top/second magnitude ratio [79]
///   magnitudes:   list[list[float]] — raw energies [79][4]
#[pyfunction]
#[pyo3(signature = (pcm, bitrate = 128, depth = "quad"))]
fn yawp_decode(pcm: Vec<f32>, bitrate: u32, depth: &str) -> PyResult<(Vec<i32>, Vec<f32>, Vec<Vec<f32>>)> {
    let config = make_config(bitrate, depth)?;
    if pcm.len() != config.frame_samples {
        return Err(pyo3::exceptions::PyValueError::new_err(
            format!("expected {} samples, got {}", config.frame_samples, pcm.len()),
        ));
    }

    let result = yawp::fft_decode_with_confidence(&pcm, &config);
    Ok((result.symbols, result.confidences, result.magnitudes))
}

/// Convert symbols to bits. Each symbol is unpacked MSB-first.
/// Returns list[float] of 0.0/1.0 values.
#[pyfunction]
#[pyo3(signature = (symbols, bits_per_bin = 2))]
fn yawp_symbols_to_bits(symbols: Vec<i32>, bits_per_bin: usize) -> Vec<f32> {
    yawp::symbols_to_bits(&symbols, bits_per_bin)
}

/// Decode a PCM frame with neural correction (FFT + corrector MLP).
/// Returns bits [158] as list of floats.
#[pyfunction]
#[pyo3(signature = (pcm, bitrate_idx = 2, bitrate = 128, depth = "quad"))]
fn yawp_correct(pcm: Vec<f32>, bitrate_idx: usize, bitrate: u32, depth: &str) -> PyResult<Vec<f32>> {
    let config = make_config(bitrate, depth)?;
    if pcm.len() != config.frame_samples {
        return Err(pyo3::exceptions::PyValueError::new_err(
            format!("expected {} samples, got {}", config.frame_samples, pcm.len()),
        ));
    }
    Ok(yawp::decode_frame_corrected(&pcm, &config, bitrate_idx))
}

/// The native `_yote` Python module.
#[pymodule]
fn _yote(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(yip, m)?)?;
    m.add_function(wrap_pyfunction!(unyip, m)?)?;
    m.add_function(wrap_pyfunction!(info, m)?)?;
    m.add_function(wrap_pyfunction!(encode, m)?)?;
    m.add_function(wrap_pyfunction!(decode, m)?)?;
    m.add_function(wrap_pyfunction!(encode_frame, m)?)?;
    m.add_function(wrap_pyfunction!(decode_frame, m)?)?;
    m.add_function(wrap_pyfunction!(stats, m)?)?;
    m.add_function(wrap_pyfunction!(yawp_decode, m)?)?;
    m.add_function(wrap_pyfunction!(yawp_symbols_to_bits, m)?)?;
    m.add_function(wrap_pyfunction!(yawp_correct, m)?)?;
    Ok(())
}
