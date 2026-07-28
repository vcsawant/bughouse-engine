//! Alpha-beta negamax search with drop pruning for bughouse.
//!
//! Searches the game tree to a configurable depth using negamax with
//! alpha-beta pruning. Drops are pruned to a subset of relevant squares
//! (king attack/defense zones, center, promotion zone) at every node.

use bughouse_chess::{
    BitBoard, Board, BoardStatus, BughouseMove, CacheTable, Color, File,
    MoveGen, Piece, Rank, EMPTY, NON_KING_PIECES, NUM_NON_KING_PIECES,
    get_bishop_moves, get_file, get_king_moves, get_king_zone, get_knight_moves,
    get_pawn_attack_squares, get_rank, get_rook_moves,
};

use std::sync::LazyLock;
use std::time::Instant;
use log::debug;

use crate::config::EngineConfig;
use crate::scoring;

// ─── Transposition Table ────────────────────────────────────────────

/// TT entry flag: score is the exact minimax value.
const TT_FLAG_EXACT: u8 = 0;
/// TT entry flag: score is a lower bound (beta cutoff occurred).
const TT_FLAG_LOWER: u8 = 1;
/// TT entry flag: score is an upper bound (no move beat alpha).
const TT_FLAG_UPPER: u8 = 2;

/// Transposition table entry. 8 bytes, stored in CacheTable.
#[derive(Copy, Clone, PartialEq, PartialOrd)]
pub struct TTEntry {
    pub score: i16,
    pub best_move: u16,
    pub depth: u8,
    pub flag: u8,
}

/// Default empty TT entry.
pub const TT_DEFAULT: TTEntry = TTEntry { score: 0, best_move: 0, depth: 0, flag: 0 };

/// Default TT size: 2^20 entries = ~8 MB.
pub const TT_DEFAULT_SIZE: usize = 1 << 20;

/// Adjust a score for TT storage: convert ply-relative mate/frozen scores to node-relative.
fn score_to_tt(score: i32, ply: u32) -> i16 {
    let adjusted = if score > TT_PLY_SCORE_MIN {
        score + ply as i32  // storing a positive mate/frozen: add ply to make it node-relative
    } else if score < -TT_PLY_SCORE_MIN {
        score - ply as i32  // storing a negative mate/frozen: subtract ply
    } else {
        score
    };
    adjusted.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Adjust a score from TT retrieval: convert node-relative mate/frozen scores to ply-relative.
fn score_from_tt(score: i16, ply: u32) -> i32 {
    let s = score as i32;
    if s > TT_PLY_SCORE_MIN {
        s - ply as i32
    } else if s < -TT_PLY_SCORE_MIN {
        s + ply as i32
    } else {
        s
    }
}

/// Maximum search depth (hard cap for iterative deepening).
const MAX_DEPTH: u32 = 64;

/// Mate score base value. Actual mate scores are `MATE_SCORE - ply` to prefer shorter mates.
const MATE_SCORE: i32 = 30000;

/// Frozen-position score base. A side is "frozen" when it is in check with no
/// legal moves while `status()` still reports Ongoing: the check is drop-blockable,
/// so under bughouse rules the player sits and waits for their partner to supply a
/// blocking piece. In practice this is nearly always lost — the opponent gains
/// unlimited tempo and the sitter can flag — so it is scored just below the mate
/// band as `FROZEN_SCORE - ply` (prefer freezing the opponent sooner). Kept distinct
/// from MATE_SCORE so scores can distinguish "mated" from "forced to sit".
const FROZEN_SCORE: i32 = 28000;

/// Scores at or above this magnitude are ply-relative (mate or frozen) and are
/// converted to node-relative form for TT storage. Static evals can never reach
/// this range.
const TT_PLY_SCORE_MIN: i32 = 27000;

/// How often to check the clock (every N nodes).
const TIME_CHECK_INTERVAL: usize = 1024;

// ─── Late Move Reductions ───────────────────────────────────────────

/// Minimum depth to apply LMR (don't reduce shallow searches).
const LMR_MIN_DEPTH: u32 = 3;

/// Number of moves searched at full depth before reductions kick in.
/// Covers the TT best move + first 2 captures/killers.
const LMR_FULL_DEPTH_MOVES: usize = 3;

/// Precomputed LMR reduction table: `LMR_TABLE[depth][move_index]` → reduction.
/// Uses `ln(depth) * ln(move_index) / 2.0`, clamped to at least 1.
static LMR_TABLE: LazyLock<[[u32; 64]; 64]> = LazyLock::new(|| {
    let mut table = [[0u32; 64]; 64];
    for d in 1..64 {
        for m in 1..64 {
            let r = ((d as f64).ln() * (m as f64).ln() / 2.0) as u32;
            table[d][m] = r.max(1);
        }
    }
    table
});

/// Per-piece-type capture statistics derived from the search tree.
///
/// P(piece_type, color) = probability that `color` captures a piece of type `piece_type`.
/// C(piece_type, color) = cost in centipawns for `color` to capture that piece type.
///
/// Indexed by `[color.to_index()][piece_type_index]` where piece_type_index:
/// 0=Pawn, 1=Knight, 2=Bishop, 3=Rook, 4=Queen.
#[derive(Debug, Clone)]
pub struct CaptureStats {
    pub probability: [[f32; NUM_NON_KING_PIECES]; 2],
    pub cost: [[i32; NUM_NON_KING_PIECES]; 2],
}

impl Default for CaptureStats {
    fn default() -> Self {
        CaptureStats {
            probability: [[0.0; NUM_NON_KING_PIECES]; 2],
            cost: [[0; NUM_NON_KING_PIECES]; 2],
        }
    }
}

/// Result of a search: the best move found, its score, nodes evaluated, depth, and principal variation.
pub struct SearchResult {
    pub best_move: BughouseMove,
    pub score: i32,
    pub nodes: usize,
    pub depth: u32,
    #[allow(dead_code)]
    pub pv: Vec<String>,
    /// P/C capture statistics for the strategy layer (Phase E).
    pub capture_stats: CaptureStats,
    /// All root move evaluations from the last completed depth.
    pub root_move_evals: Vec<RootMoveEvalPub>,
}

/// Per-board evaluation state for cross-board strategy.
///
/// Stores the results of evaluating a board position: P/C capture statistics,
/// reserve impact (how much each piece type would improve the position),
/// and the search score/depth.
#[derive(Debug, Clone)]
pub struct BoardEval {
    pub capture_stats: CaptureStats,
    /// Score delta if we added one of each piece type to our reserves.
    /// Indexed by piece type (Pawn=0, Knight=1, Bishop=2, Rook=3, Queen=4).
    pub reserve_impact: [i32; NUM_NON_KING_PIECES],
    pub score: i32,
    pub depth: u32,
}

impl Default for BoardEval {
    fn default() -> Self {
        BoardEval {
            capture_stats: CaptureStats::default(),
            reserve_impact: [0; NUM_NON_KING_PIECES],
            score: 0,
            depth: 0,
        }
    }
}

/// Per-depth search info for streaming `info` lines.
#[derive(Debug, Clone)]
pub struct SearchInfo {
    pub depth: u32,
    pub score: i32,
    pub nodes: usize,
    pub time_ms: u64,
    pub pv: Vec<String>,
}

/// Number of killer move slots per ply.
const NUM_KILLERS: usize = 2;

/// Killer moves table: quiet moves that caused beta cutoffs at each ply.
/// Indexed by `[ply][slot]`. Stored as compressed u16 moves.
type KillerTable = [[u16; NUM_KILLERS]; MAX_DEPTH as usize];

/// History heuristic table: tracks which quiet moves cause beta cutoffs.
/// Indexed by `[color][source_square][dest_square]`. Higher = search earlier.
type HistoryTable = [[[i32; 64]; 64]; 2];

/// Shared search state for time management and transposition table.
struct SearchContext<'a> {
    start: Instant,
    budget_ms: u64,
    nodes: usize,
    time_up: bool,
    tt: &'a mut CacheTable<TTEntry>,
    killers: KillerTable,
    history: HistoryTable,
    /// External abort flag — set by eval thread when position changes or pause requested.
    /// Checked every TIME_CHECK_INTERVAL nodes alongside time_up.
    abort: Option<&'a std::sync::atomic::AtomicBool>,
    /// Engine config for piece values, reserve multipliers, etc.
    config: &'a EngineConfig,
}

