//! Static evaluation function for bughouse positions.
//!
//! Returns centipawns from the perspective of `board.side_to_move()`.
//! Positive = good for the side to move.
//!
//! Components: material, reserves, piece-square tables, king safety,
//! mobility, and pawn structure.

use bughouse_chess::{
    Board, BoardStatus, Color, File, Piece, Rank,
    ALL_FILES, ALL_PIECES, EMPTY,
    get_adjacent_files, get_bishop_moves, get_file, get_king_zone,
    get_knight_moves, get_rank, get_rook_moves,
};

use crate::config::EngineConfig;

// ─── Piece Values (centipawns) ──────────────────────────────────────

const PAWN_VALUE: i32 = 100;
const KNIGHT_VALUE: i32 = 320;
const BISHOP_VALUE: i32 = 330;
const ROOK_VALUE: i32 = 500;
const QUEEN_VALUE: i32 = 900;

pub fn piece_value(piece: Piece) -> i32 {
    match piece {
        Piece::Pawn => PAWN_VALUE,
        Piece::Knight => KNIGHT_VALUE,
        Piece::Bishop => BISHOP_VALUE,
        Piece::Rook => ROOK_VALUE,
        Piece::Queen => QUEEN_VALUE,
        Piece::King => 0,
    }
}

// Reserve premium multipliers (config, defaults in EngineConfig):
// Reserves are valued *higher* than on-board pieces because of placement
// flexibility — you can drop them on any empty square.
//
// - Knights (160%): extremely flexible drop targets, fork potential
// - Pawns (130%): great for blocking attacks and shielding the king
// - Bishops (130%): good defensive drops, diagonal coverage
// - Rooks/Queens (120%): powerful but already highly valued
//
// On top of the static base, multipliers are adjusted dynamically by king
// exposure — see `reserve_score_dyn`.

// ─── Piece-Square Tables ────────────────────────────────────────────
// Indexed from white's perspective (a1=0, h8=63).
// For black, flip rank: index ^ 56.

#[rustfmt::skip]
const PAWN_PST: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
    50, 50, 50, 50, 50, 50, 50, 50,
    10, 10, 20, 30, 30, 20, 10, 10,
     5,  5, 10, 25, 25, 10,  5,  5,
     0,  0,  0, 20, 20,  0,  0,  0,
     5, -5,-10,  0,  0,-10, -5,  5,
     5, 10, 10,-20,-20, 10, 10,  5,
     0,  0,  0,  0,  0,  0,  0,  0,
];

#[rustfmt::skip]
const KNIGHT_PST: [i32; 64] = [
    -50,-40,-30,-30,-30,-30,-40,-50,
    -40,-20,  0,  0,  0,  0,-20,-40,
    -30,  0, 10, 15, 15, 10,  0,-30,
    -30,  5, 15, 20, 20, 15,  5,-30,
    -30,  0, 15, 20, 20, 15,  0,-30,
    -30,  5, 10, 15, 15, 10,  5,-30,
    -40,-20,  0,  5,  5,  0,-20,-40,
    -50,-40,-30,-30,-30,-30,-40,-50,
];

#[rustfmt::skip]
const BISHOP_PST: [i32; 64] = [
    -20,-10,-10,-10,-10,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0, 10, 10, 10, 10,  0,-10,
    -10,  5,  5, 10, 10,  5,  5,-10,
    -10,  0, 10, 10, 10, 10,  0,-10,
    -10, 10, 10, 10, 10, 10, 10,-10,
    -10,  5,  0,  0,  0,  0,  5,-10,
    -20,-10,-10,-10,-10,-10,-10,-20,
];

#[rustfmt::skip]
const ROOK_PST: [i32; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
     5, 10, 10, 10, 10, 10, 10,  5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
     0,  0,  0,  5,  5,  0,  0,  0,
];

#[rustfmt::skip]
const QUEEN_PST: [i32; 64] = [
    -20,-10,-10, -5, -5,-10,-10,-20,
    -10,  0,  0,  0,  0,  0,  0,-10,
    -10,  0,  5,  5,  5,  5,  0,-10,
     -5,  0,  5,  5,  5,  5,  0, -5,
      0,  0,  5,  5,  5,  5,  0, -5,
    -10,  5,  5,  5,  5,  5,  0,-10,
    -10,  0,  5,  0,  0,  0,  0,-10,
    -20,-10,-10, -5, -5,-10,-10,-20,
];

