//! Opening book for bughouse chess.
//!
//! Maps known opening positions to pre-selected moves using position hashes
//! (excluding reserves). Lines are encoded as human-readable move sequences
//! and replayed at startup to build a hash table.

use std::collections::HashMap;

use bughouse_chess::{Board, BughouseMove};
use log::{debug, warn};
use rand::Rng;

// ─── Book Data ──────────────────────────────────────────────────────

struct BookLine {
    setup: &'static [&'static str], // moves to reach position
    response: &'static str,         // recommended move from that position
    weight: u8,                     // selection weight (higher = more likely)
}

/// All opening lines. Each entry maps a position (reached by replaying `setup`
/// from startpos) to a recommended `response` move with a `weight`.
const BOOK_LINES: &[BookLine] = &[
    // ═══════════════════════════════════════════════════════════════
    // WHITE OPENINGS (White to move)
    // ═══════════════════════════════════════════════════════════════

    // ── Move 1: Startpos ──
    BookLine { setup: &[], response: "e2e4", weight: 5 },
    BookLine { setup: &[], response: "d2d4", weight: 3 },

    // ── After 1.e4 e5 (White move 2) ──
    BookLine { setup: &["e2e4", "e7e5"], response: "g1h3", weight: 4 }, // Nh3 → Ng5 system
    BookLine { setup: &["e2e4", "e7e5"], response: "f1c4", weight: 3 }, // Italian, f7 pressure
    BookLine { setup: &["e2e4", "e7e5"], response: "g1f3", weight: 3 }, // Classical

    // ── After 1.e4 e6 (fortress — White move 2) ──
    BookLine { setup: &["e2e4", "e7e6"], response: "d2d4", weight: 5 }, // Mongolian Attack
    BookLine { setup: &["e2e4", "e7e6"], response: "g1h3", weight: 3 }, // Nh3 system

    // ── After 1.e4 d6 (White move 2) ──
    BookLine { setup: &["e2e4", "d7d6"], response: "d2d4", weight: 5 }, // Claim center
    BookLine { setup: &["e2e4", "d7d6"], response: "g1h3", weight: 3 },

    // ── After 1.e4 e6 2.d4 (White move 3) ──
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6"], response: "f1c4", weight: 4 }, // Mongolian: Bc4
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6"], response: "g1h3", weight: 3 }, // Nh3
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6"], response: "g1f3", weight: 3 }, // Classical

    // ── After 1.e4 e6 2.d4 d6 3.Bc4 (White move 4) ──
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4", "g8f6"], response: "c1e3", weight: 4 }, // Mongolian: Be3
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4", "g8f6"], response: "g1h3", weight: 3 }, // Nh3
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4", "b8c6"], response: "c1e3", weight: 4 },
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4", "b8c6"], response: "g1h3", weight: 3 },

    // ── After 1.e4 e5 2.Nh3 (White move 3) ──
    BookLine { setup: &["e2e4", "e7e5", "g1h3", "d7d6"], response: "f1c4", weight: 4 }, // Bc4 battery
    BookLine { setup: &["e2e4", "e7e5", "g1h3", "d7d6"], response: "d2d4", weight: 3 },
    BookLine { setup: &["e2e4", "e7e5", "g1h3", "g8f6"], response: "f1c4", weight: 4 },
    BookLine { setup: &["e2e4", "e7e5", "g1h3", "g8f6"], response: "d2d4", weight: 3 },
    BookLine { setup: &["e2e4", "e7e5", "g1h3", "b8c6"], response: "f1c4", weight: 4 },

    // ── After 1.e4 e5 2.Nf3 Nc6 (White move 3) ──
    BookLine { setup: &["e2e4", "e7e5", "g1f3", "b8c6"], response: "f1c4", weight: 5 }, // Italian → f7

    // ── After 1.e4 e5 2.Bc4 (White move 3) ──
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "b8c6"], response: "g1h3", weight: 4 }, // Heading for Ng5
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "b8c6"], response: "g1f3", weight: 3 },
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "g8f6"], response: "g1h3", weight: 4 },
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "g8f6"], response: "g1f3", weight: 3 },

    // ── After 1.d4 (White move 2) ──
    BookLine { setup: &["d2d4", "d7d6"], response: "e2e4", weight: 5 }, // Transpose to center
    BookLine { setup: &["d2d4", "e7e6"], response: "e2e4", weight: 5 },
    BookLine { setup: &["d2d4", "g8f6"], response: "e2e4", weight: 4 },
    BookLine { setup: &["d2d4", "g8f6"], response: "c1g5", weight: 3 }, // Bg5 pin

    // ═══════════════════════════════════════════════════════════════
    // BLACK DEFENSES (Black to move)
    // ═══════════════════════════════════════════════════════════════

    // ── After 1.e4 (Black move 1) ──
    BookLine { setup: &["e2e4"], response: "e7e6", weight: 5 }, // Fortress
    BookLine { setup: &["e2e4"], response: "e7e5", weight: 3 }, // Contest center
    BookLine { setup: &["e2e4"], response: "d7d6", weight: 2 }, // Passive but solid

    // ── After 1.d4 (Black move 1) ──
    BookLine { setup: &["d2d4"], response: "d7d6", weight: 4 }, // Fortress
    BookLine { setup: &["d2d4"], response: "e7e6", weight: 4 },

    // ── After 1.e4 e6 2.d4 (Black move 2) ──
    BookLine { setup: &["e2e4", "e7e6", "d2d4"], response: "d7d6", weight: 5 }, // Complete fortress

    // ── After 1.e4 e5 2.Nf3 (Black move 2) ──
    BookLine { setup: &["e2e4", "e7e5", "g1f3"], response: "b8c6", weight: 5 }, // Develop, defend e5

    // ── After 1.e4 e5 2.Bc4 (Black move 2) ──
    BookLine { setup: &["e2e4", "e7e5", "f1c4"], response: "b8c6", weight: 4 },
    BookLine { setup: &["e2e4", "e7e5", "f1c4"], response: "g8f6", weight: 3 }, // Attack e4

    // ── After 1.e4 e5 2.Nh3 (Black move 2) ──
    BookLine { setup: &["e2e4", "e7e5", "g1h3"], response: "d7d6", weight: 4 }, // Solid
    BookLine { setup: &["e2e4", "e7e5", "g1h3"], response: "g8f6", weight: 3 }, // Develop

    // ── After 1.d4 d6 2.e4 (Black move 2) ──
    BookLine { setup: &["d2d4", "d7d6", "e2e4"], response: "e7e6", weight: 5 }, // Fortress
    BookLine { setup: &["d2d4", "d7d6", "e2e4"], response: "g8f6", weight: 3 },

    // ── After 1.d4 e6 2.e4 (Black move 2) ──
    BookLine { setup: &["d2d4", "e7e6", "e2e4"], response: "d7d6", weight: 5 }, // Fortress

    // ── After 1.e4 e6 2.d4 d6 3.Bc4 (Black move 3) ──
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4"], response: "g8f6", weight: 4 },
    BookLine { setup: &["e2e4", "e7e6", "d2d4", "d7d6", "f1c4"], response: "b8c6", weight: 3 },

    // ── After 1.e4 e6 2.Nh3 (Black move 2) ──
    BookLine { setup: &["e2e4", "e7e6", "g1h3"], response: "d7d6", weight: 5 }, // Fortress

    // ── After 1.e4 d6 2.d4 (Black move 2) ──
    BookLine { setup: &["e2e4", "d7d6", "d2d4"], response: "e7e6", weight: 5 }, // Fortress
    BookLine { setup: &["e2e4", "d7d6", "d2d4"], response: "g8f6", weight: 3 },

    // ── After 1.e4 e5 2.Nf3 Nc6 3.Bc4 (Black move 3) ──
    BookLine { setup: &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4"], response: "f8c5", weight: 4 }, // Italian game
    BookLine { setup: &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4"], response: "g8f6", weight: 3 }, // Two knights

    // ── After 1.e4 e5 2.Bc4 Nc6 3.Nh3 (Black move 3) ──
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "b8c6", "g1h3"], response: "g8f6", weight: 4 },
    BookLine { setup: &["e2e4", "e7e5", "f1c4", "b8c6", "g1h3"], response: "d7d6", weight: 3 },
];

