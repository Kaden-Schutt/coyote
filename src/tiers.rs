/// Bitrate tier constructors — predefined configs for common Opus bitrates.
///
/// Each tier is validated via autoresearch sweeps (11,000+ trials).
/// Tiers return either a PilotConfig (absolute encoding) or a DiffConfig
/// (differential encoding), depending on which achieves higher throughput
/// at that bitrate.

use crate::config::Config;
use crate::differential::{DiffConfig, DiffMode};
use crate::pilot::{InterpMethod, PilotConfig};

/// Bitrate tier — predefined configs for common Opus bitrates.
/// Each tier is validated via autoresearch sweeps (11,000+ trials).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// 256 kbps — max throughput
    Ultra,
    /// 192 kbps — near-max
    High,
    /// 128 kbps — reliable default
    Standard,
    /// 96 kbps — broad compatibility
    Medium,
    /// 64 kbps — max compatibility
    Low,
    /// 48 kbps — experimental, unreliable
    Minimal,
}

impl Tier {
    /// Opus bitrate in kbps for this tier.
    pub fn bitrate_kbps(&self) -> u32 {
        match self {
            Tier::Ultra => 256,
            Tier::High => 192,
            Tier::Standard => 128,
            Tier::Medium => 96,
            Tier::Low => 64,
            Tier::Minimal => 48,
        }
    }

    /// Expected raw throughput in bits per second (zero errors verified).
    pub fn throughput_bps(&self) -> usize {
        match self {
            Tier::Ultra => 13_500,
            Tier::High => 12_200,
            Tier::Standard => 9_450,
            Tier::Medium => 6_100,
            Tier::Low => 2_700,
            Tier::Minimal => 0, // unreliable — no verified throughput
        }
    }

    /// Expected raw throughput in bytes per second.
    pub fn throughput_bytes(&self) -> usize {
        match self {
            Tier::Ultra => 1_687,
            Tier::High => 1_525,
            Tier::Standard => 1_181,
            Tier::Medium => 762,
            Tier::Low => 337,
            Tier::Minimal => 0,
        }
    }

    /// Whether this tier uses differential encoding.
    pub fn uses_diff(&self) -> bool {
        matches!(self, Tier::Ultra | Tier::High)
    }

    /// Whether this tier is marked as experimental/unreliable.
    pub fn is_experimental(&self) -> bool {
        matches!(self, Tier::Minimal)
    }

    /// Get the PilotConfig for this tier.
    pub fn pilot_config(&self) -> PilotConfig {
        let (n_bins, bin_spacing, pilot_spacing, amp_levels, phase_levels, interp, bitrate) =
            match self {
                Tier::Ultra => (60, 200.0, 10, 16, 2, InterpMethod::NearestNeighbor, 256),
                Tier::High => (70, 200.0, 8, 8, 2, InterpMethod::NearestNeighbor, 192),
                Tier::Standard => (70, 200.0, 10, 4, 2, InterpMethod::Linear, 128),
                Tier::Medium => (70, 200.0, 8, 2, 2, InterpMethod::Linear, 96),
                Tier::Low => (60, 250.0, 10, 2, 1, InterpMethod::Linear, 64),
                Tier::Minimal => (60, 200.0, 8, 2, 1, InterpMethod::NearestNeighbor, 48),
            };

        PilotConfig {
            config: Config {
                n_bins,
                bin_spacing,
                base_freq: 200.0,
                opus_bitrate: bitrate,
                ..Config::default()
            },
            pilot_spacing,
            amp_levels,
            phase_levels,
            pilot_amplitude: 1.0,
            interp_method: interp,
        }
    }

    /// Get the DiffConfig for this tier.
    ///
    /// # Panics
    /// Panics if this tier does not use differential encoding (`!self.uses_diff()`).
    pub fn diff_config(&self) -> DiffConfig {
        assert!(
            self.uses_diff(),
            "Tier::{:?} does not use differential encoding",
            self
        );

        let (recal_interval, diff_mode) = match self {
            Tier::Ultra => (0, DiffMode::Both),
            Tier::High => (0, DiffMode::AmpOnly),
            _ => unreachable!(),
        };

        DiffConfig {
            pilot: self.pilot_config(),
            recal_interval,
            diff_mode,
        }
    }