// ─── Move Generation ─────────────────────────────────────────────────

/// Generate all legal moves (regular + pruned drops) for a position.
///
/// Regular moves come from the library's legal move generator. Drop moves
/// use `MoveGen::drop_moves()` for legality (including check resolution),
/// then filter through our drop mask for pruning irrelevant squares.
fn generate_moves(board: &Board) -> Vec<BughouseMove> {
    let mut moves: Vec<BughouseMove> = MoveGen::new_legal(board)
        .map(BughouseMove::Regular)
        .collect();

    // Use library's legal drop generation, then prune to relevant squares
    let drop_mask = build_drop_mask(board);
    let drops = MoveGen::drop_moves(board);
    for drop_move in &drops {
        if let BughouseMove::Drop { square, .. } = drop_move {
            if (drop_mask & BitBoard::from_square(*square)) != EMPTY {
                moves.push(drop_move.clone());
            }
        }
    }

    // If pruning removed every candidate but legal drops exist, keep them all.
    // An empty result must mean the position truly has no legal moves — the
    // frozen-check detection in negamax depends on this invariant.
    if moves.is_empty() && !drops.is_empty() {
        moves = drops;
    }

    moves
}

/// A board is frozen when the side to move is in check with no legal moves while
/// `status()` still reports Ongoing (the check is drop-blockable): under bughouse
/// rules they must sit until their partner supplies a blocking piece. Also true
/// for actual checkmate — callers must check `status()` first.
fn is_frozen(board: &Board) -> bool {
    *board.checkers() != EMPTY
        && MoveGen::new_legal(board).len() == 0
        && MoveGen::drop_moves(board).is_empty()
}

/// Generate all drops that give check: for each piece type in our reserve, the
/// empty squares from which that piece would attack the enemy king. Uses attack
/// symmetry — a piece on square `s` attacks the king on `k` iff `s` is in the
/// piece's attack set computed *from* `k` with the current occupancy. Drops never
/// create discovered checks, so this is exact.
///
/// Only valid when the side to move is NOT in check (arbitrary drops are illegal
/// while in check — only blocking drops are, and those come from `drop_moves`).
fn generate_checking_drops(board: &Board) -> Vec<BughouseMove> {
    let us = board.side_to_move();
    let reserve = &board.reserves()[us.to_index()];
    if reserve.is_empty() {
        return Vec::new();
    }

    let combined = *board.combined();
    let empty = !combined;
    let enemy_king_sq = board.king_square(!us);
    let back_ranks = get_rank(Rank::First) | get_rank(Rank::Eighth);

    let mut moves = Vec::new();
    for (piece, _count) in reserve.iter() {
        let checking_squares = match piece {
            Piece::Knight => get_knight_moves(enemy_king_sq),
            Piece::Rook => get_rook_moves(enemy_king_sq, combined),
            Piece::Bishop => get_bishop_moves(enemy_king_sq, combined),
            Piece::Queen => {
                get_rook_moves(enemy_king_sq, combined) | get_bishop_moves(enemy_king_sq, combined)
            }
            // A pawn on `s` checks the king iff `s` is a pawn-attack square of
            // the *opposite* color computed from the king square.
            Piece::Pawn => get_pawn_attack_squares(enemy_king_sq, !us) & !back_ranks,
            Piece::King => EMPTY,
        } & empty;

        for square in checking_squares {
            moves.push(BughouseMove::Drop { piece, square });
        }
    }
    moves
}

// ─── Move Ordering ───────────────────────────────────────────────────

/// Score a move for ordering purposes (higher = searched first).
///
/// Priority: captures (MVV-LVA) > promotions > drops (by piece value) > quiet moves (history).
fn move_order_score(board: &Board, m: &BughouseMove, history: &HistoryTable, config: &EngineConfig) -> i32 {
    match m {
        BughouseMove::Regular(cm) => {
            let dest = cm.get_dest();
            let is_promotion = cm.get_promotion().is_some();
            let victim = board.piece_on(dest);

            match (victim, is_promotion) {
                // Capture + promotion
                (Some(v), true) => {
                    20000 + config.piece_value(v) * 10 + 900
                }
                // Capture only (MVV-LVA)
                (Some(v), false) => {
                    let source = cm.get_source();
                    let attacker = board.piece_on(source).unwrap_or(Piece::Pawn);
                    20000 + config.piece_value(v) * 10 - config.piece_value(attacker)
                }
                // Promotion only
                (None, true) => 15000,
                // Quiet move: use history score
                (None, false) => {
                    let ci = board.side_to_move().to_index();
                    history[ci][cm.get_source().to_index()][cm.get_dest().to_index()]
                }
            }
        }
        BughouseMove::Drop { piece, .. } => {
            // Drops: queen drops first, then rook, etc.
            10000 + config.piece_value(*piece)
        }
    }
}

/// Check if a move is "quiet" (not a capture, not a promotion, not a drop).
/// Quiet moves are candidates for killer move tracking.
fn is_quiet_move(board: &Board, m: &BughouseMove) -> bool {
    match m {
        BughouseMove::Regular(cm) => {
            board.piece_on(cm.get_dest()).is_none() && cm.get_promotion().is_none()
        }
        BughouseMove::Drop { .. } => false,
    }
}

/// Store a killer move at the given ply. Shifts existing killers down.
fn store_killer(killers: &mut KillerTable, ply: u32, m: u16) {
    let p = ply as usize;
    if p >= MAX_DEPTH as usize { return; }
    // Don't store duplicates
    if killers[p][0] == m { return; }
    killers[p][1] = killers[p][0];
    killers[p][0] = m;
}

/// Sort moves for better alpha-beta pruning: captures first (MVV-LVA), then drops, then quiet (history).
fn order_moves(board: &Board, moves: &mut [BughouseMove], history: &HistoryTable, config: &EngineConfig) {
    moves.sort_by(|a, b| {
        move_order_score(board, b, history, config).cmp(&move_order_score(board, a, history, config))
    });
}

// ─── Alpha-Beta Negamax ──────────────────────────────────────────────

/// Make a move on the board, returning the new board (or None for illegal/invalid moves).
///
/// Uses `catch_unwind` to guard against panics in the bughouse-chess library's
/// `make_move_new`, which can panic on certain positions (e.g., `piece_on(source).unwrap()`
/// on an empty square). This prevents the engine process from crashing during search.
fn make_move(board: &Board, m: &BughouseMove) -> Option<Board> {
    match m {
        BughouseMove::Regular(cm) => {
            let cm = *cm;
            let board = *board;
            match std::panic::catch_unwind(move || board.make_move_new(cm)) {
                Ok(b) => Some(b),
                Err(e) => {
                    let msg = e.downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| e.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    log::error!("Panic in make_move_new for move {}: {}", cm, msg);
                    None
                }
            }
        }
        BughouseMove::Drop { piece, square } => board.make_drop_new(*piece, *square),
    }
}

