//! Play style scaffolding for time-aware strategy selection.
//!
//! Phase C always uses `Blitz`. Future phases will select style based on
//! clock state and the `TimeState` framework described in README.

/// Determines search depth, time budget, and move selection personality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Future phases will use all variants
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
/// For Phase C, always returns `Blitz`. Future phases will analyze the four
/// clocks to determine the appropriate `TimeState` and map it to a style.
pub fn determine_play_style(_clocks: &[u64; 4]) -> PlayStyle {
    PlayStyle::Blitz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_blitz_in_phase_c() {
        assert_eq!(determine_play_style(&[180000, 180000, 180000, 180000]), PlayStyle::Blitz);
        assert_eq!(determine_play_style(&[0, 0, 0, 0]), PlayStyle::Blitz);
        assert_eq!(determine_play_style(&[1000, 180000, 5000, 60000]), PlayStyle::Blitz);
    }
}
