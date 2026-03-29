use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use yip::config::{Config, Depth};

#[derive(Clone, ValueEnum)]
enum DepthArg {
    /// 1-bit, 2-tone FSK — proven zero errors
    Binary,
    /// 2-bit, 4-tone FSK — 2x throughput (default)
    Quad,
    /// 4-bit, 16-tone FSK — 4x throughput
    Hex16,
}

#[derive(Parser)]
#[command(name = "yote")]
#[command(about = "Data-over-Opus file codec — part of the (co)yote protocol family.")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pack file into <file>.yip (compress + encode)
    Yip {
        /// File to pack
        path: PathBuf,
        /// Opus bitrate in kbps
        #[arg(long, default_value = "128")]
        bitrate: u32,
        /// Modulation depth
        #[arg(long, value_enum, default_value = "quad")]
        depth: DepthArg,
        /// Disable zstd compression
        #[arg(long)]
        no_compress: bool,
    },
    /// Unpack .yip back to original file
    Unyip {
        /// .yip file to unpack
        file: PathBuf,
    },
    /// Show .yip file metadata
    Info {
        /// .yip file to inspect
        file: PathBuf,
    },
    /// Opus roundtrip test (encode → Opus → decode)
    Test {
        /// Message to test
        message: String,
        /// Opus bitrate in kbps
        #[arg(long, default_value = "128")]
        bitrate: u32,
        /// Modulation depth
        #[arg(long, value_enum, default_value = "quad")]
        depth: DepthArg,
    },
    /// Show throughput and capacity stats
    Stats,
}

fn make_config(depth: &DepthArg, bitrate: u32) -> Config {
    let mut config = match depth {
        DepthArg::Binary => Config::conservative(),
        DepthArg::Quad => Config::default(),
        DepthArg::Hex16 => Config::for_depth(Depth::Hex16),
    };
    config.opus_bitrate = bitrate;
    config
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Yip { path, bitrate, depth, no_compress } => {
            cmd_yip(&path, bitrate, &depth, !no_compress);
        }
        Commands::Unyip { file } => {
            cmd_unyip(&file);
        }
        Commands::Info { file } => {
            cmd_info(&file);
        }
        Commands::Test { message, bitrate, depth } => {
            cmd_test(&message, bitrate, &depth);
        }
        Commands::Stats => {
            cmd_stats();
        }
    }
}

fn cmd_yip(path: &PathBuf, bitrate: u32, depth: &DepthArg, compress: bool) {
    if path.is_dir() {
        eprintln!("Error: directory packing not yet supported. Please pass a file.");
        std::process::exit(1);
    }

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error reading {}: {}", path.display(), e);
            std::process::exit(1);
        }
    };

    let filename = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data");

    let config = make_config(depth, bitrate);
    let output = PathBuf::from(format!("{}.yip", path.display()));

    match yip::io::encode_to_yip(&data, &output, &config, compress, filename) {
        Ok(()) => {
            let out_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
            let ratio = if data.is_empty() { 0.0 } else { out_size as f64 / data.len() as f64 };
            println!("{} -> {} ({} bytes -> {} bytes, {:.1}:1)",
                filename, output.display(), data.len(), out_size, ratio);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_unyip(file: &PathBuf) {
    match yip::io::decode_from_yip(file) {
        Ok((data, wire)) => {
            let output_name = if !wire.filename.is_empty() {
                wire.filename.clone()
            } else {
                // Strip .yip extension
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output")
                    .to_string()
            };

            let output_path = file.parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(&output_name);

            if let Err(e) = std::fs::write(&output_path, &data) {
                eprintln!("Error writing {}: {}", output_path.display(), e);
                std::process::exit(1);
            }

            let in_size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            println!("{} -> {} ({} bytes -> {} bytes)",
                file.display(), output_path.display(), in_size, data.len());
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_info(file: &PathBuf) {
    let file_data = match std::fs::read(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error reading {}: {}", file.display(), e);
            std::process::exit(1);
        }
    };

    if file_data.len() < 12 || &file_data[..3] != b"YIP" {
        eprintln!("Error: not a .yip file");
        std::process::exit(1);
    }

    let file_version = file_data[3];
    let (wire, _) = match yip::config::WireHeader::from_bytes(&file_data[4..]) {
        Some(w) => w,
        None => {
            eprintln!("Error: invalid wire header");
            std::process::exit(1);
        }
    };

    let file_size = file_data.len();
    let compress_str = match wire.compression {
        0 => "none",
        1 => "zstd",
        _ => "unknown",
    };

    println!("YIP file: {}", file.display());
    println!("  File version:  {}", file_version);
    println!("  Wire version:  {}", wire.version);
    println!("  Depth:         {}", wire.depth);
    println!("  Bins:          {}", wire.n_bins);
    println!("  Bin spacing:   {} Hz", wire.bin_spacing_hz);
    println!("  Base freq:     {} Hz", wire.base_freq_hz);
    println!("  Compression:   {}", compress_str);
    if !wire.filename.is_empty() {
        println!("  Filename:      {}", wire.filename);
    }
    println!("  File size:     {} bytes", file_size);
}

fn cmd_test(message: &str, bitrate: u32, depth: &DepthArg) {
    let config = make_config(depth, bitrate);
    let data = message.as_bytes();

    println!("Testing Opus roundtrip: {} bytes at {} kbps [{}]",
        data.len(), bitrate, config.depth);
    println!("Throughput: {} bps = {} B/s",
        config.throughput_bps(), config.throughput_bytes());

    let start = std::time::Instant::now();
    match yip::quant::opus_roundtrip(data, &config) {
        Ok(recovered) => {
            let elapsed = start.elapsed();
            if recovered == data {
                println!("PASS — perfect roundtrip in {:.1}ms", elapsed.as_secs_f64() * 1000.0);
            } else {
                let errors = data.iter().zip(recovered.iter())
                    .filter(|(a, b)| a != b).count();
                eprintln!("FAIL — {} byte errors out of {}", errors, data.len());
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("FAIL — {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_stats() {
    for depth in [Depth::Binary, Depth::Quad, Depth::Hex16] {
        let config = match depth {
            Depth::Binary => Config::conservative(),
            Depth::Quad => Config::default(),
            Depth::Hex16 => Config::for_depth(Depth::Hex16),
        };
        let info = yip::quant::throughput(&config);
        println!("{}", config.depth);
        println!("  Bins:         {} x {} bits x {} frames/s",
            info.n_bins, config.depth.bits_per_bin(), info.frames_per_sec);
        println!("  Freq range:   {:.0} - {:.0} Hz", config.base_freq, config.max_freq());
        println!("  Throughput:   {} bps = {} bytes/sec", info.bps, info.bytes_per_sec);
        println!("  Overhead at 128 kbps:");
        for &size in &[100, 1000, 4096] {
            let (opus_bytes, ratio) = yip::quant::overhead(size, &config);
            println!("    {} bytes -> ~{} bytes ({:.0}:1)", size, opus_bytes, ratio);
        }
        println!();
    }
}