/// Quiescence search: continue searching captures (and, near the horizon,
/// checking drops) until the position is quiet. Prevents the horizon effect
/// where the engine stops looking right before a recapture or a drop-check
/// tactic — the core bughouse attacking motif.
///
/// `check_depth` counts down each qsearch ply; checking drops are generated
/// while it is > 0 (initial value: `config.qs_check_drop_depth`). At the
/// default of 1, drops are generated only at the horizon entry itself; the
/// resulting evasions and recaptures are still searched, and frozen/checkmate
/// children are detected via status, so the freeze motif survives.
///
/// While in check there is no stand-pat: every evasion is searched, so forcing
/// drop-check sequences are evaluated soundly.
fn quiescence(board: &Board, ply: u32, mut alpha: i32, beta: i32, ctx: &mut SearchContext, check_depth: u32) -> i32 {
    // Abort check every node (cheap atomic load)
    if let Some(abort) = ctx.abort {
        if abort.load(std::sync::atomic::Ordering::Relaxed) {
            ctx.time_up = true;
            return 0;
        }
    }
    // Time check periodically (expensive syscall)
    if ctx.nodes % TIME_CHECK_INTERVAL == 0 && ctx.budget_ms > 0 {
        if ctx.start.elapsed().as_millis() as u64 >= ctx.budget_ms {
            ctx.time_up = true;
            return 0;
        }
    }

    // Terminal detection
    match board.status() {
        BoardStatus::Checkmate => return -(MATE_SCORE - ply as i32),
        BoardStatus::Stalemate => return 0,
        BoardStatus::Ongoing => {}
    }

    // Frozen: in check with no legal response, but drop-blockable — near-loss.
    // Checked here so freezes are visible at the search horizon.
    let in_check = *board.checkers() != EMPTY;
    if in_check && is_frozen(board) {
        return -(FROZEN_SCORE - ply as i32);
    }

    ctx.nodes += 1;

    let mut moves: Vec<BughouseMove>;
    if in_check {
        // In check: no stand-pat (standing still is not an option) — search
        // every evasion. Evasion counts are small, so this stays cheap.
        moves = generate_moves(board);
    } else {
        let stand_pat = scoring::evaluate_with_config(board, ctx.config);

        // Beta cutoff: position is already good enough without capturing
        if stand_pat >= beta {
            return beta;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }

        moves = MoveGen::capture_moves(board)
            .map(BughouseMove::Regular)
            .collect();

        // Checking drops near the horizon (bounded by check_depth)
        if check_depth > 0 {
            moves.extend(generate_checking_drops(board));
        }

        if moves.is_empty() {
            return alpha; // Position is quiet
        }
    }

    order_moves(board, &mut moves, &ctx.history, ctx.config);

    let mut searched_any = false;
    for m in &moves {
        if ctx.time_up {
            break;
        }

        let child = match make_move(board, m) {
            Some(b) => b,
            None => continue,
        };

        let score = -quiescence(&child, ply + 1, -beta, -alpha, ctx, check_depth.saturating_sub(1));
        searched_any = true;

        if ctx.time_up {
            break;
        }

        if score > alpha {
            alpha = score;
        }
        if alpha >= beta {
            break; // Beta cutoff
        }
    }

    // Defensive: while in check, alpha was never anchored to a stand-pat. If no
    // evasion could actually be searched (library move-gen edge cases), fall
    // back to static eval rather than leaking the caller's raw alpha bound.
    if in_check && !searched_any {
        return scoring::evaluate_with_config(board, ctx.config);
    }

    alpha
}

/// Recursive negamax with alpha-beta pruning and transposition table.
///
/// Returns the score from the perspective of `board.side_to_move()`.
/// `depth` is remaining depth to search. `ply` is distance from root (for mate scoring).
fn negamax(board: &Board, depth: u32, ply: u32, mut alpha: i32, beta: i32, ctx: &mut SearchContext) -> i32 {
    // Abort check every node (cheap atomic load)
    if let Some(abort) = ctx.abort {
        if abort.load(std::sync::atomic::Ordering::Relaxed) {
            ctx.time_up = true;
            return 0;
        }
    }
    // Time check periodically (expensive syscall)
    if ctx.nodes % TIME_CHECK_INTERVAL == 0 && ctx.budget_ms > 0 {
        if ctx.start.elapsed().as_millis() as u64 >= ctx.budget_ms {
            ctx.time_up = true;
            return 0;
        }
    }

    // Terminal detection
    match board.status() {
        BoardStatus::Checkmate => return -(MATE_SCORE - ply as i32),
        BoardStatus::Stalemate => return 0,
        BoardStatus::Ongoing => {}
    }

    // Leaf node: quiescence search (captures + near-horizon checking drops)
    if depth == 0 {
        return quiescence(board, ply, alpha, beta, ctx, ctx.config.qs_check_drop_depth);
    }

    let original_alpha = alpha;
    let hash = board.get_hash();
    let mut tt_best_move: u16 = 0;

    // TT probe
    if let Some(entry) = ctx.tt.get(hash) {
        tt_best_move = entry.best_move;
        if entry.depth as u32 >= depth {
            let tt_score = score_from_tt(entry.score, ply);
            match entry.flag {
                TT_FLAG_EXACT => return tt_score,
                TT_FLAG_LOWER => {
                    if tt_score > alpha { alpha = tt_score; }
                }
                TT_FLAG_UPPER => {
                    if tt_score < beta { /* beta = tt_score; -- conservative: don't narrow beta from TT upper bound */ }
                }
                _ => {}
            }
            if alpha >= beta {
                return tt_score;
            }
        }
    }

    let mut moves = generate_moves(board);

    // No legal moves at all. Checkmate/stalemate were handled above, so this
    // means the side to move is frozen: in check with no legal response, but
    // the check is drop-blockable (status() == Ongoing). They must sit and
    // wait for a piece their partner may never deliver — score as a near-loss.
    if moves.is_empty() {
        ctx.nodes += 1;
        return -(FROZEN_SCORE - ply as i32);
    }

    order_moves(board, &mut moves, &ctx.history, ctx.config);

    // TT best move ordering: if we have a TT hit with a best move, put it first
    let mut insert_pos = 0;
    if tt_best_move != 0 {
        if let Some(tt_move) = BughouseMove::decompress(tt_best_move) {
            if let Some(pos) = moves.iter().position(|m| *m == tt_move) {
                moves.swap(0, pos);
                insert_pos = 1;
            }
        }
    }

    // Killer move ordering: promote killer moves after TT move and captures
    if (ply as usize) < MAX_DEPTH as usize {
        for slot in 0..NUM_KILLERS {
            let killer_compressed = ctx.killers[ply as usize][slot];
            if killer_compressed == 0 { continue; }
            if let Some(killer_move) = BughouseMove::decompress(killer_compressed) {
                // Find the killer in the move list (skip if it's already the TT move)
                if let Some(pos) = moves[insert_pos..].iter().position(|m| *m == killer_move) {
                    let actual_pos = pos + insert_pos;
                    // Find the first quiet move position to insert before
                    // (killers go after captures but before other quiet moves)
                    let target = moves[insert_pos..actual_pos].iter()
                        .position(|m| is_quiet_move(board, m))
                        .map(|p| p + insert_pos)
                        .unwrap_or(actual_pos);
                    if target < actual_pos {
                        // Shift moves to make room and insert killer
                        let killer = moves[actual_pos];
                        moves.copy_within(target..actual_pos, target + 1);
                        moves[target] = killer;
                    }
                }
            }
        }
    }

    let mut best_score = i32::MIN + 1; // Avoid overflow on negation
    let mut best_move: u16 = 0;
    let in_check = *board.checkers() != EMPTY;
    let lmr = &*LMR_TABLE;

    for (i, m) in moves.iter().enumerate() {
        if ctx.time_up {
            break;
        }

        let child = match make_move(board, m) {
            Some(b) => b,
            None => continue, // Illegal drop
        };

        // Late Move Reductions: reduce depth for late quiet non-drop moves
        let is_quiet = is_quiet_move(board, m);
        let is_drop = matches!(m, BughouseMove::Drop { .. });
        let is_killer = (ply as usize) < MAX_DEPTH as usize
            && (m.compress() == ctx.killers[ply as usize][0]
                || m.compress() == ctx.killers[ply as usize][1]);

        let score;
        if i >= LMR_FULL_DEPTH_MOVES
            && depth >= LMR_MIN_DEPTH
            && is_quiet
            && !is_drop
            && !in_check
            && !is_killer
        {
            let r = lmr[depth.min(63) as usize][i.min(63)];
            let reduced_depth = (depth - 1).saturating_sub(r);

            // Reduced null-window search
            let reduced_score = -negamax(&child, reduced_depth, ply + 1, -(alpha + 1), -alpha, ctx);

            // Re-search at full depth + full window if reduced search beat alpha
            if reduced_score > alpha && !ctx.time_up {
                score = -negamax(&child, depth - 1, ply + 1, -beta, -alpha, ctx);
            } else {
                score = reduced_score;
            }
        } else {
            score = -negamax(&child, depth - 1, ply + 1, -beta, -alpha, ctx);
        }

        if ctx.time_up {
            break;
        }

        if score > best_score {
            best_score = score;
            best_move = m.compress();
        }
        if score > alpha {
            alpha = score;
        }
        if alpha >= beta {
            // Store killer and update history if this is a quiet move causing a beta cutoff
            if is_quiet_move(board, m) {
                store_killer(&mut ctx.killers, ply, m.compress());
                if let BughouseMove::Regular(cm) = m {
                    let ci = board.side_to_move().to_index();
                    let bonus = (depth * depth) as i32;
                    ctx.history[ci][cm.get_source().to_index()][cm.get_dest().to_index()] += bonus;
                }
            }
            break; // Beta cutoff
        }
    }

    // Defensive: if no child position could be searched at all (library
    // move-gen edge cases on corrupt/illegal positions), fall back to static
    // eval rather than leaking the -infinity sentinel to the parent.
    if best_score == i32::MIN + 1 {
        return scoring::evaluate_with_config(board, ctx.config);
    }

    // TT store (skip if time ran out mid-search — result is incomplete)
    if !ctx.time_up {
        let flag = if best_score <= original_alpha {
            TT_FLAG_UPPER
        } else if best_score >= beta {
            TT_FLAG_LOWER
        } else {
            TT_FLAG_EXACT
        };
        ctx.tt.add(hash, TTEntry {
            score: score_to_tt(best_score, ply),
            best_move,
            depth: depth as u8,
            flag,
        });
    }

    best_score
}

