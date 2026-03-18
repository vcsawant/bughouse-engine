//! Play style selection based on time state.
//!
//! Analyzes the four clocks to determine the appropriate play style
//! for the current move. PlayStyle drives time budgets, cross-board
//! aggression, and aggressiveness thresholds (via EngineConfig).

use bughouse_chess::Color;
use crate::config::EngineConfig;
use crate::ubi::BoardId;

/// Determines search depth, time budget, and move selection personality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayStyle {
    /// Fast, shallow, forcing moves (Disadvantage time state)
    Blitz,
    /// Normal search depth, solid play (MildDisadvantage)
    Standard,
    /// Deep search, find the best move (PotentialAdvantage / LocalAdvantage)
    Extended,
    /// Quiet, non-committal moves (Stall)
    Slow,
    /// Premove, < 3.5s remaining (Emergency)
    Instant,
}

/// Select a play style based on clock state and engine config.
///
/// Clock index mapping: `board_index * 2 + color_index`
/// - white_A=0, black_A=1, white_B=2, black_B=3
///
/// `both_team_active`: true when both boards have active `go` commands,
/// meaning both our team's clocks are draining simultaneously.
pub fn determine_play_style(
    clocks: &[u64; 4],
    board_id: BoardId,
    side: Color,
    both_team_active: bool,
    config: &EngineConfig,
) -> PlayStyle {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];
    let opp_time = clocks[board_idx * 2 + (1 - color_idx)];

    // Emergency: buffer for network latency
    if our_time < config.instant_time_ms {
        return PlayStyle::Instant;
    }

    // Both team clocks running — extra time pressure
    if both_team_active && our_time < config.both_active_blitz_ms {
        return PlayStyle::Blitz;
    }

    // Low time: play fast
    if our_time < config.blitz_time_ms {
        return PlayStyle::Blitz;
    }

    // Flat time advantage over direct opponent — search deeper
    if our_time > opp_time + config.extended_advantage_ms {
        return PlayStyle::Extended;
    }

    PlayStyle::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EngineConfig { EngineConfig::default() }

    #[test]
    fn emergency_under_3_5s() {
        let clocks = [3000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg()), PlayStyle::Instant);
        let clocks2 = [4000, 60000, 60000, 60000];
        assert_ne!(determine_play_style(&clocks2, BoardId::A, Color::White, false, &cfg()), PlayStyle::Instant);
    }

    #[test]
    fn blitz_under_10s() {
        let clocks = [5000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg()), PlayStyle::Blitz);
    }

    #[test]
    fn extended_with_15s_advantage() {
        let clocks = [45000, 28000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg()), PlayStyle::Extended);
        let clocks2 = [30000, 20000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks2, BoardId::A, Color::White, false, &cfg()), PlayStyle::Standard);
    }

    #[test]
    fn standard_default() {
        let clocks = [60000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg()), PlayStyle::Standard);
    }

    #[test]
    fn correct_clock_for_black_board_b() {
        let clocks = [60000, 60000, 60000, 2000];
        assert_eq!(determine_play_style(&clocks, BoardId::B, Color::Black, false, &cfg()), PlayStyle::Instant);
    }

    #[test]
    fn both_team_active_forces_blitz() {
        let clocks = [12000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, true, &cfg()), PlayStyle::Blitz);
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg()), PlayStyle::Standard);
    }

    #[test]
    fn custom_config_thresholds() {
        let mut cfg = EngineConfig::default();
        cfg.instant_time_ms = 5000; // Raise instant threshold
        let clocks = [4000, 60000, 60000, 60000];
        // With default config, 4s → Blitz. With custom config, 4s → Instant.
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false, &cfg), PlayStyle::Instant);
    }

    #[test]
    fn config_style_factor_values() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.style_factor(PlayStyle::Instant), 0.0);
        assert_eq!(cfg.style_factor(PlayStyle::Blitz), 0.5);
        assert_eq!(cfg.style_factor(PlayStyle::Standard), 1.0);
        assert_eq!(cfg.style_factor(PlayStyle::Extended), 1.5);
    }

    #[test]
    fn config_aggression_values() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.aggressiveness_threshold(PlayStyle::Instant), i32::MAX);
        assert_eq!(cfg.aggressiveness_threshold(PlayStyle::Blitz), 150);
        assert_eq!(cfg.aggressiveness_threshold(PlayStyle::Standard), 75);
        assert_eq!(cfg.aggressiveness_threshold(PlayStyle::Extended), 30);
    }
}
