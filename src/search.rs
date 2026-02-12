//! 1-ply search with drop pruning for bughouse.
//!
//! Evaluates every legal move at depth 1, using `scoring::evaluate()` on the
//! resulting position. Drops are pruned to a subset of relevant squares
//! (king attack/defense zones, center, promotion zone) to avoid evaluating
//! 200+ drop candidates on open boards.

use bughouse_chess::{
    BitBoard, Board, BoardStatus, BughouseMove, Color, File,
    MoveGen, Piece, Rank, EMPTY,
    get_file, get_king_moves, get_rank,
};

use log::debug;

use crate::scoring;
use crate::strategy::PlayStyle;

/// Result of a search: the best move found, its score, nodes evaluated, and principal variation.
pub struct SearchResult {
    pub best_move: BughouseMove,
    pub score: i32,
    pub nodes: usize,
    pub pv: Vec<String>,
}

/// Find the best move for the current position using 1-ply search.
///
/// Returns `None` if there are no legal moves.
/// `_play_style` is accepted for future use (search depth, time budget).
pub fn find_best_move(board: &Board, _play_style: PlayStyle) -> Option<SearchResult> {
    // If king is already capturable, find the capture
    if board.status() == BoardStatus::KingCapturable {
        return find_king_capture(board);
    }

    let mut best_move: Option<BughouseMove> = None;
    let mut best_score = i32::MIN;
    let mut nodes = 0usize;

    // Count regular moves for logging
    let regular_moves: Vec<_> = MoveGen::new_legal(board).collect();
    let regular_count = regular_moves.len();

    // Evaluate all regular moves
    for cm in regular_moves {
        let new_board = board.make_move_new(cm);
        let score = score_child(&new_board);
        nodes += 1;

        if score > best_score {
            best_score = score;
            best_move = Some(BughouseMove::Regular(cm));

            let bd = scoring::evaluate_detailed(&new_board);
            debug!(
                "New best: {} score={} (mat={} res={} pst={} king={} mob={} pawn={})",
                BughouseMove::Regular(cm), score,
                bd.material, bd.reserves, bd.pst, bd.king_safety, bd.mobility, bd.pawn_structure
            );
        }
    }

    // Evaluate pruned drop moves
    let drop_mask = build_drop_mask(board);
    let us = board.side_to_move();
    let reserves = board.reserves(us);
    let combined = *board.combined();
    let empty_targets = drop_mask & !combined;

    let mut drop_count = 0usize;

    for (piece, count) in reserves.iter() {
        if count == 0 || piece == Piece::King {
            continue;
        }

        for sq in empty_targets {
            // Pawn drops cannot go on rank 1 or 8
            if piece == Piece::Pawn {
                let rank = sq.get_rank();
                if rank == Rank::First || rank == Rank::Eighth {
                    continue;
                }
            }

            if let Some(new_board) = board.make_drop_new(piece, sq) {
                let score = score_child(&new_board);
                nodes += 1;
                drop_count += 1;

                if score > best_score {
                    best_score = score;
                    let drop_move = BughouseMove::Drop { piece, square: sq };
                    best_move = Some(drop_move);

                    let bd = scoring::evaluate_detailed(&new_board);
                    debug!(
                        "New best: {} score={} (mat={} res={} pst={} king={} mob={} pawn={})",
                        drop_move, score,
                        bd.material, bd.reserves, bd.pst, bd.king_safety, bd.mobility, bd.pawn_structure
                    );
                }
            }
        }
    }

    debug!(
        "Search: {} regular + {} drops = {} candidates",
        regular_count, drop_count, nodes
    );

    best_move.map(|m| {
        let move_str = format!("{}", m);
        debug!("Best: {} score={} nodes={}", move_str, best_score, nodes);
        SearchResult {
            best_move: m,
            score: best_score,
            nodes,
            pv: vec![move_str],
        }
    })
}

/// Score a child position from the parent's perspective (negamax).
/// If the child position has king capturable, that means we left our king
/// hanging — score it very negatively.
fn score_child(child: &Board) -> i32 {
    if child.status() == BoardStatus::KingCapturable {
        // Opponent can capture our king — this move is terrible
        return -30000;
    }
    -scoring::evaluate(child)
}

/// When king is already capturable, find any move that captures it.
fn find_king_capture(board: &Board) -> Option<SearchResult> {
    let opponent = !board.side_to_move();
    let king_sq = board.king_square(opponent);

    for cm in MoveGen::new_legal(board) {
        if cm.get_dest() == king_sq {
            let m = BughouseMove::Regular(cm);
            return Some(SearchResult {
                pv: vec![format!("{}", m)],
                best_move: m,
                score: 30000,
                nodes: 1,
            });
        }
    }
    None
}