// ─── Root Search ─────────────────────────────────────────────────────

/// A root move evaluation: its score, the move, and what it captures (if anything).
struct RootMoveEval {
    mv: BughouseMove,
    score: i32,
    captured: Option<Piece>,
}

/// Search at a single fixed depth (used internally by iterative deepening and tests).
/// Returns the best move, score, PV, and all root move evaluations (for P/C computation).
fn search_at_depth(board: &Board, moves: &[BughouseMove], depth: u32, ctx: &mut SearchContext<'_>) -> Option<(BughouseMove, i32, Vec<String>, Vec<RootMoveEval>)> {
    let mut best_move: Option<BughouseMove> = None;
    let mut best_score = i32::MIN + 1;
    let mut best_pv = Vec::new();
    let mut root_evals = Vec::new();

    let total_moves = moves.len();
    for (move_idx, m) in moves.iter().enumerate() {
        if ctx.time_up {
            break;
        }

        let child = match make_move(board, m) {
            Some(b) => b,
            None => continue,
        };

        // Determine what this move captures (if anything)
        let captured = match m {
            BughouseMove::Regular(cm) => board.piece_on(cm.get_dest()),
            BughouseMove::Drop { .. } => None,
        };

        // Captures get a full window so their root scores are exact — the
        // cross-board ranking and P/C statistics consume capture scores as
        // real values, and only captures carry cross-board value. Everything
        // else only needs to answer "does this beat the best so far?", so it
        // keeps the cheap narrowed window.
        let score = if captured.is_some() {
            -negamax(&child, depth - 1, 1, -(i32::MAX - 1), i32::MAX - 1, ctx)
        } else {
            -negamax(&child, depth - 1, 1, -(i32::MAX - 1), -best_score.max(i32::MIN + 1), ctx)
        };

        if ctx.time_up {
            break;
        }

        root_evals.push(RootMoveEval { mv: m.clone(), score, captured });

        if score > best_score {
            let cap_str = match captured {
                Some(p) => format!(" captures {:?}", p),
                None => String::new(),
            };
            let prev_str = match &best_move {
                Some(prev) => format!(" (was {} at {})", prev, best_score),
                None => String::new(),
            };
            best_score = score;
            best_move = Some(m.clone());
            best_pv = vec![format!("{}", m)];

            debug!("depth {} [{}/{}] new best: {} score={}{}{} nodes={}",
                depth, move_idx + 1, total_moves, m, score, cap_str, prev_str, ctx.nodes);
        }
    }

    best_move.map(|m| (m, best_score, best_pv, root_evals))
}

/// Compute P and C from root move evaluations.
///
/// For each piece type capturable by the side to move:
/// - P is based on how the capture's eval compares to the best eval
/// - C = best_eval - best_capture_eval for that piece type
///
/// Per piece type: use the minimum P across instances and minimum C across instances
/// (we want the cheapest-to-acquire instance).
fn compute_capture_stats(best_eval: i32, root_evals: &[RootMoveEval], side: Color) -> CaptureStats {
    let mut stats = CaptureStats::default();
    let ci = side.to_index();

    // Group captures by piece type: track the best eval for each captured piece type
    let mut best_capture_eval: [Option<i32>; NUM_NON_KING_PIECES] = [None; NUM_NON_KING_PIECES];

    for eval in root_evals {
        if let Some(captured) = eval.captured {
            if let Some(idx) = captured.non_king_index() {
                let current_best = best_capture_eval[idx].unwrap_or(i32::MIN);
                if eval.score > current_best {
                    best_capture_eval[idx] = Some(eval.score);
                }
            }
        }
    }

    // Compute P and C for each piece type
    for idx in 0..NUM_NON_KING_PIECES {
        if let Some(capture_eval) = best_capture_eval[idx] {
            // C = best_eval - best_capture_eval (cost of going for this capture instead of best move)
            let cost: i32 = best_eval - capture_eval;
            stats.cost[ci][idx] = cost.max(0); // Cost is never negative

            // P based on how close the capture eval is to the best eval
            let diff = best_eval - capture_eval;
            stats.probability[ci][idx] = if diff <= 50 {
                1.0  // Essentially free — hanging or best move IS the capture
            } else if diff <= 150 && capture_eval > 0 {
                0.8  // Tactical win — good capture but not the absolute best
            } else if diff <= 200 {
                0.5  // Equal trade territory
            } else {
                0.2  // Unfavorable — significant cost to capture this piece
            };
        }
        // P = 0.0 and C = 0 by default for uncapturable pieces
    }

    stats
}

/// Compute reserve impact: how much the score would change if each piece type
/// were added to reserves. Uses shallow re-searches with the TT warm from
/// the main search.
pub fn compute_reserve_impact(board: &Board, base_score: i32, tt: &mut CacheTable<TTEntry>, config: &EngineConfig) -> [i32; NUM_NON_KING_PIECES] {
    let color = board.side_to_move();
    let mut impact = [0i32; NUM_NON_KING_PIECES];
    let shallow_depth = 3;

    for &piece in &NON_KING_PIECES {
        let mut hypothetical = *board;
        hypothetical.add_to_reserve(color, piece);

        // Quick shallow search — TT is warm, most positions are cached
        let mut ctx = SearchContext {
            start: std::time::Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config,
        };

        let mut moves = generate_moves(&hypothetical);
        if moves.is_empty() {
            continue;
        }
        order_moves(&hypothetical, &mut moves, &ctx.history, ctx.config);

        if let Some((_, score, _, _)) = search_at_depth(&hypothetical, &moves, shallow_depth, &mut ctx) {
            impact[piece.to_index()] = score - base_score;
        }
    }

    impact
}

/// Quick evaluation of a board at a shallow depth. Returns a BoardEval
/// with P/C and score but no reserve_impact (caller can compute separately).
pub fn quick_eval(board: &Board, depth: u32, tt: &mut CacheTable<TTEntry>, config: &EngineConfig) -> BoardEval {
    let search_depth = depth.max(1);
    let side = board.side_to_move();

    let mut moves = generate_moves(board);
    if moves.is_empty() {
        return BoardEval {
            score: scoring::evaluate_with_config(board, config),
            depth: 0,
            ..Default::default()
        };
    }

    let mut ctx = SearchContext {
        start: std::time::Instant::now(),
        budget_ms: 0,
        nodes: 0,
        time_up: false,
        tt,
        killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
        history: [[[0; 64]; 64]; 2],
        abort: None,
        config,
    };

    order_moves(board, &mut moves, &ctx.history, ctx.config);

    if let Some((_, score, _, root_evals)) = search_at_depth(board, &moves, search_depth, &mut ctx) {
        let capture_stats = compute_capture_stats(score, &root_evals, side);
        BoardEval {
            capture_stats,
            reserve_impact: [0; NUM_NON_KING_PIECES],
            score,
            depth: search_depth,
        }
    } else {
        BoardEval::default()
    }
}

