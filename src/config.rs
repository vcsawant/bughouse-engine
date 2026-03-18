//! Engine configuration — all tunable parameters in one place.
//!
//! Parameters can be changed at runtime via `setoption name <Name> value <Value>`.
//! Option names are case-insensitive.

use log::debug;

/// All tunable engine parameters with sensible defaults.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    // ── Strategy thresholds (ms) ──
    pub instant_time_ms: u64,
    pub blitz_time_ms: u64,
    pub extended_advantage_ms: u64,
    pub both_active_blitz_ms: u64,

    // ── Style factors (cross-board weight multiplier) ──
    pub blitz_style_factor: f32,
    pub standard_style_factor: f32,
    pub extended_style_factor: f32,

    // ── Aggressiveness thresholds (centipawns) ──
    pub blitz_aggression: i32,
    pub standard_aggression: i32,
    pub extended_aggression: i32,

    // ── Time budget ──
    pub instant_budget_ms: u64,
    pub slow_budget_ms: u64,
    pub blitz_divisor: u64,
    pub blitz_cap_ms: u64,
    pub standard_divisor: u64,
    pub standard_cap_ms: u64,
    pub extended_divisor: u64,
    pub extended_cap_ms: u64,

    // ── Piece values (centipawns) ──
    pub pawn_value: i32,
    pub knight_value: i32,
    pub bishop_value: i32,
    pub rook_value: i32,
    pub queen_value: i32,

    // ── Reserve multipliers (percentage, e.g. 160 = 160%) ──
    pub knight_reserve_pct: i32,
    pub pawn_reserve_pct: i32,
    pub bishop_reserve_pct: i32,
    pub rook_reserve_pct: i32,
    pub queen_reserve_pct: i32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            // Strategy thresholds
            instant_time_ms: 3500,
            blitz_time_ms: 10000,
            extended_advantage_ms: 15000,
            both_active_blitz_ms: 15000,

            // Style factors
            blitz_style_factor: 0.5,
            standard_style_factor: 1.0,
            extended_style_factor: 1.5,

            // Aggressiveness thresholds
            blitz_aggression: 150,
            standard_aggression: 75,
            extended_aggression: 30,

            // Time budget
            instant_budget_ms: 50,
            slow_budget_ms: 200,
            blitz_divisor: 40,
            blitz_cap_ms: 500,
            standard_divisor: 30,
            standard_cap_ms: 2000,
            extended_divisor: 20,
            extended_cap_ms: 4000,

            // Piece values
            pawn_value: 100,
            knight_value: 320,
            bishop_value: 330,
            rook_value: 500,
            queen_value: 900,

            // Reserve multipliers
            knight_reserve_pct: 160,
            pawn_reserve_pct: 130,
            bishop_reserve_pct: 130,
            rook_reserve_pct: 120,
            queen_reserve_pct: 120,
        }
    }
}

impl EngineConfig {
    /// Apply a `setoption` command. Returns true if the option was recognized.
    /// Option names are case-insensitive.
    pub fn apply_option(&mut self, name: &str, value: &str) -> bool {
        let lower = name.to_lowercase();
        match lower.as_str() {
            // Strategy thresholds
            "instanttime" => { if let Ok(v) = value.parse() { self.instant_time_ms = v; return true; } }
            "blitztime" => { if let Ok(v) = value.parse() { self.blitz_time_ms = v; return true; } }
            "extendedadvantage" => { if let Ok(v) = value.parse() { self.extended_advantage_ms = v; return true; } }
            "bothactiveblitz" => { if let Ok(v) = value.parse() { self.both_active_blitz_ms = v; return true; } }

            // Style factors
            "blitzstylefactor" => { if let Ok(v) = value.parse() { self.blitz_style_factor = v; return true; } }
            "standardstylefactor" => { if let Ok(v) = value.parse() { self.standard_style_factor = v; return true; } }
            "extendedstylefactor" => { if let Ok(v) = value.parse() { self.extended_style_factor = v; return true; } }

            // Aggressiveness thresholds
            "blitzaggression" => { if let Ok(v) = value.parse() { self.blitz_aggression = v; return true; } }
            "standardaggression" => { if let Ok(v) = value.parse() { self.standard_aggression = v; return true; } }
            "extendedaggression" => { if let Ok(v) = value.parse() { self.extended_aggression = v; return true; } }

            // Time budget
            "instantbudget" => { if let Ok(v) = value.parse() { self.instant_budget_ms = v; return true; } }
            "slowbudget" => { if let Ok(v) = value.parse() { self.slow_budget_ms = v; return true; } }
            "blitzdivisor" => { if let Ok(v) = value.parse() { self.blitz_divisor = v; return true; } }
            "blitzcap" => { if let Ok(v) = value.parse() { self.blitz_cap_ms = v; return true; } }
            "standarddivisor" => { if let Ok(v) = value.parse() { self.standard_divisor = v; return true; } }
            "standardcap" => { if let Ok(v) = value.parse() { self.standard_cap_ms = v; return true; } }
            "extendeddivisor" => { if let Ok(v) = value.parse() { self.extended_divisor = v; return true; } }
            "extendedcap" => { if let Ok(v) = value.parse() { self.extended_cap_ms = v; return true; } }

            // Piece values
            "pawnvalue" => { if let Ok(v) = value.parse() { self.pawn_value = v; return true; } }
            "knightvalue" => { if let Ok(v) = value.parse() { self.knight_value = v; return true; } }
            "bishopvalue" => { if let Ok(v) = value.parse() { self.bishop_value = v; return true; } }
            "rookvalue" => { if let Ok(v) = value.parse() { self.rook_value = v; return true; } }
            "queenvalue" => { if let Ok(v) = value.parse() { self.queen_value = v; return true; } }

            // Reserve multipliers
            "knightreservepct" => { if let Ok(v) = value.parse() { self.knight_reserve_pct = v; return true; } }
            "pawnreservepct" => { if let Ok(v) = value.parse() { self.pawn_reserve_pct = v; return true; } }
            "bishopreservepct" => { if let Ok(v) = value.parse() { self.bishop_reserve_pct = v; return true; } }
            "rookreservepct" => { if let Ok(v) = value.parse() { self.rook_reserve_pct = v; return true; } }
            "queenreservepct" => { if let Ok(v) = value.parse() { self.queen_reserve_pct = v; return true; } }

            _ => {}
        }
        false
    }

