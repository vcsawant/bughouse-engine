//! Play style selection based on time state.
//!
//! Analyzes the four clocks to determine the appropriate play style
//! for the current move.

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

/// Select a play style based on clock state.
///
/// Clock index mapping: `board_index * 2 + color_index`
/// - white_A=0, black_A=1, white_B=2, black_B=3
pub fn determine_play_style(clocks: &[u64; 4], board_id: BoardId, side: Color) -> PlayStyle {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];
    let opp_time = clocks[board_idx * 2 + (1 - color_idx)];

    // Emergency: almost no time
    if our_time < 1000 {
        return PlayStyle::Instant;
    }

    // Low time: play fast
    if our_time < 10000 {
        return PlayStyle::Blitz;
    }

    // Significant time advantage over direct opponent
    if our_time > 30000 && our_time > opp_time * 2 {
        return PlayStyle::Extended;
    }

    PlayStyle::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_under_1s() {
        let clocks = [500, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White), PlayStyle::Instant);
    }

    #[test]
    fn blitz_under_10s() {
        let clocks = [5000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White), PlayStyle::Blitz);
    }

    #[test]
    fn extended_with_big_time_advantage() {
        // We have 60s, opponent has 10s
        let clocks = [60000, 10000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White), PlayStyle::Extended);
    }

    #[test]
    fn standard_default() {
        let clocks = [60000, 60000, 60000, 60000];
        assert_eq!(determine_play_style(&clocks, BoardId::A, Color::White), PlayStyle::Standard);
    }

    #[test]
    fn correct_clock_for_black_board_b() {
        // black_B = index 3, has 500ms → Instant
        let clocks = [60000, 60000, 60000, 500];
        assert_eq!(determine_play_style(&clocks, BoardId::B, Color::Black), PlayStyle::Instant);
    }
}