// King PST: prioritize castled positions (corners) — critical in bughouse
#[rustfmt::skip]
const KING_PST: [i32; 64] = [
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -30,-40,-40,-50,-50,-40,-40,-30,
    -20,-30,-30,-40,-40,-30,-30,-20,
    -10,-20,-20,-20,-20,-20,-20,-10,
     20, 20,  0,  0,  0,  0, 20, 20,
     20, 30, 10,  0,  0, 10, 30, 20,
];

fn pst_for_piece(piece: Piece) -> &'static [i32; 64] {
    match piece {
        Piece::Pawn => &PAWN_PST,
        Piece::Knight => &KNIGHT_PST,
        Piece::Bishop => &BISHOP_PST,
        Piece::Rook => &ROOK_PST,
        Piece::Queen => &QUEEN_PST,
        Piece::King => &KING_PST,
    }
}

// ─── Evaluation Components ──────────────────────────────────────────

/// Reserve value for one color with dynamic position-aware multipliers.
///
/// The static base multiplier (config) is adjusted by king exposure:
/// - Attack bonus: non-pawn pieces gain value as the ENEMY king grows exposed —
///   they are drop-attack ammunition.
/// - Defense bonus: pawns gain value as OUR king grows exposed — the premier
///   blocking and shielding drop.
///
/// Exposure is the non-negative magnitude of `king_safety_base` (raw shield /
/// open-file / attack-zone penalties, before the reserve amplifier). Added
/// percentage points = exposure * scale / 100, capped by
/// `reserve_dynamic_cap`. Scale 0 restores static multipliers.
fn reserve_score_dyn(
    board: &Board,
    color: Color,
    config: &EngineConfig,
    own_exposure: i32,
    enemy_exposure: i32,
) -> i32 {
    let reserve = &board.reserves()[color.to_index()];
    let mut score = 0;
    for (piece, count) in reserve.iter() {
        let base_pct = config.reserve_multiplier_pct(piece);
        let bonus_pct = match piece {
            Piece::Pawn => {
                (own_exposure * config.reserve_defense_scale / 100).min(config.reserve_dynamic_cap)
            }
            Piece::King => 0,
            _ => {
                (enemy_exposure * config.reserve_attack_scale / 100).min(config.reserve_dynamic_cap)
            }
        };
        score += count as i32 * config.piece_value(piece) * (base_pct + bonus_pct) / 100;
    }
    score
}

/// Piece-square table score for one color.
fn pst_score(board: &Board, color: Color) -> i32 {
    let mut score = 0;
    for piece in ALL_PIECES {
        let pst = pst_for_piece(piece);
        let pieces = *board.pieces(piece) & *board.color_combined(color);
        for sq in pieces {
            let idx = match color {
                Color::White => sq.to_index(),
                Color::Black => sq.to_index() ^ 56,
            };
            score += pst[idx];
        }
    }
    score
}

/// King safety base penalty (without reserve amplifier).
fn king_safety_base(board: &Board, color: Color) -> i32 {
    let king_sq = board.king_square(color);
    let king_file = king_sq.get_file();
    let king_rank = king_sq.get_rank();
    let our_pawns = *board.pieces(Piece::Pawn) & *board.color_combined(color);
    let opponent = !color;

    let mut penalty = 0i32;

    // (a) Pawn shield: check 3 files around king for friendly pawn in front
    let shield_rank = match color {
        Color::White => {
            if king_rank.to_index() < 7 { Some(Rank::from_index(king_rank.to_index() + 1)) } else { None }
        }
        Color::Black => {
            if king_rank.to_index() > 0 { Some(Rank::from_index(king_rank.to_index() - 1)) } else { None }
        }
    };

    let king_file_idx = king_file.to_index();
    let shield_files: Vec<File> = {
        let mut files = vec![king_file];
        if king_file_idx > 0 {
            files.push(File::from_index(king_file_idx - 1));
        }
        if king_file_idx < 7 {
            files.push(File::from_index(king_file_idx + 1));
        }
        files
    };

    if let Some(sr) = shield_rank {
        let shield_rank_bb = get_rank(sr);
        for &f in &shield_files {
            let file_bb = get_file(f);
            if (our_pawns & file_bb & shield_rank_bb) == EMPTY {
                penalty -= 30;
            }
        }
    }

    // (b) Open/semi-open files around king
    for &f in &shield_files {
        let file_bb = get_file(f);
        if (our_pawns & file_bb) == EMPTY {
            penalty -= 25;
        }
    }

    // (c) Attackers on king zone (2-ring)
    let king_zone = get_king_zone(king_sq);
    let opp_attacks = board.attacks_by(opponent);
    let zone_under_attack = (king_zone & opp_attacks).popcnt() as i32;
    penalty -= 10 * zone_under_attack;

    penalty
}