    /// Get piece value from config.
    pub fn piece_value(&self, p: bughouse_chess::Piece) -> i32 {
        match p {
            bughouse_chess::Piece::Pawn => self.pawn_value,
            bughouse_chess::Piece::Knight => self.knight_value,
            bughouse_chess::Piece::Bishop => self.bishop_value,
            bughouse_chess::Piece::Rook => self.rook_value,
            bughouse_chess::Piece::Queen => self.queen_value,
            bughouse_chess::Piece::King => 0,
        }
    }

    /// Get reserve multiplier percentage from config.
    pub fn reserve_multiplier_pct(&self, p: bughouse_chess::Piece) -> i32 {
        match p {
            bughouse_chess::Piece::Knight => self.knight_reserve_pct,
            bughouse_chess::Piece::Pawn => self.pawn_reserve_pct,
            bughouse_chess::Piece::Bishop => self.bishop_reserve_pct,
            bughouse_chess::Piece::Rook => self.rook_reserve_pct,
            bughouse_chess::Piece::Queen => self.queen_reserve_pct,
            bughouse_chess::Piece::King => 100,
        }
    }

    /// Get style factor for a PlayStyle.
    pub fn style_factor(&self, style: crate::strategy::PlayStyle) -> f32 {
        match style {
            crate::strategy::PlayStyle::Instant => 0.0,
            crate::strategy::PlayStyle::Blitz => self.blitz_style_factor,
            crate::strategy::PlayStyle::Standard => self.standard_style_factor,
            crate::strategy::PlayStyle::Extended => self.extended_style_factor,
            crate::strategy::PlayStyle::Slow => 0.25,
        }
    }

    /// Get aggressiveness threshold for a PlayStyle.
    pub fn aggressiveness_threshold(&self, style: crate::strategy::PlayStyle) -> i32 {
        match style {
            crate::strategy::PlayStyle::Instant => i32::MAX,
            crate::strategy::PlayStyle::Blitz => self.blitz_aggression,
            crate::strategy::PlayStyle::Standard => self.standard_aggression,
            crate::strategy::PlayStyle::Extended => self.extended_aggression,
            crate::strategy::PlayStyle::Slow => 200,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.instant_time_ms, 3500);
        assert_eq!(cfg.pawn_value, 100);
        assert_eq!(cfg.knight_reserve_pct, 160);
        assert_eq!(cfg.blitz_style_factor, 0.5);
        assert_eq!(cfg.standard_aggression, 75);
        assert_eq!(cfg.blitz_divisor, 40);
    }

    #[test]
    fn apply_option_recognized() {
        let mut cfg = EngineConfig::default();
        assert!(cfg.apply_option("BlitzStyleFactor", "0.7"));
        assert_eq!(cfg.blitz_style_factor, 0.7);

        assert!(cfg.apply_option("PawnValue", "120"));
        assert_eq!(cfg.pawn_value, 120);

        assert!(cfg.apply_option("BlitzDivisor", "35"));
        assert_eq!(cfg.blitz_divisor, 35);
    }

    #[test]
    fn apply_option_case_insensitive() {
        let mut cfg = EngineConfig::default();
        assert!(cfg.apply_option("blitzstylefactor", "0.8"));
        assert_eq!(cfg.blitz_style_factor, 0.8);

        assert!(cfg.apply_option("PAWNVALUE", "110"));
        assert_eq!(cfg.pawn_value, 110);
    }

    #[test]
    fn apply_option_unknown_ignored() {
        let mut cfg = EngineConfig::default();
        assert!(!cfg.apply_option("Nonexistent", "42"));
    }

    #[test]
    fn apply_option_invalid_value_rejected() {
        let mut cfg = EngineConfig::default();
        let old = cfg.pawn_value;
        assert!(!cfg.apply_option("PawnValue", "not_a_number"));
        assert_eq!(cfg.pawn_value, old);
    }

    #[test]
    fn piece_value_from_config() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.piece_value(bughouse_chess::Piece::Pawn), 100);
        assert_eq!(cfg.piece_value(bughouse_chess::Piece::Queen), 900);
        assert_eq!(cfg.piece_value(bughouse_chess::Piece::King), 0);
    }
}