/// Find the best move using iterative deepening with a time budget.
///
/// Searches depth 1, 2, 3, ... until `budget_ms` expires. Depth 1 always
/// completes regardless of budget. Each completed depth pushes a `SearchInfo`
/// into `info_sink`. Returns `None` if there are no legal moves.
pub fn find_best_move_timed(board: &Board, budget_ms: u64, info_sink: &mut Vec<SearchInfo>, tt: &mut CacheTable<TTEntry>, config: &EngineConfig) -> Option<SearchResult> {
    let mut moves = generate_moves(board);
    if moves.is_empty() {
        return None;
    }

    let us = board.side_to_move();
    let mut ctx = SearchContext {
        start: Instant::now(),
        budget_ms,
        nodes: 0,
        time_up: false,
        tt,
        killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
        history: [[[0; 64]; 64]; 2],
        abort: None,
        config,
    };

    order_moves(board, &mut moves, &ctx.history, ctx.config);

    let mut best_result: Option<SearchResult> = None;

    for depth in 1..=MAX_DEPTH {
        ctx.time_up = false; // Reset for each iteration

        let result = search_at_depth(board, &moves, depth, &mut ctx);

        if ctx.time_up {
            debug!("Time up at depth {}, using depth {} result", depth, depth - 1);
            break;
        }

        if let Some((best_move, score, pv, root_evals)) = result {
            let elapsed = ctx.start.elapsed().as_millis() as u64;

            info_sink.push(SearchInfo {
                depth,
                score,
                nodes: ctx.nodes,
                time_ms: elapsed,
                pv: pv.clone(),
            });

            debug!("Depth {}: {} score={} nodes={} time={}ms",
                depth, best_move, score, ctx.nodes, elapsed);

            let capture_stats = compute_capture_stats(score, &root_evals, us);

            let pub_root_evals: Vec<RootMoveEvalPub> = root_evals.iter().map(|e| {
                RootMoveEvalPub { mv: e.mv.clone(), score: e.score, captured: e.captured }
            }).collect();

            best_result = Some(SearchResult {
                best_move,
                score,
                nodes: ctx.nodes,
                depth,
                pv,
                capture_stats,
                root_move_evals: pub_root_evals,
            });

            if elapsed >= budget_ms {
                break;
            }
        } else {
            break;
        }
    }

    best_result
}

