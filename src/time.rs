//! Time budget allocation for search.
//!
//! Given the four bughouse clocks, the board/side we need to move on,
//! and the current PlayStyle, determines how many milliseconds the
//! engine should spend searching.

use bughouse_chess::Color;
use crate::strategy::PlayStyle;
use crate::ubi::BoardId;

/// Allocate a time budget (in ms) for the current move.
///
/// The budget depends on the PlayStyle:
/// - `Instant`: 50ms fixed (emergency)
/// - `Blitz`: time/40, max 500ms (conserve time)
/// - `Standard`: time/30, max 2000ms (normal play)
/// - `Extended`: time/20, max 4000ms (deep search with time advantage)
/// - `Slow`: 200ms fixed (stalling — future use)
///
/// All styles are clamped to [100ms, 25% of remaining time] as a safety net.
///
/// Clock index mapping: `board_index * 2 + color_index`
/// - white_A=0, black_A=1, white_B=2, black_B=3
pub fn allocate_time(clocks: &[u64; 4], board_id: BoardId, side: Color, style: PlayStyle) -> u64 {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];

    // Emergency: very low time (regardless of style)
    if our_time < 3500 {
        return 50;
    }

    let base = match style {
        PlayStyle::Instant => return 50,
        PlayStyle::Slow => return 200,
        PlayStyle::Blitz => {
            let b = our_time / 40;
            b.min(500) // Hard cap for Blitz
        }
        PlayStyle::Standard => {
            let b = our_time / 30;
            b.min(2000) // Hard cap for Standard
        }
        PlayStyle::Extended => {
            let b = our_time / 20;
            b.min(4000) // Hard cap for Extended
        }
    };

    // Safety net: minimum 100ms, maximum 25% of remaining time
    base.clamp(100, our_time / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasonable_budget_with_plenty_of_time() {
        // 60 seconds on our clock, Standard → ~2000ms budget (60000/30)
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert!(budget >= 100 && budget <= 15000,
            "budget should be reasonable, got {}", budget);
    }

    #[test]
    fn emergency_low_time() {
        // Less than 3.5 seconds → 50ms regardless of style
        let clocks = [3000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert_eq!(budget, 50);
    }

    #[test]
    fn never_exceeds_quarter_of_remaining() {
        // 8 seconds → Standard base would be 8000/30 ≈ 266ms, max is 2000ms
        let clocks = [8000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert!(budget <= 2000, "should not exceed 25% of remaining time, got {}", budget);
    }

    #[test]
    fn minimum_budget_enforced() {
        // 5 seconds → Standard base would be 5000/30 ≈ 166ms, should be >= 100
        let clocks = [5000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert!(budget >= 100, "minimum budget should be 100ms, got {}", budget);
    }

    #[test]
    fn correct_clock_indexing() {
        // Only black_B has time, others are emergency
        let clocks = [500, 500, 500, 60000];
        // Board B, Black → index 3
        let budget = allocate_time(&clocks, BoardId::B, Color::Black, PlayStyle::Standard);
        assert!(budget > 50, "should use black_B clock (60s), got {}", budget);
        // Board A, White → index 0, emergency
        let budget_emergency = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert_eq!(budget_emergency, 50);
    }

    #[test]
    fn board_b_white_clock() {
        // white_B = index 2
        let clocks = [500, 500, 30000, 500];
        let budget = allocate_time(&clocks, BoardId::B, Color::White, PlayStyle::Standard);
        assert!(budget >= 100, "should use white_B clock (30s), got {}", budget);
    }

    #[test]
    fn blitz_budget_smaller_than_standard() {
        let clocks = [60000, 60000, 60000, 60000];
        let blitz = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Blitz);
        let standard = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        assert!(blitz < standard,
            "Blitz budget ({}) should be less than Standard ({})", blitz, standard);
    }

    #[test]
    fn extended_budget_larger_than_standard() {
        let clocks = [60000, 60000, 60000, 60000];
        let standard = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard);
        let extended = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Extended);
        assert!(extended > standard,
            "Extended budget ({}) should be more than Standard ({})", extended, standard);
    }

    #[test]
    fn instant_always_50ms() {
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Instant);
        assert_eq!(budget, 50, "Instant should always be 50ms, got {}", budget);
    }

    #[test]
    fn slow_always_200ms() {
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Slow);
        assert_eq!(budget, 200, "Slow should always be 200ms, got {}", budget);
    }

    #[test]
    fn blitz_capped_at_500ms() {
        // 120s clock, Blitz: 120000/40 = 3000 but capped at 500
        let clocks = [120000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Blitz);
        assert!(budget <= 500, "Blitz should be capped at 500ms, got {}", budget);
    }
}