// ─── Opening Book ───────────────────────────────────────────────────

pub struct BookEntry {
    pub mv: BughouseMove,
    pub weight: u8,
}

pub struct OpeningBook {
    entries: HashMap<u64, Vec<BookEntry>>,
}

impl OpeningBook {
    /// Build the opening book by replaying all lines from startpos.
    pub fn new() -> Self {
        let mut entries: HashMap<u64, Vec<BookEntry>> = HashMap::new();
        let startpos = Board::default();

        for (i, line) in BOOK_LINES.iter().enumerate() {
            // Replay setup moves to reach the position
            let mut board = startpos;
            let mut valid = true;

            for (j, move_str) in line.setup.iter().enumerate() {
                let m: BughouseMove = match move_str.parse() {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("Book line {}: invalid setup move '{}': {}", i, move_str, e);
                        valid = false;
                        break;
                    }
                };

                board = match apply_move(&board, &m) {
                    Some(b) => b,
                    None => {
                        warn!("Book line {}: illegal setup move '{}' at step {}", i, move_str, j);
                        valid = false;
                        break;
                    }
                };
            }

            if !valid {
                continue;
            }

            // Parse the response move
            let response: BughouseMove = match line.response.parse() {
                Ok(m) => m,
                Err(e) => {
                    warn!("Book line {}: invalid response '{}': {}", i, line.response, e);
                    continue;
                }
            };

            // Verify the response is legal
            if apply_move(&board, &response).is_none() {
                warn!("Book line {}: illegal response '{}' in position", i, line.response);
                continue;
            }

            let hash = board.get_position_hash();
            entries.entry(hash).or_default().push(BookEntry {
                mv: response,
                weight: line.weight,
            });
        }