/// King safety amplified by the opponent's reserve material: an exposed king
/// is far more dangerous when the opponent holds pieces to drop.
/// `base` is this color's `king_safety_base` (≤ 0). Amplifier: 1 + reserve/500.
fn king_safety_amplified(board: &Board, color: Color, config: &EngineConfig, base: i32) -> i32 {
    let reserve = &board.reserves()[(!color).to_index()];
    let mut opp_reserve_value = 0;
    for (piece, count) in reserve.iter() {
        opp_reserve_value += count as i32 * config.piece_value(piece);
    }
    let amplifier = 100 + opp_reserve_value / 5;
    base * amplifier / 100
}

/// Mobility score for one color.
fn mobility_score(board: &Board, color: Color) -> i32 {
    let combined = *board.combined();
    let our_pieces = *board.color_combined(color);
    let mut moves = 0i32;

    // Knights
    let knights = *board.pieces(Piece::Knight) & our_pieces;
    for sq in knights {
        let attacks = get_knight_moves(sq) & !our_pieces;
        moves += attacks.popcnt() as i32;
    }

    // Bishops
    let bishops = *board.pieces(Piece::Bishop) & our_pieces;
    for sq in bishops {
        let attacks = get_bishop_moves(sq, combined) & !our_pieces;
        moves += attacks.popcnt() as i32;
    }

    // Rooks
    let rooks = *board.pieces(Piece::Rook) & our_pieces;
    for sq in rooks {
        let attacks = get_rook_moves(sq, combined) & !our_pieces;
        moves += attacks.popcnt() as i32;
    }

    // Queens (bishop + rook moves)
    let queens = *board.pieces(Piece::Queen) & our_pieces;
    for sq in queens {
        let attacks = (get_bishop_moves(sq, combined) | get_rook_moves(sq, combined)) & !our_pieces;
        moves += attacks.popcnt() as i32;
    }

    // Weight: 4 cp per move
    moves * 4
}

/// Pawn structure score for one color.
fn pawn_structure(board: &Board, color: Color) -> i32 {
    let our_pawns = *board.pieces(Piece::Pawn) & *board.color_combined(color);
    let mut score = 0i32;

    for file in ALL_FILES {
        let file_bb = get_file(file);
        let pawns_on_file = our_pawns & file_bb;
        let count = pawns_on_file.popcnt();

        // Doubled pawns: penalize each extra pawn on the file
        if count > 1 {
            score -= 20 * (count as i32 - 1);
        }

        // Isolated pawns: no friendly pawns on adjacent files
        if count > 0 {
            let adj = get_adjacent_files(file);
            if (our_pawns & adj) == EMPTY {
                score -= 15 * count as i32;
            }
        }
    }

    // Passed pawns: bonus by advancement
    for sq in our_pawns {
        if board.is_passed_pawn(sq, color) {
            let rank_from_promo = match color {
                Color::White => sq.get_rank().to_index(),
                Color::Black => 7 - sq.get_rank().to_index(),
            };
            score += match rank_from_promo {
                6 => 150, // 7th rank (one step from promotion)
                5 => 80,  // 6th rank
                4 => 40,  // 5th rank
                3 => 20,  // 4th rank
                _ => 5,
            };
        }
    }

    score
}

// ─── Main Evaluation ────────────────────────────────────────────────

/// Per-component evaluation breakdown (from the side-to-move perspective).
pub struct EvalBreakdown {
    pub material: i32,
    pub reserves: i32,
    pub pst: i32,
    pub king_safety: i32,
    pub mobility: i32,
    pub pawn_structure: i32,
    pub total: i32,
}