    /// Auto-select the best tier for a given Opus bitrate.
    /// Picks the highest tier that fits within the bitrate.
    pub fn for_bitrate(kbps: u32) -> Tier {
        if kbps >= 256 {
            Tier::Ultra
        } else if kbps >= 192 {
            Tier::High
        } else if kbps >= 128 {
            Tier::Standard
        } else if kbps >= 96 {
            Tier::Medium
        } else if kbps >= 64 {
            Tier::Low
        } else {
            Tier::Minimal
        }
    }

    /// All tiers from highest to lowest throughput.
    pub fn all() -> Vec<Tier> {
        vec![
            Tier::Ultra,
            Tier::High,
            Tier::Standard,
            Tier::Medium,
            Tier::Low,
            Tier::Minimal,
        ]
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Tier::Ultra => "ultra",
            Tier::High => "high",
            Tier::Standard => "standard",
            Tier::Medium => "medium",
            Tier::Low => "low",
            Tier::Minimal => "minimal",
        }
    }

    /// Full roundtrip: data -> encode -> Opus -> decode -> data.
    /// Dispatches to pilot_opus_roundtrip or diff_pilot_opus_roundtrip as appropriate.
    pub fn roundtrip(&self, data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if self.uses_diff() {
            let diff_cfg = self.diff_config();
            crate::differential::diff_pilot_opus_roundtrip(data, &diff_cfg)
        } else {
            let pilot_cfg = self.pilot_config();
            crate::pilot::pilot_opus_roundtrip(data, &pilot_cfg)
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}kbps): {:>6} bps = {:>5} B/s",
            self.name(),
            self.bitrate_kbps(),
            format_number(self.throughput_bps()),
            format_number(self.throughput_bytes()),
        )
    }
}

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ultra_roundtrip() {
        let tier = Tier::Ultra;
        let data = b"hello";
        let recovered = tier.roundtrip(data).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn tier_standard_roundtrip() {
        let tier = Tier::Standard;
        let data = b"hello";
        let recovered = tier.roundtrip(data).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn tier_low_roundtrip() {
        let tier = Tier::Low;
        let data = b"hello";
        let recovered = tier.roundtrip(data).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn for_bitrate_mapping() {
        assert_eq!(Tier::for_bitrate(200), Tier::High);
        assert_eq!(Tier::for_bitrate(128), Tier::Standard);
        assert_eq!(Tier::for_bitrate(256), Tier::Ultra);
        assert_eq!(Tier::for_bitrate(300), Tier::Ultra);
        assert_eq!(Tier::for_bitrate(96), Tier::Medium);
        assert_eq!(Tier::for_bitrate(64), Tier::Low);
        assert_eq!(Tier::for_bitrate(48), Tier::Minimal);
        assert_eq!(Tier::for_bitrate(32), Tier::Minimal);
    }

    #[test]
    fn all_tiers_display() {
        for tier in Tier::all() {
            let display = format!("{}", tier);
            println!("{}", display);
            assert!(!display.is_empty());
        }
    }

    #[test]
    fn throughput_values() {
        assert_eq!(Tier::Ultra.throughput_bps(), 13_500);
        assert_eq!(Tier::Ultra.throughput_bytes(), 1_687);
        assert_eq!(Tier::High.throughput_bps(), 12_200);
        assert_eq!(Tier::High.throughput_bytes(), 1_525);
        assert_eq!(Tier::Standard.throughput_bps(), 9_450);
        assert_eq!(Tier::Standard.throughput_bytes(), 1_181);
        assert_eq!(Tier::Medium.throughput_bps(), 6_100);
        assert_eq!(Tier::Medium.throughput_bytes(), 762);
        assert_eq!(Tier::Low.throughput_bps(), 2_700);
        assert_eq!(Tier::Low.throughput_bytes(), 337);
        assert_eq!(Tier::Minimal.throughput_bps(), 0);
        assert_eq!(Tier::Minimal.throughput_bytes(), 0);
    }

    #[test]
    fn uses_diff_correct() {
        assert!(Tier::Ultra.uses_diff());
        assert!(Tier::High.uses_diff());
        assert!(!Tier::Standard.uses_diff());
        assert!(!Tier::Medium.uses_diff());
        assert!(!Tier::Low.uses_diff());
        assert!(!Tier::Minimal.uses_diff());
    }

    #[test]
    fn is_experimental_correct() {
        assert!(!Tier::Ultra.is_experimental());
        assert!(!Tier::Standard.is_experimental());
        assert!(Tier::Minimal.is_experimental());
    }

    #[test]
    #[should_panic(expected = "does not use differential encoding")]
    fn diff_config_panics_for_non_diff_tier() {
        Tier::Standard.diff_config();
    }
}
