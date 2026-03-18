//! Time budget allocation for search.
//!
//! Given the four bughouse clocks, the board/side we need to move on,
//! the current PlayStyle, and engine config, determines how many
//! milliseconds the engine should spend searching.

use bughouse_chess::Color;
use crate::config::EngineConfig;
use crate::strategy::PlayStyle;
use crate::ubi::BoardId;

/// Allocate a time budget (in ms) for the current move.
///
/// The budget depends on the PlayStyle and config parameters.
/// All styles are clamped to [100ms, 25% of remaining time] as a safety net.
///
/// Clock index mapping: `board_index * 2 + color_index`
/// - white_A=0, black_A=1, white_B=2, black_B=3
pub fn allocate_time(clocks: &[u64; 4], board_id: BoardId, side: Color, style: PlayStyle, config: &EngineConfig) -> u64 {
    let board_idx = match board_id {
        BoardId::A => 0,
        BoardId::B => 1,
    };
    let color_idx = side.to_index();
    let our_time = clocks[board_idx * 2 + color_idx];

    // Emergency: very low time (regardless of style)
    if our_time < config.instant_time_ms {
        return config.instant_budget_ms;
    }

    let base = match style {
        PlayStyle::Instant => return config.instant_budget_ms,
        PlayStyle::Slow => return config.slow_budget_ms,
        PlayStyle::Blitz => {
            let b = our_time / config.blitz_divisor;
            b.min(config.blitz_cap_ms)
        }
        PlayStyle::Standard => {
            let b = our_time / config.standard_divisor;
            b.min(config.standard_cap_ms)
        }
        PlayStyle::Extended => {
            let b = our_time / config.extended_divisor;
            b.min(config.extended_cap_ms)
        }
    };

    // Safety net: minimum 100ms, maximum 25% of remaining time
    base.clamp(100, our_time / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EngineConfig { EngineConfig::default() }

    #[test]
    fn reasonable_budget_with_plenty_of_time() {
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert!(budget >= 100 && budget <= 15000, "budget should be reasonable, got {}", budget);
    }

    #[test]
    fn emergency_low_time() {
        let clocks = [3000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert_eq!(budget, 50);
    }

    #[test]
    fn never_exceeds_quarter_of_remaining() {
        let clocks = [8000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert!(budget <= 2000, "should not exceed 25% of remaining time, got {}", budget);
    }

    #[test]
    fn minimum_budget_enforced() {
        let clocks = [5000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert!(budget >= 100, "minimum budget should be 100ms, got {}", budget);
    }

    #[test]
    fn correct_clock_indexing() {
        let clocks = [500, 500, 500, 60000];
        let budget = allocate_time(&clocks, BoardId::B, Color::Black, PlayStyle::Standard, &cfg());
        assert!(budget > 50, "should use black_B clock (60s), got {}", budget);
        let budget_emergency = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert_eq!(budget_emergency, 50);
    }

    #[test]
    fn board_b_white_clock() {
        let clocks = [500, 500, 30000, 500];
        let budget = allocate_time(&clocks, BoardId::B, Color::White, PlayStyle::Standard, &cfg());
        assert!(budget >= 100, "should use white_B clock (30s), got {}", budget);
    }

    #[test]
    fn blitz_budget_smaller_than_standard() {
        let clocks = [60000, 60000, 60000, 60000];
        let blitz = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Blitz, &cfg());
        let standard = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        assert!(blitz < standard, "Blitz ({}) should be less than Standard ({})", blitz, standard);
    }

    #[test]
    fn extended_budget_larger_than_standard() {
        let clocks = [60000, 60000, 60000, 60000];
        let standard = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg());
        let extended = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Extended, &cfg());
        assert!(extended > standard, "Extended ({}) should be more than Standard ({})", extended, standard);
    }

    #[test]
    fn instant_always_50ms() {
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Instant, &cfg());
        assert_eq!(budget, 50);
    }

    #[test]
    fn slow_always_200ms() {
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Slow, &cfg());
        assert_eq!(budget, 200);
    }

    #[test]
    fn blitz_capped_at_500ms() {
        let clocks = [120000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Blitz, &cfg());
        assert!(budget <= 500, "Blitz should be capped at 500ms, got {}", budget);
    }

    #[test]
    fn custom_config_divisor() {
        let mut cfg = EngineConfig::default();
        cfg.standard_divisor = 15; // Much more generous
        let clocks = [60000, 60000, 60000, 60000];
        let budget = allocate_time(&clocks, BoardId::A, Color::White, PlayStyle::Standard, &cfg);
        // 60000/15 = 4000, capped at standard_cap=2000
        assert_eq!(budget, 2000);
    }
}