        debug!("Opening book loaded: {} positions, {} total entries",
            entries.len(),
            entries.values().map(|v| v.len()).sum::<usize>());

        OpeningBook { entries }
    }

    /// Look up the current position in the book. Returns a weighted-random
    /// move selection, or None if the position is not in the book.
    pub fn lookup(&self, board: &Board, rng: &mut impl Rng) -> Option<BughouseMove> {
        let hash = board.get_position_hash();
        let candidates = self.entries.get(&hash)?;

        if candidates.is_empty() {
            return None;
        }

        // Weighted random selection
        let total_weight: u32 = candidates.iter().map(|e| e.weight as u32).sum();
        if total_weight == 0 {
            return None;
        }

        let mut roll = rng.gen_range(0..total_weight);
        for entry in candidates {
            if roll < entry.weight as u32 {
                return Some(entry.mv.clone());
            }
            roll -= entry.weight as u32;
        }

        // Shouldn't reach here, but return last entry as fallback
        candidates.last().map(|e| e.mv.clone())
    }

    /// Number of unique positions in the book.
    #[cfg(test)]
    fn position_count(&self) -> usize {
        self.entries.len()
    }

    /// Total number of entries (moves) across all positions.
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }
}

/// Apply a move to a board, returning the new board or None if illegal.
fn apply_move(board: &Board, m: &BughouseMove) -> Option<Board> {
    match m {
        BughouseMove::Regular(cm) => {
            let cm = *cm;
            let board = *board;
            match std::panic::catch_unwind(move || board.make_move_new(cm)) {
                Ok(b) => Some(b),
                Err(_) => None,
            }
        }
        BughouseMove::Drop { piece, square } => board.make_drop_new(*piece, *square),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bughouse_chess::MoveGen;

    #[test]
    fn book_loads_successfully() {
        let book = OpeningBook::new();
        assert!(book.position_count() > 0, "book should have entries");
        assert!(book.entry_count() > 0, "book should have moves");
        // We defined ~50+ lines, they map to 20+ unique positions
        assert!(book.position_count() >= 15,
            "expected at least 15 positions, got {}", book.position_count());
    }

    #[test]
    fn book_contains_startpos() {
        let book = OpeningBook::new();
        let board = Board::default();
        let mut rng = rand::thread_rng();
        let mv = book.lookup(&board, &mut rng);
        assert!(mv.is_some(), "startpos should be in the book");

        // Should be e2e4 or d2d4
        let mv = mv.unwrap();
        let mv_str = format!("{}", mv);
        assert!(mv_str == "e2e4" || mv_str == "d2d4",
            "startpos book move should be e2e4 or d2d4, got {}", mv_str);
    }

    #[test]
    fn book_lookup_returns_legal_move() {
        let book = OpeningBook::new();
        let board = Board::default();
        let mut rng = rand::thread_rng();

        // Check multiple lookups
        for _ in 0..20 {
            if let Some(mv) = book.lookup(&board, &mut rng) {
                // Verify move is legal
                let legal_moves: Vec<_> = MoveGen::new_legal(&board)
                    .map(BughouseMove::Regular)
                    .collect();
                assert!(legal_moves.contains(&mv),
                    "book move {} should be legal", mv);
            }
        }
    }

    #[test]
    fn book_miss_returns_none() {
        let book = OpeningBook::new();
        // A random midgame position shouldn't be in the book
        let board: Board = "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[] b KQkq - 3 3"
            .parse()
            .unwrap();
        let mut rng = rand::thread_rng();
        // This specific position might happen to be in the book, but positions
        // far from the opening shouldn't be
        let board2: Board = "rnbqk2r/pppp1ppp/5n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQ1RK1[] b kq - 5 4"
            .parse()
            .unwrap();
        let result = book.lookup(&board2, &mut rng);
        assert!(result.is_none(),
            "castled position should not be in the book");
    }

    #[test]
    fn book_weighted_selection() {
        let book = OpeningBook::new();
        let board = Board::default();
        let mut rng = rand::thread_rng();

        // At startpos, e2e4 has weight 5, d2d4 has weight 3.
        // Over many samples, e2e4 should appear more often.
        let mut e4_count = 0;
        let mut d4_count = 0;
        let trials = 1000;

        for _ in 0..trials {
            if let Some(mv) = book.lookup(&board, &mut rng) {
                let s = format!("{}", mv);
                if s == "e2e4" { e4_count += 1; }
                if s == "d2d4" { d4_count += 1; }
            }
        }

        assert!(e4_count > d4_count,
            "e2e4 (weight 5) should appear more than d2d4 (weight 3): e4={}, d4={}",
            e4_count, d4_count);
        assert!(e4_count + d4_count == trials,
            "all picks should be e2e4 or d2d4: e4={}, d4={}, total={}",
            e4_count, d4_count, trials);
    }

    #[test]
    fn book_after_e4_e5() {
        let book = OpeningBook::new();
        let mut board = Board::default();

        // Play 1.e4 e5
        let e4: BughouseMove = "e2e4".parse().unwrap();
        board = apply_move(&board, &e4).unwrap();
        let e5: BughouseMove = "e7e5".parse().unwrap();
        board = apply_move(&board, &e5).unwrap();

        let mut rng = rand::thread_rng();
        let mv = book.lookup(&board, &mut rng);
        assert!(mv.is_some(), "position after 1.e4 e5 should be in book");

        let mv_str = format!("{}", mv.unwrap());
        assert!(mv_str == "g1h3" || mv_str == "f1c4" || mv_str == "g1f3",
            "after 1.e4 e5, book should suggest Nh3/Bc4/Nf3, got {}", mv_str);
    }

    #[test]
    fn book_fortress_defense() {
        let book = OpeningBook::new();
        let mut board = Board::default();

        // Play 1.e4
        let e4: BughouseMove = "e2e4".parse().unwrap();
        board = apply_move(&board, &e4).unwrap();

        let mut rng = rand::thread_rng();
        let mv = book.lookup(&board, &mut rng);
        assert!(mv.is_some(), "position after 1.e4 should be in book for Black");

        // Should include fortress defense (e7e6)
        let mut found_e6 = false;
        for _ in 0..100 {
            if let Some(mv) = book.lookup(&board, &mut rng) {
                if format!("{}", mv) == "e7e6" {
                    found_e6 = true;
                    break;
                }
            }
        }
        assert!(found_e6, "Black should sometimes play e7e6 (fortress) after 1.e4");
    }
}
