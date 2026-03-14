//! Time budget allocation for search.
//!
//! Given the four bughouse clocks and the board/side we need to move on,
//! determines how many milliseconds the engine should spend searching.

use bughouse_chess::Color;
use crate::ubi::BoardId;

/// Allocate a time budget (in ms) for the current move.
///
/// Uses a simple heuristic: divide remaining time by estimated remaining moves,
/// clamped to reasonable bounds.
///
/// Clock index mapping: `board_index * 2 + color_index`
/// - white_A=0, black_A=1, white_B=2, black_B=3
pub fn allocate_time(clocks: &[u64; 4], board_id: BoardId, side: Color) -> u64 {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];

    // Emergency: very low time
    if our_time < 1000 {
        return 50;
    }

    // Estimate ~30 remaining moves (reasonable midgame assumption)
    let estimated_remaining = 30u64;
    let base = our_time / estimated_remaining;

    // Clamp: minimum 100ms, maximum 25% of remaining time
    base.clamp(100, our_time / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasonable_budget_with_plenty_of_time() {
        // 60 seconds on our clock → ~2000ms budget (60000/30)
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White);
        assert!(budget >= 100 && budget <= 15000,
            "budget should be reasonable, got {}", budget);
    }

    #[test]
    fn emergency_low_time() {
        // Less than 1 second → 50ms
        let clocks = [500, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White);
        assert_eq!(budget, 50);
    }

    #[test]
    fn never_exceeds_quarter_of_remaining() {
        // 4 seconds → base would be 4000/30 ≈ 133ms, max is 1000ms
        let clocks = [4000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White);
        assert!(budget <= 1000, "should not exceed 25% of remaining time, got {}", budget);
    }

    #[test]
    fn minimum_budget_enforced() {
        // 3 seconds → base would be 3000/30 = 100ms, should be >= 100
        let clocks = [3000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White);
        assert!(budget >= 100, "minimum budget should be 100ms, got {}", budget);
    }

    #[test]
    fn correct_clock_indexing() {
        // Only black_B has time, others are emergency
        let clocks = [500, 500, 500, 60000];
        // Board B, Black → index 3
        let budget = allocate_time(&clocks, BoardId::B, Color::Black);
        assert!(budget > 50, "should use black_B clock (60s), got {}", budget);
        // Board A, White → index 0, emergency
        let budget_emergency = allocate_time(&clocks, BoardId::A, Color::White);
        assert_eq!(budget_emergency, 50);
    }

    #[test]
    fn board_b_white_clock() {
        // white_B = index 2
        let clocks = [500, 500, 30000, 500];
        let budget = allocate_time(&clocks, BoardId::B, Color::White);
        assert!(budget >= 100, "should use white_B clock (30s), got {}", budget);
    }
}