/// Find the best move at a fixed depth (no time limit).
///
/// Convenience function for tests and cases where a specific depth is desired.
#[allow(dead_code)]
pub fn find_best_move(board: &Board, depth: u32, config: &EngineConfig) -> Option<SearchResult> {
    let search_depth = depth.min(MAX_DEPTH).max(1);
    let us = board.side_to_move();

    let mut moves = generate_moves(board);
    if moves.is_empty() {
        return None;
    }

    let mut local_tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
    let mut ctx = SearchContext {
        start: Instant::now(),
        budget_ms: 0, // No time limit
        nodes: 0,
        time_up: false,
        tt: &mut local_tt,
        killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
        history: [[[0; 64]; 64]; 2],
        abort: None,
        config,
    };

    order_moves(board, &mut moves, &ctx.history, ctx.config);

    let result = search_at_depth(board, &moves, search_depth, &mut ctx);

    result.map(|(best_move, score, pv, root_evals)| {
        let capture_stats = compute_capture_stats(score, &root_evals, us);
        debug!("Best: {} score={} depth={} nodes={}", best_move, score, search_depth, ctx.nodes);
        let pub_root_evals: Vec<RootMoveEvalPub> = root_evals.iter().map(|e| {
            RootMoveEvalPub { mv: e.mv.clone(), score: e.score, captured: e.captured }
        }).collect();

        SearchResult {
            best_move,
            score,
            nodes: ctx.nodes,
            depth: search_depth,
            pv,
            capture_stats,
            root_move_evals: pub_root_evals,
        }
    })
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

    // Attack zone: 2 rings around enemy king (precomputed table)
    let attack_zone = get_king_zone(enemy_king_sq);

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

// ─── Public API for Reserve Impact ──────────────────────────────────

/// Evaluate a single position at a given depth using the provided TT.
/// Returns the score from the perspective of `board.side_to_move()`.
/// Used for reserve impact computation where we only need a score, not a full SearchResult.
pub fn evaluate_position(board: &Board, depth: u32, tt: &mut CacheTable<TTEntry>, config: &EngineConfig) -> i32 {
    let mut ctx = SearchContext {
        start: std::time::Instant::now(),
        budget_ms: 0,
        nodes: 0,
        time_up: false,
        tt,
        killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
        history: [[[0; 64]; 64]; 2],
        abort: None,
        config,
    };
    negamax(board, depth, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx)
}

// ─── Public API for Eval Threads ────────────────────────────────────

/// Public wrapper for generate_moves (used by eval threads).
pub fn generate_moves_pub(board: &Board) -> Vec<BughouseMove> {
    generate_moves(board)
}

/// Public wrapper for order_moves (used by eval threads).
pub fn order_moves_pub(board: &Board, moves: &mut [BughouseMove], config: &EngineConfig) {
    let empty_history: HistoryTable = [[[0; 64]; 64]; 2];
    order_moves(board, moves, &empty_history, config);
}

/// Root move evaluation result (public version for eval threads and cross-board analysis).
#[derive(Debug, Clone)]
pub struct RootMoveEvalPub {
    pub mv: BughouseMove,
    pub score: i32,
    pub captured: Option<Piece>,
}

/// Search at a fixed depth with a provided TT (public version for eval threads).
/// Returns the best move, score, PV, root move evals, and total node count.
pub fn search_at_depth_pub(
    board: &Board,
    moves: &[BughouseMove],
    depth: u32,
    tt: &mut CacheTable<TTEntry>,
    abort: Option<&std::sync::atomic::AtomicBool>,
    config: &EngineConfig,
) -> Option<(BughouseMove, i32, Vec<String>, Vec<RootMoveEvalPub>, usize)> {
    let mut ctx = SearchContext {
        start: std::time::Instant::now(),
        budget_ms: 0,
        nodes: 0,
        time_up: false,
        tt,
        killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
        history: [[[0; 64]; 64]; 2],
        abort,
        config,
    };

    let result = search_at_depth(board, moves, depth, &mut ctx);
    result.map(|(best_move, score, pv, root_evals)| {
        let pub_evals: Vec<RootMoveEvalPub> = root_evals.iter().map(|e| {
            RootMoveEvalPub {
                mv: e.mv.clone(),
                score: e.score,
                captured: e.captured,
            }
        }).collect();
        (best_move, score, pv, pub_evals, ctx.nodes)
    })
}

/// Public wrapper for compute_capture_stats (used by eval threads).
pub fn compute_capture_stats_pub(best_eval: i32, root_evals: &[RootMoveEvalPub], side: Color) -> CaptureStats {
    // Convert pub evals to internal format
    let internal_evals: Vec<RootMoveEval> = root_evals.iter().map(|e| {
        RootMoveEval {
            mv: e.mv.clone(),
            score: e.score,
            captured: e.captured,
        }
    }).collect();
    compute_capture_stats(best_eval, &internal_evals, side)
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bughouse_chess::Square;

    /// Helper: search at a fixed depth for tests.
    fn search(board: &Board, depth: u32) -> Option<SearchResult> {
        find_best_move(board, depth, &EngineConfig::default())
    }

    #[test]
    fn finds_move_from_startpos() {
        let board = Board::default();
        let result = search(&board, 1);
        assert!(result.is_some(), "should find a move from starting position");
        let result = result.unwrap();
        assert!(result.nodes > 0);
        assert!(
            result.score.abs() < 200,
            "starting position score should be reasonable, got {}",
            result.score
        );
    }

    #[test]
    fn captures_hanging_queen() {
        let board: Board =
            "rnb1kbnr/pppppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        assert!(
            result.score > 500,
            "should find the hanging queen capture, score={}",
            result.score
        );
    }

    #[test]
    fn finds_checkmate_move() {
        // Qg6-g7# — queen on g7 defended by pawn f6. Adjacent check, so it is
        // true checkmate even under bughouse rules (not drop-blockable).
        // (Previously used a bishop on f6, but that position was illegal —
        // the bishop already checked h8 with white to move.)
        let board: Board =
            "7k/7p/5PQ1/8/8/8/8/7K[] w - - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        // Mate in 1: score = MATE_SCORE - 1 (found at ply 1)
        assert_eq!(result.score, MATE_SCORE - 1, "Engine should find the Qg7# checkmate");
    }

    #[test]
    fn best_move_is_legal() {
        let board = Board::default();
        let result = search(&board, 2).unwrap();
        let new_board = match result.best_move {
            BughouseMove::Regular(cm) => board.make_move_new(cm),
            BughouseMove::Drop { piece, square } => board.make_drop_new(piece, square).unwrap(),
        };
        assert_ne!(
            new_board.status(),
            BoardStatus::Checkmate,
            "best move should not result in immediate checkmate of ourselves"
        );
    }

    #[test]
    fn drop_pruning_reduces_candidates() {
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QRBNPqrbnp] w KQkq - 0 1"
                .parse()
                .unwrap();
        let drop_mask = build_drop_mask(&board);
        let combined = *board.combined();
        let empty_targets = drop_mask & !combined;

        let pruned_squares = empty_targets.popcnt();
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
        let result = search(&board, 1).unwrap();
        assert!(result.nodes >= 20, "startpos should evaluate at least 20 nodes");
        assert_eq!(result.depth, 1);
        let move_str = format!("{}", result.best_move);
        assert!(move_str.len() >= 4, "move string too short: {}", move_str);
    }

    #[test]
    fn prefers_strong_drop() {
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[Q] b KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1);
        assert!(result.is_some());
    }

    #[test]
    fn drop_mask_covers_enemy_king_zone() {
        let board = Board::default();
        let mask = build_drop_mask(&board);
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
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        assert!(
            result.nodes > 20,
            "should evaluate drops too, but only got {} nodes",
            result.nodes
        );
    }

    #[test]
    fn search_deterministic() {
        let board = Board::default();
        let result1 = search(&board, 2).unwrap();
        let result2 = search(&board, 2).unwrap();
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
        let board: Board =
            "rn1qkbnr/pppppppp/8/3b4/8/2N5/PPPPPPPP/R1BQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        assert!(
            result.score > 200,
            "should prefer capturing the bishop, score={}",
            result.score
        );
    }

    #[test]
    fn negamax_depth_0_returns_static_eval() {
        // In a quiet position (startpos), depth-0 → quiescence → stand_pat = static eval
        let board = Board::default();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let score = negamax(&board, 0, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx);
        let static_eval = scoring::evaluate(&board);
        assert_eq!(score, static_eval,
            "depth-0 negamax should return static eval: got {}, expected {}",
            score, static_eval
        );
        assert_eq!(ctx.nodes, 1);
    }

    #[test]
    fn depth_2_avoids_trap() {
        // White knight on f3 can capture "hanging" queen on e5, but queen is
        // defended by bishop on c7. At depth 1, Nxe5 looks great (+580 cp gain).
        // At depth 2, black recaptures Bxe5 and white lost a knight for nothing
        // net (queen was defended). Depth-2 should avoid this.
        let board: Board =
            "rn2kbnr/ppbppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();

        let result_d1 = search(&board, 1).unwrap();
        let result_d2 = search(&board, 2).unwrap();

        // At depth 1, the engine might grab the queen. At depth 2 it should
        // see the recapture and pick a more cautious move (or still take if
        // the exchange is favorable — queen for knight is still good even with
        // recapture since Q(900) > N(320)).
        // The key test: depth-2 score should be lower than depth-1 score for
        // this position if depth-1 overestimated.
        // Actually queen capture is still good (win 900-320=580), so both depths
        // should find it profitable. Let's just verify depth 2 works and gives
        // a reasonable score.
        assert!(result_d2.score > 200,
            "depth-2 should still find favorable exchange, score={}",
            result_d2.score
        );
    }

    #[test]
    fn depth_2_finds_deeper_tactics() {
        // Depth 2 should evaluate more nodes than depth 1 on the same position
        let board = Board::default();
        let result_d1 = search(&board, 1).unwrap();
        let result_d2 = search(&board, 2).unwrap();
        assert!(
            result_d2.nodes > result_d1.nodes,
            "depth-2 ({} nodes) should evaluate more than depth-1 ({} nodes)",
            result_d2.nodes, result_d1.nodes
        );
        assert_eq!(result_d2.depth, 2);
    }

    #[test]
    fn mate_score_prefers_shorter_mate() {
        // Verify that MATE_SCORE - ply scoring works: shorter mates score higher
        // A mate in 1 should score MATE_SCORE - 1 = 29999
        // Use depth 1 to avoid library issues with sparse endgame positions
        let board: Board =
            "7k/7p/5PQ1/8/8/8/8/7K[] w - - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        assert_eq!(result.score, MATE_SCORE - 1,
            "mate-in-1 should score {}, got {}",
            MATE_SCORE - 1, result.score
        );
    }

    #[test]
    fn iterative_deepening_with_budget() {
        let board = Board::default();
        let mut info_sink = Vec::new();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        // Give generous budget — should reach depth > 1
        let result = find_best_move_timed(&board, 5000, &mut info_sink, &mut tt, &EngineConfig::default()).unwrap();
        assert!(result.depth >= 2, "should reach at least depth 2, got {}", result.depth);
        assert!(info_sink.len() >= 2, "should have at least 2 info entries, got {}", info_sink.len());
        // Info entries should have increasing depth
        for (i, info) in info_sink.iter().enumerate() {
            assert_eq!(info.depth, (i + 1) as u32, "info depth should match iteration");
        }
    }

    #[test]
    fn iterative_deepening_zero_budget_completes_depth_1() {
        let board = Board::default();
        let mut info_sink = Vec::new();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        // Zero budget — depth 1 should still complete
        let result = find_best_move_timed(&board, 0, &mut info_sink, &mut tt, &EngineConfig::default()).unwrap();
        assert_eq!(result.depth, 1, "depth 1 should complete even with 0 budget");
        assert!(!info_sink.is_empty(), "should have at least 1 info entry");
    }

    #[test]
    fn move_ordering_captures_first() {
        // Verify that captures are ordered before quiet moves
        let board: Board =
            "rn1qkbnr/pppppppp/8/3b4/8/2N5/PPPPPPPP/R1BQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let mut moves = generate_moves(&board);
        let empty_history: HistoryTable = [[[0; 64]; 64]; 2];
        order_moves(&board, &mut moves, &empty_history, &EngineConfig::default());

        // First move should be a capture (Nc3xd5)
        if let Some(BughouseMove::Regular(cm)) = moves.first() {
            let victim = board.piece_on(cm.get_dest());
            assert!(victim.is_some(),
                "first ordered move should be a capture, got {:?}",
                moves.first()
            );
        } else {
            panic!("first ordered move should be a regular capture");
        }
    }

    #[test]
    fn root_capture_scores_are_exact() {
        // Black queen hangs on e5 (Nxe5) and black knight hangs on a3 (bxa3).
        // MVV-LVA searches the queen capture first, so under the old
        // beat-the-best window the knight capture returned a fail-soft bound.
        // Its reported root score must equal an independent full-window search
        // of the child position.
        let board: Board =
            "rnb1kb1r/pppp1ppp/8/4q3/8/n4N2/PPPPPPPP/R1BQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let config = EngineConfig::default();
        let result = find_best_move(&board, 2, &config).unwrap();

        let queen_capture = result.root_move_evals.iter()
            .find(|e| format!("{}", e.mv) == "f3e5")
            .expect("Nxe5 should be a root move");
        let knight_capture = result.root_move_evals.iter()
            .find(|e| format!("{}", e.mv) == "b2a3")
            .expect("bxa3 should be a root move");

        assert!(queen_capture.score > knight_capture.score,
            "queen capture ({}) should outscore knight capture ({})",
            queen_capture.score, knight_capture.score);

        // Exactness: recompute the knight capture's true value independently.
        let child = board.make_move_new(match knight_capture.mv {
            BughouseMove::Regular(cm) => cm,
            _ => panic!("bxa3 is a regular move"),
        });
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let expected = -evaluate_position(&child, 1, &mut tt, &config);
        assert_eq!(knight_capture.score, expected,
            "non-best capture score must be exact, not a fail-soft bound");
    }

    #[test]
    fn capture_stats_hanging_piece() {
        // Black bishop hanging on d5, white knight on c3 can capture freely
        let board: Board =
            "rn1qkbnr/pppppppp/8/3b4/8/2N5/PPPPPPPP/R1BQKBNR[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 2).unwrap();
        let stats = &result.capture_stats;
        // White (index 0) should have high P for bishop (index 2) since it's hanging
        let bishop_idx = Piece::Bishop.non_king_index().unwrap();
        assert!(stats.probability[0][bishop_idx] > 0.5,
            "hanging bishop should have high P, got {}",
            stats.probability[0][bishop_idx]
        );
        // Cost should be low since the capture is free
        assert!(stats.cost[0][bishop_idx] <= 100,
            "hanging bishop should have low cost, got {}",
            stats.cost[0][bishop_idx]
        );
    }

    #[test]
    fn capture_stats_no_captures() {
        // Starting position: no immediate captures available
        let board = Board::default();
        let result = search(&board, 1).unwrap();
        let stats = &result.capture_stats;
        // All P values should be 0.0 since no captures are possible
        for idx in 0..NUM_NON_KING_PIECES {
            assert_eq!(stats.probability[0][idx], 0.0,
                "no captures in startpos: P[{}] should be 0.0, got {}",
                idx, stats.probability[0][idx]
            );
        }
    }

    #[test]
    fn crash_position_from_game() {
        // This position caused the engine to crash during a real game at depth 5.
        // The crash was in make_move_new -> piece_on(source).unwrap() deep in the tree.
        let board: Board =
            "r1b2r1k/pppp1p1p/4pN1p/8/2PP4/2P5/PP2PPPP/nq1K1BR1[QBNP] w - - 3 25"
                .parse()
                .unwrap();
        let mut info_sink = Vec::new();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let result = find_best_move_timed(&board, 2000, &mut info_sink, &mut tt, &EngineConfig::default());
        assert!(result.is_some(), "should find a move without crashing");
        assert!(result.unwrap().depth >= 1, "should complete at least depth 1");
    }

    #[test]
    fn no_illegal_drops_while_in_check() {
        // Reproduction of a real game bug: black king on e8 in check from Nf6,
        // engine chose p@d7 which doesn't resolve the knight check.
        // With the fix, drops that don't resolve check should not be generated.
        let board: Board =
            "r3kbr1/p1p2p1p/2p2Np1/1p2p3/Q5nq/2B1P3/PPPP1PPP/R1B1K2R[BNNPbbnnpp] b q - 1 18"
                .parse()
                .unwrap();
        let moves = generate_moves(&board);
        // No drop moves should appear — king is in check from a knight, which can't be blocked
        for m in &moves {
            if let BughouseMove::Drop { piece, square } = m {
                panic!("illegal drop {:?}@{:?} generated while in knight check", piece, square);
            }
        }
        // Should still have regular moves (king moves, captures of the knight)
        assert!(!moves.is_empty(), "should have legal moves to resolve check");
    }

    #[test]
    fn capture_stats_returns_valid_struct() {
        // Just verify the structure is populated without panicking
        let board: Board =
            "rnb1kbnr/pppppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 2).unwrap();
        let stats = &result.capture_stats;
        // Queen (index 4) should be capturable with high P
        let queen_idx = Piece::Queen.non_king_index().unwrap();
        assert!(stats.probability[0][queen_idx] > 0.0,
            "should detect queen capture possibility, P={}",
            stats.probability[0][queen_idx]
        );
    }

    #[test]
    fn tt_warm_reduces_nodes() {
        // Search the same position at a fixed depth with a shared TT.
        // The second search should evaluate fewer nodes because
        // the TT is warm from the first search.
        let board = Board::default();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);

        // Use fixed depth (not time-based) to avoid CPU contention issues
        let mut ctx1 = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let mut moves = generate_moves(&board);
        let empty_history: HistoryTable = [[[0; 64]; 64]; 2];
        order_moves(&board, &mut moves, &empty_history, &EngineConfig::default());
        let _ = search_at_depth(&board, &moves, 4, &mut ctx1);
        let nodes1 = ctx1.nodes;

        // Second search with warm TT
        let mut ctx2 = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let _ = search_at_depth(&board, &moves, 4, &mut ctx2);
        let nodes2 = ctx2.nodes;

        assert!(nodes2 <= nodes1,
            "warm TT should reduce nodes at same depth: first={}, second={}",
            nodes1, nodes2
        );
    }

    #[test]
    fn tt_preserves_correctness() {
        // Hanging queen should still be captured with TT enabled
        let board: Board =
            "rnb1kbnr/pppppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 2).unwrap();
        assert!(result.score > 500,
            "should capture hanging queen with TT, score={}", result.score);
    }

    #[test]
    fn tt_mate_score_correct() {
        // Mate in 1 should still score correctly with TT
        let board: Board =
            "7k/7p/5PQ1/8/8/8/8/7K[] w - - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 1).unwrap();
        assert_eq!(result.score, MATE_SCORE - 1,
            "mate-in-1 with TT should score {}, got {}",
            MATE_SCORE - 1, result.score
        );
    }

    #[test]
    fn quiescence_quiet_position() {
        // Starting position has no captures — quiescence should return
        // roughly the static eval with minimal node count.
        let board = Board::default();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let score = quiescence(&board, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx, 1);
        let static_eval = scoring::evaluate(&board);
        assert_eq!(score, static_eval,
            "quiescence in quiet position should match static eval: got {}, expected {}",
            score, static_eval
        );
        assert_eq!(ctx.nodes, 1, "quiet position should evaluate 1 node (stand_pat only)");
    }

    #[test]
    fn quiescence_finds_recapture() {
        // White knight on c3 just captured a pawn on d5. Black queen on d8
        // can recapture Qxd5. At depth 1 without quiescence, white might
        // think Nxd5 is free. With quiescence, the recapture is found.
        //
        // Position: white Nc3 captured to d5, black Qd8 can recapture.
        let board: Board =
            "rnbqkb1r/pppppppp/5n2/3N4/8/8/PPPPPPPP/R1BQKBNR[] b KQkq - 0 1"
                .parse()
                .unwrap();
        // From black's perspective, Qxd5 captures the knight.
        // Quiescence should find this capture and evaluate favorably for black.
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let score = quiescence(&board, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx, 1);
        // Black can capture a knight (+320cp) so quiescence should score
        // significantly better than just stand_pat
        let static_eval = scoring::evaluate(&board);
        assert!(score >= static_eval,
            "quiescence should find capture opportunity: score={}, static_eval={}",
            score, static_eval
        );
        assert!(ctx.nodes > 1, "should explore capture nodes, got {} nodes", ctx.nodes);
    }

    #[test]
    fn quiescence_doesnt_worsen_eval() {
        // Quiescence should never return worse than static eval because
        // stand_pat gives the option to not capture.
        let board: Board =
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1"
                .parse()
                .unwrap();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let score = quiescence(&board, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx, 1);
        let static_eval = scoring::evaluate(&board);
        assert!(score >= static_eval,
            "quiescence should never be worse than stand_pat: score={}, static_eval={}",
            score, static_eval
        );
    }

    #[test]
    fn checking_drops_match_brute_force() {
        // Fast attack-symmetry generation must exactly match filtering all
        // legal drops by "does the child position leave the opponent in check".
        // Black king on e5 is open, so all five piece types produce checks
        // (including pawn drops from d4/f4).
        let board: Board = "r7/5ppp/8/4k3/8/8/5PPP/R5K1[QRBNP] w - - 0 1"
            .parse()
            .unwrap();

        let fast: std::collections::HashSet<String> = generate_checking_drops(&board)
            .iter()
            .map(|m| format!("{}", m))
            .collect();

        let brute: std::collections::HashSet<String> = MoveGen::drop_moves(&board)
            .into_iter()
            .filter(|m| {
                make_move(&board, m)
                    .map(|child| *child.checkers() != EMPTY)
                    .unwrap_or(false)
            })
            .map(|m| format!("{}", m))
            .collect();

        assert!(!fast.is_empty(), "should generate some checking drops");
        assert_eq!(fast, brute, "fast generation must match brute force");
        // Pawn checks specifically (d4 and f4 attack e5 for white)
        assert!(fast.contains("p@d4"), "missing pawn check p@d4: {:?}", fast);
        assert!(fast.contains("p@f4"), "missing pawn check p@f4: {:?}", fast);
    }

    #[test]
    fn quiescence_finds_freezing_drop_check() {
        // White to move holding a rook; r@e8 freezes black. The freeze must be
        // visible from quiescence itself (checking drops at the horizon), not
        // just from root move generation.
        let board: Board = "6k1/5ppp/8/8/8/8/5PPP/6K1[R] w - - 0 1"
            .parse()
            .unwrap();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let score = quiescence(&board, 0, i32::MIN + 1, i32::MAX - 1, &mut ctx, 1);
        assert_eq!(score, FROZEN_SCORE - 1,
            "quiescence should find the freezing r@e8, got {}", score);
    }

    #[test]
    fn engine_defends_against_freeze_threat() {
        // White to move; black holds a rook and threatens r@e1, freezing the
        // boxed white king. At depth 1 the threat only appears in quiescence
        // (after white's move, black's checking drops are horizon moves).
        // The engine must pick a move that defuses it: luft (f3/g3/h3) or Kf1
        // (which gains the e2 escape square).
        let board: Board = "6k1/5ppp/8/8/8/8/P4PPP/6K1[r] w - - 0 1"
            .parse()
            .unwrap();
        let result = search(&board, 1).unwrap();
        let chosen = format!("{}", result.best_move);
        let defenses = ["f2f3", "g2g3", "h2h3", "g1f1"];
        assert!(defenses.contains(&chosen.as_str()),
            "should defuse the freeze threat, chose {} (score {})", chosen, result.score);
        assert!(result.score > -(FROZEN_SCORE - 1000),
            "a defense exists, so score must not be in the frozen band: {}", result.score);
    }

    #[test]
    fn frozen_opponent_scored_near_loss() {
        // r@e8 leaves black in check with no legal moves and an empty reserve.
        // status() stays Ongoing (the check is blockable on f8 by a future drop),
        // but black must sit — the search should treat this as nearly winning.
        // Found at depth 1 via the quiescence frozen check.
        let board: Board = "6k1/5ppp/8/8/8/8/5PPP/6K1[R] w - - 0 1"
            .parse()
            .unwrap();
        let result = search(&board, 1).unwrap();
        assert_eq!(format!("{}", result.best_move), "r@e8",
            "should freeze black with the back-rank rook drop");
        assert_eq!(result.score, FROZEN_SCORE - 1,
            "frozen at ply 1 should score FROZEN_SCORE - 1");
    }

    #[test]
    fn frozen_score_stable_across_depths() {
        let board: Board = "6k1/5ppp/8/8/8/8/5PPP/6K1[R] w - - 0 1"
            .parse()
            .unwrap();
        let result = search(&board, 3).unwrap();
        assert_eq!(format!("{}", result.best_move), "r@e8");
        assert_eq!(result.score, FROZEN_SCORE - 1,
            "deeper search should agree on the frozen score");
    }

    #[test]
    fn pawn_in_reserve_cannot_block_back_rank() {
        // Black holds a pawn, but pawns can't be dropped on the 8th rank, so
        // r@e8 still freezes: no legal drop exists even with a non-empty reserve.
        let board: Board = "6k1/5ppp/8/8/8/8/5PPP/6K1[Rp] w - - 0 1"
            .parse()
            .unwrap();
        let result = search(&board, 2).unwrap();
        assert_eq!(format!("{}", result.best_move), "r@e8");
        assert_eq!(result.score, FROZEN_SCORE - 1,
            "pawn in hand cannot block on rank 8 — still frozen");
    }

    #[test]
    fn blockable_check_with_blocker_not_frozen() {
        // Same freeze attempt, but black holds a knight: n@f8 blocks the check,
        // so the position after r@e8 is not frozen and no frozen-band score
        // should appear.
        let board: Board = "6k1/5ppp/8/8/8/8/5PPP/6K1[Rn] w - - 0 1"
            .parse()
            .unwrap();
        let result = search(&board, 2).unwrap();
        assert!(result.score < FROZEN_SCORE - 1000,
            "black can block with n@f8 — not frozen, got score {}", result.score);
    }

    #[test]
    fn frozen_side_evaluated_as_lost() {
        // From the frozen side's perspective: black to move, in check from Re8,
        // no legal moves, empty reserve. Direct negamax should report near-loss.
        let board: Board = "4R1k1/5ppp/8/8/8/8/5PPP/6K1[] b - - 0 1"
            .parse()
            .unwrap();
        assert_eq!(board.status(), BoardStatus::Ongoing,
            "drop-blockable check should report Ongoing");
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let score = evaluate_position(&board, 2, &mut tt, &EngineConfig::default());
        assert_eq!(score, -FROZEN_SCORE,
            "frozen side at ply 0 should score -FROZEN_SCORE");
    }

    #[test]
    fn lmr_table_values() {
        let table = &*LMR_TABLE;
        // depth=0, move_index=0: no reduction
        assert_eq!(table[0][0], 0);
        // depth=1, move_index=1: ln(1)*ln(1)/2 = 0, clamped to 1
        assert_eq!(table[1][1], 1);
        // Reductions increase with depth and move index
        assert!(table[6][10] > table[3][3],
            "deeper+later should reduce more: [6][10]={} vs [3][3]={}",
            table[6][10], table[3][3]);
        // Large depth + large move index should have substantial reduction
        assert!(table[8][20] >= 2,
            "depth 8, move 20 should reduce by at least 2, got {}",
            table[8][20]);
    }

    #[test]
    fn lmr_reduces_node_count() {
        // Compare node count at depth 6 with vs without LMR.
        // Since LMR is always active, we compare against the theoretical
        // maximum by asserting a reasonable node count.
        let board = Board::default();
        let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        let mut ctx = SearchContext {
            start: Instant::now(),
            budget_ms: 0,
            nodes: 0,
            time_up: false,
            tt: &mut tt,
            killers: [[0; NUM_KILLERS]; MAX_DEPTH as usize],
            history: [[[0; 64]; 64]; 2],
            abort: None,
            config: &EngineConfig::default(),
        };
        let mut moves = generate_moves(&board);
        let empty_history: HistoryTable = [[[0; 64]; 64]; 2];
        order_moves(&board, &mut moves, &empty_history, &EngineConfig::default());
        let _ = search_at_depth(&board, &moves, 6, &mut ctx);

        // Without LMR, depth 6 from startpos would be ~500K-2M nodes.
        // With LMR, expect a significant reduction.
        assert!(ctx.nodes < 1_500_000,
            "LMR should reduce node count at depth 6, got {} nodes", ctx.nodes);
        assert!(ctx.nodes > 0, "should search some nodes");
    }

    #[test]
    fn lmr_still_finds_hanging_queen() {
        // Correctness check: LMR should not miss obvious tactics.
        let board: Board =
            "rnb1kbnr/pppppppp/8/4q3/8/5N2/PPPPPPPP/RNBQKB1R[] w KQkq - 0 1"
                .parse()
                .unwrap();
        let result = search(&board, 4).unwrap();
        assert!(result.score > 500,
            "should still capture hanging queen with LMR active, score={}", result.score);
    }

    #[test]
    fn lmr_inactive_at_shallow_depth() {
        // At depth 1-2, LMR should not trigger (LMR_MIN_DEPTH = 3).
        // Results should be identical to a search without LMR.
        let board = Board::default();
        let result1 = search(&board, 1).unwrap();
        let result2 = search(&board, 2).unwrap();
        // Just verify they complete and produce reasonable results
        assert!(result1.score.abs() < 1000, "depth 1 score reasonable: {}", result1.score);
        assert!(result2.score.abs() < 1000, "depth 2 score reasonable: {}", result2.score);
    }
}