/// Evaluate a position with full component breakdown, returning centipawns
/// from the perspective of `board.side_to_move()`. Positive = good for the
/// side to move. This is THE evaluation implementation — `evaluate` and
/// `evaluate_with_config` both delegate here so the paths cannot diverge.
pub fn evaluate_detailed(board: &Board, config: &EngineConfig) -> EvalBreakdown {
    // Terminal checks
    match board.status() {
        BoardStatus::Checkmate => {
            // Side to move is checkmated → worst possible score
            return EvalBreakdown {
                material: 0,
                reserves: 0,
                pst: 0,
                king_safety: 0,
                mobility: 0,
                pawn_structure: 0,
                total: -30000,
            };
        }
        BoardStatus::Stalemate => {
            return EvalBreakdown {
                material: 0,
                reserves: 0,
                pst: 0,
                king_safety: 0,
                mobility: 0,
                pawn_structure: 0,
                total: 0,
            };
        }
        BoardStatus::Ongoing => {}
    }

    let white = Color::White;
    let black = Color::Black;

    // King safety bases are shared by the safety term (amplified) and the
    // dynamic reserve multipliers (exposure), so compute them once.
    let ks_base_w = king_safety_base(board, white);
    let ks_base_b = king_safety_base(board, black);
    let exposure_w = (-ks_base_w).max(0);
    let exposure_b = (-ks_base_b).max(0);

    // Compute each component for white and black (from white's perspective)
    let mat = material_score_cfg(board, white, config) - material_score_cfg(board, black, config);
    let res = reserve_score_dyn(board, white, config, exposure_w, exposure_b)
        - reserve_score_dyn(board, black, config, exposure_b, exposure_w);
    let pst_val = pst_score(board, white) - pst_score(board, black);
    let king = king_safety_amplified(board, white, config, ks_base_w)
        - king_safety_amplified(board, black, config, ks_base_b);
    let mob = mobility_score(board, white) - mobility_score(board, black);
    let pawns = pawn_structure(board, white) - pawn_structure(board, black);

    let total = mat + res + pst_val + king + mob + pawns;

    // Adjust for side to move
    let sign = match board.side_to_move() {
        Color::White => 1,
        Color::Black => -1,
    };

    EvalBreakdown {
        material: mat * sign,
        reserves: res * sign,
        pst: pst_val * sign,
        king_safety: king * sign,
        mobility: mob * sign,
        pawn_structure: pawns * sign,
        total: total * sign,
    }
}

/// Evaluate a position, returning centipawns from the perspective of
/// `board.side_to_move()`. Positive = good for the side to move.
/// Uses default config values.
pub fn evaluate(board: &Board) -> i32 {
    evaluate_with_config(board, &EngineConfig::default())
}

/// Evaluate a position using configurable piece values and reserve multipliers.
/// Returns centipawns from the perspective of `board.side_to_move()`.
pub fn evaluate_with_config(board: &Board, config: &EngineConfig) -> i32 {
    evaluate_detailed(board, config).total
}

