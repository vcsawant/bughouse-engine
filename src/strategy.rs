//! Play style selection based on time state.
//!
//! Analyzes the four clocks to determine the appropriate play style
//! for the current move. PlayStyle drives time budgets, cross-board
//! aggression, and aggressiveness thresholds.

use bughouse_chess::Color;
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
    /// Premove, < 1s remaining (Emergency)
    Instant,
}

impl PlayStyle {
    /// Cross-board weight multiplier. Higher = more willing to sacrifice
    /// local position for cross-board benefit.
    pub fn style_factor(&self) -> f32 {
        match self {
            PlayStyle::Instant => 0.0,  // Pure selfish play
            PlayStyle::Blitz => 0.5,    // Reduced cross-board influence
            PlayStyle::Standard => 1.0, // Normal
            PlayStyle::Extended => 1.5, // Amplified — willing to sacrifice locally
            PlayStyle::Slow => 0.25,    // Minimal cross-board
        }
    }

    /// Minimum cross-board value (centipawns) before the engine will deviate
    /// from its locally-best move. Prevents tiny bonuses from overriding
    /// strong local play.
    pub fn aggressiveness_threshold(&self) -> i32 {
        match self {
            PlayStyle::Instant => i32::MAX, // Never deviate
            PlayStyle::Blitz => 150,        // Only big cross-board gains
            PlayStyle::Standard => 75,      // Moderate willingness
            PlayStyle::Extended => 30,      // Willing for small gains
            PlayStyle::Slow => 200,         // High threshold
        }
    }
}

/// Select a play style based on clock state.
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
) -> PlayStyle {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];
    let opp_time = clocks[board_idx * 2 + (1 - color_idx)];

    // Emergency: buffer for network latency
    if our_time < 3500 {
        return PlayStyle::Instant;
    }

    // Both team clocks running — extra time pressure
    if both_team_active && our_time < 15000 {
        return PlayStyle::Blitz;
    }

    // Low time: play fast
    if our_time < 10000 {
        return PlayStyle::Blitz;
    }

    // Flat 15s time advantage over direct opponent — search deeper
    if our_time > opp_time + 15000 {
        return PlayStyle::Extended;
    }

    PlayStyle::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_under_3_5s() {
        let clocks = [3000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false), PlayStyle::Instant);
        // Just above threshold → not Instant
        let clocks2 = [4000, 60000, 60000, 60000];
        assert_ne!(determine_play_style(&clocks2, BoardId::A, Color::White, false), PlayStyle::Instant);
    }

    #[test]
    fn blitz_under_10s() {
        let clocks = [5000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false), PlayStyle::Blitz);
    }

    #[test]
    fn extended_with_15s_advantage() {
        // We have 45s, opponent has 28s — 17s advantage → Extended
        let clocks = [45000, 28000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false), PlayStyle::Extended);
        // We have 30s, opponent has 20s — only 10s advantage → Standard
        let clocks2 = [30000, 20000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks2, BoardId::A, Color::White, false), PlayStyle::Standard);
    }

    #[test]
    fn standard_default() {
        let clocks = [60000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false), PlayStyle::Standard);
    }

    #[test]
    fn correct_clock_for_black_board_b() {
        // black_B = index 3, has 2000ms → Instant (under 3.5s)
        let clocks = [60000, 60000, 60000, 2000];
        assert_eq!(determine_play_style(&clocks, BoardId::B, Color::Black, false), PlayStyle::Instant);
    }

    #[test]
    fn both_team_active_forces_blitz() {
        // 12s on clock, normally Standard, but both_team_active → Blitz
        let clocks = [12000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, true), PlayStyle::Blitz);
        // Without both_team_active, same clock → Standard
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White, false), PlayStyle::Standard);
    }

    #[test]
    fn style_factor_values() {
        assert_eq!(PlayStyle::Instant.style_factor(), 0.0);
        assert_eq!(PlayStyle::Blitz.style_factor(), 0.5);
        assert_eq!(PlayStyle::Standard.style_factor(), 1.0);
        assert_eq!(PlayStyle::Extended.style_factor(), 1.5);
        assert_eq!(PlayStyle::Slow.style_factor(), 0.25);
    }

    #[test]
    fn aggressiveness_threshold_values() {
        assert_eq!(PlayStyle::Instant.aggressiveness_threshold(), i32::MAX);
        assert_eq!(PlayStyle::Blitz.aggressiveness_threshold(), 150);
        assert_eq!(PlayStyle::Standard.aggressiveness_threshold(), 75);
        assert_eq!(PlayStyle::Extended.aggressiveness_threshold(), 30);
        assert_eq!(PlayStyle::Slow.aggressiveness_threshold(), 200);
    }

    #[test]
    fn style_factor_ordering() {
        // Extended > Standard > Blitz > Slow > Instant
        assert!(PlayStyle::Extended.style_factor() > PlayStyle::Standard.style_factor());
        assert!(PlayStyle::Standard.style_factor() > PlayStyle::Blitz.style_factor());
        assert!(PlayStyle::Blitz.style_factor() > PlayStyle::Slow.style_factor());
        assert!(PlayStyle::Slow.style_factor() > PlayStyle::Instant.style_factor());
    }
}