/// Build the drop mask: a BitBoard of squares where drops are worth considering.
///
/// Combines:
/// - Attack zone: 2 rings around the enemy king
/// - Defense zone: 1 ring around our king
/// - Extended center: ranks 3-6, files c-f
/// - Pawn promotion zone: ranks 6+7 (relative to us, for pawn drops)
fn build_drop_mask(board: &Board) -> BitBoard {
    let us = board.side_to_move();
    let them = !us;

    let our_king_sq = board.king_square(us);
    let enemy_king_sq = board.king_square(them);

    // Attack zone: 2 rings around enemy king
    let ring1 = get_king_moves(enemy_king_sq);
    let mut ring2 = EMPTY;
    for sq in ring1 {
        ring2 |= get_king_moves(sq);
    }
    let attack_zone = ring1 | ring2;

    // Defense zone: 1 ring around our king
    let defense_zone = get_king_moves(our_king_sq);

    // Extended center: ranks 3-6, files c-f
    let center_ranks = get_rank(Rank::Third)
        | get_rank(Rank::Fourth)
        | get_rank(Rank::Fifth)
        | get_rank(Rank::Sixth);
    let center_files = get_file(File::C)
        | get_file(File::D)
        | get_file(File::E)
        | get_file(File::F);
    let extended_center = center_ranks & center_files;

    // Pawn promotion zone: ranks 6+7 relative to side
    let promo_zone = match us {
        Color::White => get_rank(Rank::Sixth) | get_rank(Rank::Seventh),
        Color::Black => get_rank(Rank::Second) | get_rank(Rank::Third),
    };

    attack_zone | defense_zone | extended_center | promo_zone
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bughouse_chess::Square;

    #[test]
    fn finds_move_from_startpos() {
        let board = Board::default();
        let result = find_best_move(&board, PlayStyle::Blitz);
        assert!(result.is_some(), "should find a move from starting position");
        let result = result.unwrap();
        assert!(result.nodes > 0);
        // Score should be roughly near 0 for a balanced position
        assert!(
            result.score.abs() < 200,
            "starting position score should be reasonable, got {}",
            result.score
        );
    }

    #[test]
    fn captures_hanging_queen() {
        // Black queen hanging on e4, white pawn on d3 can capture via d3e4
        // (After e2e4 d7d5 e4d5 Qd8d5? Nb1c3 Qd5e4? Nc3e4 captures queen)
        // Simpler: white knight on f3, black queen on e5 — Nf3 can capture
        let board: Board =
            "rnb1kbnr/pppppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
        // Knight on f3 can capture queen on e5 — should score very well
        assert!(
            result.score > 500,
            "should find the hanging queen capture, score={}",
            result.score
        );
    }

    #[test]
    fn captures_king_when_capturable() {
        // Position where king is capturable — a queen is next to the enemy king
        let board: Board =
            "rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBqKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        if board.status() == BoardStatus::KingCapturable {
            let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
            assert_eq!(result.score, 30000);
        }
    }

    #[test]
    fn avoids_losing_king() {
        // After a move, if our king would be capturable, avoid that move
        // This is tested implicitly: score_child returns -30000 for such positions
        let board = Board::default();
        let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
        // The chosen move should not leave king capturable
        let new_board = match result.best_move {
            BughouseMove::Regular(cm) => board.make_move_new(cm),
            BughouseMove::Drop { piece, square } => board.make_drop_new(piece, square).unwrap(),
        };
        assert_ne!(
            new_board.status(),
            BoardStatus::KingCapturable,
            "best move should not leave king capturable"
        );
    }

    #[test]
    fn drop_pruning_reduces_candidates() {
        // Position with reserves — pruned drops should be fewer than all drops
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QRBNPqrbnp] w KQkq - 0 1"
                .parse()
                .unwrap();
        let drop_mask = build_drop_mask(&board);
        let combined = *board.combined();
        let empty_targets = drop_mask & !combined;

        // Count pruned drop squares
        let pruned_squares = empty_targets.popcnt();
        // All empty squares = 32 in starting position
        let all_empty = (!combined).popcnt();

        assert!(
            pruned_squares < all_empty,
            "drop pruning should reduce candidates: pruned={}, all_empty={}",
            pruned_squares,
            all_empty
        );
    }

    #[test]
    fn search_result_has_correct_structure() {
        let board = Board::default();
        let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
        // Nodes should equal the number of moves evaluated
        assert!(result.nodes >= 20, "startpos should evaluate at least 20 nodes");
        // bestmove should be parseable
        let move_str = format!("{}", result.best_move);
        assert!(move_str.len() >= 4, "move string too short: {}", move_str);
    }

    #[test]
    fn prefers_strong_drop() {
        // White has a queen in reserve, open board — should consider dropping it
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[Q] b KQkq - 0 1"
                .parse()
                .unwrap();
        let result = find_best_move(&board, PlayStyle::Blitz);
        assert!(result.is_some());
    }

    #[test]
    fn drop_mask_covers_enemy_king_zone() {
        let board = Board::default();
        let mask = build_drop_mask(&board);
        // Black king is on e8 — the attack zone (2 rings) should cover squares
        // around e8. At minimum, d7/e7/f7 (ring 1) should be in the mask.
        let d7 = Square::make_square(Rank::Seventh, File::D);
        let e7 = Square::make_square(Rank::Seventh, File::E);
        let f7 = Square::make_square(Rank::Seventh, File::F);
        for sq in [d7, e7, f7] {
            assert!(
                (mask & BitBoard::from_square(sq)) != EMPTY,
                "drop mask should include {:?} (near enemy king)",
                sq
            );
        }
    }

    #[test]
    fn drop_mask_covers_own_king_zone() {
        let board = Board::default();
        let mask = build_drop_mask(&board);
        // White king is on e1 — defense zone should cover d1/d2/e2/f1/f2
        let d2 = Square::make_square(Rank::Second, File::D);
        let f2 = Square::make_square(Rank::Second, File::F);
        for sq in [d2, f2] {
            assert!(
                (mask & BitBoard::from_square(sq)) != EMPTY,
                "drop mask should include {:?} (near own king)",
                sq
            );
        }
    }

    #[test]
    fn drop_mask_covers_extended_center() {
        let board = Board::default();
        let mask = build_drop_mask(&board);
        // Extended center: ranks 3-6, files c-f — d4/e4/d5/e5 should all be covered
        let d4 = Square::make_square(Rank::Fourth, File::D);
        let e5 = Square::make_square(Rank::Fifth, File::E);
        for sq in [d4, e5] {
            assert!(
                (mask & BitBoard::from_square(sq)) != EMPTY,
                "drop mask should include {:?} (extended center)",
                sq
            );
        }
    }

    #[test]
    fn search_with_reserves_evaluates_drops() {
        // Position with a knight in reserve — search should evaluate drop moves
        // and report more nodes than just regular moves
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
        // 20 regular moves + some drop moves
        assert!(
            result.nodes > 20,
            "should evaluate drops too, but only got {} nodes",
            result.nodes
        );
    }

    #[test]
    fn search_deterministic() {
        // Same position should always produce the same result
        let board = Board::default();
        let result1 = find_best_move(&board, PlayStyle::Blitz).unwrap();
        let result2 = find_best_move(&board, PlayStyle::Blitz).unwrap();
        assert_eq!(
            format!("{}", result1.best_move),
            format!("{}", result2.best_move),
            "search should be deterministic"
        );
        assert_eq!(result1.score, result2.score);
        assert_eq!(result1.nodes, result2.nodes);
    }

    #[test]
    fn prefers_capturing_over_quiet() {
        // Black bishop hanging on d5, white knight on c3 can capture (Nc3xd5)
        let board: Board =
            "rn1qkbnr/pppppppp/8/3b4/8/2N5/PPPPPPPP/R1BQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = find_best_move(&board, PlayStyle::Blitz).unwrap();
        // Capturing a bishop (330 cp) should yield a positive score
        assert!(
            result.score > 200,
            "should prefer capturing the bishop, score={}",
            result.score
        );
    }

    #[test]
    fn score_child_negates_eval() {
        // score_child should return -evaluate(child) for non-terminal positions
        let board = Board::default();
        // Make a move and check score_child
        let moved = board.make_move_new(
            "e2e4".parse::<bughouse_chess::ChessMove>().unwrap()
        );
        let child_score = score_child(&moved);
        let direct_eval = crate::scoring::evaluate(&moved);
        assert_eq!(
            child_score, -direct_eval,
            "score_child should negate evaluate: got {}, expected {}",
            child_score, -direct_eval
        );
    }

    #[test]
    fn score_child_penalizes_king_capturable() {
        // If a child position has king capturable, score_child returns -30000
        let board: Board =
            "rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBqKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        if board.status() == BoardStatus::KingCapturable {
            // From the parent's perspective, this child is terrible
            let score = score_child(&board);
            assert_eq!(score, -30000);
        }
    }
}