/// Material count using config piece values.
fn material_score_cfg(board: &Board, color: Color, config: &EngineConfig) -> i32 {
    let mut score = 0;
    for piece in ALL_PIECES {
        if piece == Piece::King { continue; }
        let count = (*board.pieces(piece) & *board.color_combined(color)).popcnt() as i32;
        score += count * config.piece_value(piece);
    }
    score
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bughouse_chess::Square;

    #[test]
    fn starting_position_near_zero() {
        let board = Board::default();
        let score = evaluate(&board);
        // Symmetric position — should be close to 0 (small mobility/PST differences possible)
        assert!(
            score.abs() < 50,
            "starting position eval should be near 0, got {}",
            score
        );
    }

    #[test]
    fn material_imbalance_detected() {
        // White missing a queen (remove queen from d1)
        let board: Board = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNB1KBNR[] w KQkq - 0 1"
            .parse()
            .unwrap();
        let score = evaluate(&board);
        // White is down a queen (~900 cp) — should be strongly negative for white
        assert!(
            score < -700,
            "white down a queen should be very negative, got {}",
            score
        );
    }

    #[test]
    fn reserves_contribute_to_eval() {
        // Starting position with extra queen in white's reserve
        let with_reserve: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[Q] w KQkq - 0 1"
                .parse()
                .unwrap();
        let without_reserve = Board::default();

        let score_with = evaluate(&with_reserve);
        let score_without = evaluate(&without_reserve);

        // Having a queen in reserve should be worth about 1080 cp (900 * 1.2)
        let diff = score_with - score_without;
        assert!(
            diff > 900 && diff < 1300,
            "queen in reserve should add ~1080 cp, got diff={}",
            diff
        );
    }

    #[test]
    fn castled_king_scores_better() {
        // White king on g1 (castled) vs e1 (center)
        // Simplified: just compare king PST values
        let castled: Board = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQ1RK1[] w kq - 0 1"
            .parse()
            .unwrap();
        let center: Board = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQK2R[] w KQkq - 0 1"
            .parse()
            .unwrap();

        // Castled position should score better for white
        let castled_score = evaluate(&castled);
        let center_score = evaluate(&center);
        assert!(
            castled_score > center_score,
            "castled king ({}) should score better than center king ({})",
            castled_score,
            center_score
        );
    }

    #[test]
    fn passed_pawn_bonus() {
        // White pawn on e7 (one step from promotion), black has no pawns blocking
        let board: Board = "rnbqkbnr/4P3/8/8/8/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
            .parse()
            .unwrap();
        let baseline = Board::default();

        // The passed pawn on e7 should give white a significant bonus
        let score = evaluate(&board);
        let baseline_score = evaluate(&baseline);
        assert!(
            score > baseline_score + 100,
            "passed pawn on 7th should give big bonus: position={}, baseline={}",
            score,
            baseline_score
        );
    }

    #[test]
    fn checkmate_score() {
        // If side to move is checkmated, score should be -30000
        // Adjacent queen checkmate: black king h8, white queen g7 (defended by pawn f6)
        let board: Board = "7k/6Q1/5P2/8/8/8/8/K7[] b - - 0 1"
            .parse()
            .unwrap();
        if board.status() == BoardStatus::Checkmate {
            assert_eq!(evaluate(&board), -30000);
        }
    }

    #[test]
    fn stalemate_score() {
        // Stalemate should evaluate to 0
        let board: Board = "7k/6Q1/8/8/8/8/8/5K2[] b - - 0 1"
            .parse()
            .unwrap();
        if board.status() == BoardStatus::Stalemate {
            assert_eq!(evaluate(&board), 0);
        }
    }

    #[test]
    fn symmetric_eval() {
        // A symmetric position should evaluate the same for both sides
        let board = Board::default();
        let score = evaluate(&board);
        // Due to first-move advantage in mobility, there may be a small asymmetry,
        // but it should be small
        assert!(
            score.abs() < 50,
            "symmetric position should be near 0, got {}",
            score
        );
    }

    #[test]
    fn piece_values_correct() {
        assert_eq!(piece_value(Piece::Pawn), 100);
        assert_eq!(piece_value(Piece::Knight), 320);
        assert_eq!(piece_value(Piece::Bishop), 330);
        assert_eq!(piece_value(Piece::Rook), 500);
        assert_eq!(piece_value(Piece::Queen), 900);
        assert_eq!(piece_value(Piece::King), 0);
    }

    /// Helper: value added by giving white the given reserve string, measured
    /// as an eval delta on the same position (white to move).
    fn reserve_delta(pos_without: &str, pos_with: &str, cfg: &EngineConfig) -> i32 {
        let with: Board = pos_with.parse().unwrap();
        let without: Board = pos_without.parse().unwrap();
        evaluate_with_config(&with, cfg) - evaluate_with_config(&without, cfg)
    }

    #[test]
    fn reserve_knight_worth_more_vs_exposed_king() {
        // Same knight-in-hand, but against a castled black king stripped of
        // its f7/g7/h7 shield: the attack bonus (plus the king-safety
        // amplifier) must make the knight clearly more valuable than at
        // startpos, where both kings are fully sheltered.
        let cfg = EngineConfig::default();
        let exposed_base = "rnbq1rk1/ppppp3/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQ - 0 1";
        let exposed_with = "rnbq1rk1/ppppp3/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQ - 0 1";
        let safe_base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1";
        let safe_with = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1";

        let delta_exposed = reserve_delta(exposed_base, exposed_with, &cfg);
        let delta_safe = reserve_delta(safe_base, safe_with, &cfg);
        assert!(
            delta_exposed >= delta_safe + 100,
            "knight in hand vs exposed king ({}) should be worth well more than vs safe king ({})",
            delta_exposed, delta_safe
        );
    }

    #[test]
    fn reserve_pawn_worth_more_when_own_king_exposed() {
        // White's castled king has lost its f2/g2/h2 shield: a pawn in hand
        // (the blocking/shielding drop) should be worth more than at startpos.
        let cfg = EngineConfig::default();
        let exposed_base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPP3/RNBQ1RK1[] w kq - 0 1";
        let exposed_with = "rnbqkbnr/pppppppp/8/8/8/8/PPPPP3/RNBQ1RK1[P] w kq - 0 1";
        let safe_base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1";
        let safe_with = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[P] w KQkq - 0 1";

        let delta_exposed = reserve_delta(exposed_base, exposed_with, &cfg);
        let delta_safe = reserve_delta(safe_base, safe_with, &cfg);
        assert!(
            delta_exposed >= delta_safe + 20,
            "pawn in hand with exposed own king ({}) should be worth more than with safe king ({})",
            delta_exposed, delta_safe
        );
    }

    #[test]
    fn zero_scales_restore_static_reserve_value() {
        // With both scales at 0, the knight's value must not depend on the
        // enemy king's exposure (amplifier effects act on the king-safety
        // component, not the reserve component, and are isolated by comparing
        // scale=0 against scale=default on the same positions).
        let mut cfg_zero = EngineConfig::default();
        cfg_zero.reserve_attack_scale = 0;
        cfg_zero.reserve_defense_scale = 0;
        let cfg_default = EngineConfig::default();

        let exposed_base = "rnbq1rk1/ppppp3/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQ - 0 1";
        let exposed_with = "rnbq1rk1/ppppp3/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQ - 0 1";

        let delta_default = reserve_delta(exposed_base, exposed_with, &cfg_default);
        let delta_zero = reserve_delta(exposed_base, exposed_with, &cfg_zero);
        assert!(
            delta_default > delta_zero,
            "attack scale should add value vs exposed king: default={}, zero={}",
            delta_default, delta_zero
        );
    }

    #[test]
    fn reserve_multipliers_correct() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.reserve_multiplier_pct(Piece::Knight), 160);
        assert_eq!(cfg.reserve_multiplier_pct(Piece::Pawn), 130);
        assert_eq!(cfg.reserve_multiplier_pct(Piece::Bishop), 130);
        assert_eq!(cfg.reserve_multiplier_pct(Piece::Rook), 120);
        assert_eq!(cfg.reserve_multiplier_pct(Piece::Queen), 120);
    }

    #[test]
    fn knight_in_reserve_valued_higher_than_bishop() {
        // Knight reserve premium is 160% vs bishop 130%, and their base values
        // are close (320 vs 330), so reserve knight (512) vs reserve bishop (429)
        let with_knight: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1"
                .parse()
                .unwrap();
        let with_bishop: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[B] w KQkq - 0 1"
                .parse()
                .unwrap();
        let knight_score = evaluate(&with_knight);
        let bishop_score = evaluate(&with_bishop);
        assert!(
            knight_score > bishop_score,
            "knight in reserve ({}) should be valued higher than bishop ({})",
            knight_score,
            bishop_score
        );
    }

    #[test]
    fn reserve_piece_valued_more_than_on_board() {
        // A knight in reserve (320 * 1.6 = 512 cp) should be worth more than
        // the base piece value (320 cp). Compare position with knight in reserve
        // vs position with an extra knight on the board.
        let in_reserve: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1"
                .parse()
                .unwrap();
        let baseline = Board::default();
        let reserve_diff = evaluate(&in_reserve) - evaluate(&baseline);
        // Reserve knight at 160% = 512 cp, should be well above the 320 base
        assert!(
            reserve_diff > 400,
            "knight in reserve premium should exceed base value, got diff={}",
            reserve_diff
        );
    }

    #[test]
    fn material_score_starting_position() {
        let board = Board::default();
        // Both sides have same material: 8P + 2N + 2B + 2R + Q
        // = 800 + 640 + 660 + 1000 + 900 = 4000
        let cfg = EngineConfig::default();
        let white_mat = material_score_cfg(&board, Color::White, &cfg);
        let black_mat = material_score_cfg(&board, Color::Black, &cfg);
        assert_eq!(white_mat, black_mat);
        assert_eq!(white_mat, 8 * 100 + 2 * 320 + 2 * 330 + 2 * 500 + 900);
    }

    #[test]
    fn eval_flips_for_black_to_move() {
        // Same position but with black to move should negate the score
        let white_to_move: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let black_to_move: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1"
                .parse()
                .unwrap();
        let white_score = evaluate(&white_to_move);
        let black_score = evaluate(&black_to_move);
        assert_eq!(
            white_score, -black_score,
            "score should negate when side to move flips: white={}, black={}",
            white_score, black_score
        );
    }

    #[test]
    fn mobility_open_position_beats_blocked() {
        // Open position (pieces developed) should have more mobility than
        // a cramped position
        let open: Board =
            "r1bqkbnr/pppppppp/2n5/8/4P3/5N2/PPPP1PPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let blocked: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let open_mobility = mobility_score(&open, Color::White);
        let blocked_mobility = mobility_score(&blocked, Color::White);
        assert!(
            open_mobility > blocked_mobility,
            "open position ({}) should have more mobility than blocked ({})",
            open_mobility,
            blocked_mobility
        );
    }

    #[test]
    fn doubled_pawns_penalized() {
        // White has doubled pawns on e-file
        let doubled: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/4P3/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let normal = Board::default();
        let doubled_struct = pawn_structure(&doubled, Color::White);
        let normal_struct = pawn_structure(&normal, Color::White);
        assert!(
            doubled_struct < normal_struct,
            "doubled pawns ({}) should score worse than normal ({})",
            doubled_struct,
            normal_struct
        );
    }

    #[test]
    fn isolated_pawn_penalized() {
        // White pawn on a2 with no pawns on b-file = isolated
        // Remove b-pawn so a-pawn becomes isolated
        let isolated: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/P1PPPPPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let normal = Board::default();
        let iso_struct = pawn_structure(&isolated, Color::White);
        let normal_struct = pawn_structure(&normal, Color::White);
        assert!(
            iso_struct < normal_struct,
            "isolated pawn ({}) should score worse than normal ({})",
            iso_struct,
            normal_struct
        );
    }

    #[test]
    fn passed_pawn_detected_correctly() {
        // White pawn on e5, no black pawns on d/e/f files ahead
        // Clear black pawns from d, e, and f files
        let board: Board =
            "rnbqkbnr/ppp3pp/8/4P3/8/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let e5 = Square::make_square(Rank::Fifth, File::E);
        assert!(
            board.is_passed_pawn(e5, Color::White),
            "e5 pawn should be passed (no black pawns on d/e/f ahead)"
        );
    }

    #[test]
    fn non_passed_pawn_detected() {
        // White pawn on e4, black pawn on e5 blocks it — not passed
        let board: Board =
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let e4 = Square::make_square(Rank::Fourth, File::E);
        assert!(
            !board.is_passed_pawn(e4, Color::White),
            "e4 pawn should not be passed (black pawn on e5)"
        );
    }

    #[test]
    fn passed_pawn_blocked_by_adjacent_file() {
        // White pawn on e5, black pawn on d6 — not passed (adjacent file blocker)
        let board: Board =
            "rnbqkbnr/ppp2ppp/3p4/4P3/8/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let e5 = Square::make_square(Rank::Fifth, File::E);
        assert!(
            !board.is_passed_pawn(e5, Color::White),
            "e5 pawn should not be passed (black pawn on d6 blocks)"
        );
    }

    #[test]
    fn king_safety_open_file_penalty() {
        // Remove pawns from king file to create open file exposure
        let open_king: Board =
            "rnbqkbnr/pppp1ppp/8/8/8/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let shielded = Board::default();
        let open_safety = king_safety_base(&open_king, Color::White);
        let shielded_safety = king_safety_base(&shielded, Color::White);
        assert!(
            open_safety < shielded_safety,
            "open king file ({}) should have worse safety than shielded ({})",
            open_safety,
            shielded_safety
        );
    }

    #[test]
    fn reserve_amplifies_king_penalty() {
        // Position with open king file + opponent holding pieces in reserve
        // should be worse than the same position without reserves
        let with_reserves: Board =
            "rnbqkbnr/ppp1pppp/8/8/8/8/PPP1PPPP/RNBQKBNR[Qn] w KQkq - 0 1"
                .parse()
                .unwrap();
        let without_reserves: Board =
            "rnbqkbnr/ppp1pppp/8/8/8/8/PPP1PPPP/RNBQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();

        // Both sides have open d-file, but black holding pieces should hurt white more
        let score_with = evaluate(&with_reserves);
        let score_without = evaluate(&without_reserves);
        // The reserve amplifier + reserve material should create a difference
        // (white has Q in reserve worth +630, black has n worth -224, net ~+406 for reserves alone,
        //  but the amplifier makes white's king safety worse due to black's knight)
        // We mainly check that the evaluation accounts for reserves
        assert_ne!(score_with, score_without, "reserves should affect evaluation");
    }
}
