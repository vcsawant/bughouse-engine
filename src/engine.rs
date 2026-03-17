//! Multi-threaded evaluation engine with pondering.
//!
//! Two eval threads (one per board) continuously search via iterative
//! deepening. Each owns its own TT. The main thread communicates via
//! channels (commands) and shared status (Arc<Mutex>).

use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use bughouse_chess::{
    Board, BughouseMove, CacheTable, Color, Piece,
    NON_KING_PIECES, NUM_NON_KING_PIECES,
};
use log::{debug, info, warn};

use crate::scoring;
use crate::search::{
    self, BoardEval, CaptureStats, RootMoveEvalPub, SearchInfo,
    TTEntry, TT_DEFAULT, TT_DEFAULT_SIZE,
};

// ─── Thread Communication ───────────────────────────────────────────

/// Commands from the main thread to an eval thread.
pub enum EvalCommand {
    /// Restart search with a new board position.
    NewPosition(Board),
    /// Pause after completing the current depth (for Stop command).
    Pause,
    /// Resume pondering after a pause.
    Resume,
    /// Shut down the eval thread.
    Quit,
}

/// Information about a root move, produced by the eval thread.
#[derive(Debug, Clone)]
pub struct RootMoveInfo {
    pub mv: BughouseMove,
    pub score: i32,
    pub captured: Option<Piece>,
}

// ─── Cross-Board Analysis ───────────────────────────────────────────

/// A root move annotated with cross-board impact.
#[derive(Debug, Clone)]
pub struct AnnotatedMove {
    pub mv: BughouseMove,
    pub local_score: i32,
    pub captured: Option<Piece>,
    /// Raw, unweighted benefit to the OTHER board from this capture (centipawns).
    /// 0 if the move is not a capture or the captured piece has no cross-board value.
    pub cross_board_value: i32,
}

/// Cross-board ranking for one board, produced by search-level analysis.
#[derive(Debug, Clone)]
pub struct CrossBoardRanking {
    pub board_hash: u64,
    pub moves: Vec<AnnotatedMove>,
    /// What pieces the OTHER board needs (reserve_impact from other board's eval).
    pub other_board_reserve_impact: [i32; NUM_NON_KING_PIECES],
    /// Depth of the other board's eval (for confidence assessment).
    pub other_board_depth: u32,
}

/// Compute cross-board ranking for one board's root moves.
///
/// For each root move, annotates with how much the captured piece (if any)
/// would benefit the other board, based on the other board's reserve_impact.
/// No weights applied — that's the go handler's job.
pub fn compute_cross_board_ranking(
    our_eval: &EvalStatus,
    other_reserve_impact: &[i32; NUM_NON_KING_PIECES],
    other_depth: u32,
) -> CrossBoardRanking {
    let moves = our_eval.root_moves.iter().map(|rm| {
        let cross_board_value = match rm.captured {
            Some(piece) => {
                let idx = piece.to_index();
                if idx < NUM_NON_KING_PIECES {
                    other_reserve_impact[idx]
                } else {
                    0 // King capture — shouldn't happen but be safe
                }
            }
            None => 0,
        };
        AnnotatedMove {
            mv: rm.mv.clone(),
            local_score: rm.score,
            captured: rm.captured,
            cross_board_value,
        }
    }).collect();

    CrossBoardRanking {
        board_hash: our_eval.board_hash,
        moves,
        other_board_reserve_impact: *other_reserve_impact,
        other_board_depth: other_depth,
    }
}

// ─── Eval Thread Communication ──────────────────────────────────────

/// Status published by the eval thread, readable by the main/search thread.
#[derive(Debug, Clone)]
pub struct EvalStatus {
    /// Zobrist hash of the position being evaluated.
    pub board_hash: u64,
    /// Best move found so far.
    pub best_move: Option<BughouseMove>,
    /// Score of the best move.
    pub best_score: i32,
    /// Last fully completed search depth.
    pub completed_depth: u32,
    /// Per-board evaluation data (P/C, reserve_impact, score, depth).
    pub eval: BoardEval,
    /// Scored root moves from the last completed depth.
    pub root_moves: Vec<RootMoveInfo>,
    /// Accumulated info lines from all completed depths.
    pub info_lines: Vec<SearchInfo>,
    /// Whether the eval thread is actively searching.
    pub searching: bool,
}

impl Default for EvalStatus {
    fn default() -> Self {
        EvalStatus {
            board_hash: 0,
            best_move: None,
            best_score: 0,
            completed_depth: 0,
            eval: BoardEval::default(),
            root_moves: Vec::new(),
            info_lines: Vec::new(),
            searching: false,
        }
    }
}

/// Shared eval status with condvar for signaling status changes.
pub struct SharedEvalStatus {
    pub status: Mutex<EvalStatus>,
    /// Notified when eval thread completes a depth, pauses, or changes state.
    pub status_changed: Condvar,
}

impl SharedEvalStatus {
    pub fn new() -> Self {
        SharedEvalStatus {
            status: Mutex::new(EvalStatus::default()),
            status_changed: Condvar::new(),
        }
    }

    /// Wait until the eval thread is no longer searching (paused or idle).
    pub fn wait_for_pause(&self) {
        let mut status = self.status.lock().unwrap();
        while status.searching {
            status = self.status_changed.wait(status).unwrap();
        }
    }

    /// Wait until the eval thread has completed at least `min_depth` on the
    /// expected position (matched by `expected_hash`), or until `timeout` expires.
    /// Returns a snapshot of the status.
    ///
    /// If the eval thread's board_hash doesn't match `expected_hash`, waits for
    /// it to restart and reach min_depth on the correct position.
    pub fn wait_for_depth_or_timeout(&self, expected_hash: u64, min_depth: u32, timeout: std::time::Duration) -> EvalStatus {
        let deadline = Instant::now() + timeout;
        let mut status = self.status.lock().unwrap();
        loop {
            // Check if we have results for the correct position at sufficient depth
            let correct_position = status.board_hash == expected_hash;
            let sufficient_depth = status.completed_depth >= min_depth;
            if correct_position && sufficient_depth {
                break;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (s, _) = self.status_changed.wait_timeout(status, remaining).unwrap();
            status = s;
        }
        status.clone()
    }
}

/// Handle for communicating with an eval thread.
pub struct EvalHandle {
    pub cmd_tx: mpsc::Sender<EvalCommand>,
    pub shared: Arc<SharedEvalStatus>,
    pub thread: Option<JoinHandle<()>>,
}

impl EvalHandle {
    /// Send a command to the eval thread.
    pub fn send(&self, cmd: EvalCommand) {
        self.cmd_tx.send(cmd).ok();
    }

    /// Read the current eval status (locks briefly).
    pub fn status(&self) -> EvalStatus {
        self.shared.status.lock().unwrap().clone()
    }

    /// Wait for the eval thread to pause.
    pub fn wait_for_pause(&self) {
        self.shared.wait_for_pause();
    }
}

// ─── Eval Thread Loop ───────────────────────────────────────────────

/// Maximum search depth (matches search.rs constant).
const MAX_DEPTH: u32 = 64;

/// Run the eval thread loop. Continuously searches the given board
/// via iterative deepening, responding to commands from the main thread.
pub fn eval_thread_loop(
    cmd_rx: mpsc::Receiver<EvalCommand>,
    shared: Arc<SharedEvalStatus>,
) {
    let mut tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
    let mut board: Option<Board> = None;
    let mut paused = true; // start paused until we get a position
    let abort_flag = AtomicBool::new(false);

    // Mark as not searching initially
    {
        let mut status = shared.status.lock().unwrap();
        status.searching = false;
        shared.status_changed.notify_all();
    }

    loop {
        // If paused or no board, block-wait for a command
        if paused || board.is_none() {
            match cmd_rx.recv() {
                Ok(cmd) => {
                    match cmd {
                        EvalCommand::NewPosition(b) => {
                            board = Some(b);
                            paused = false;
                            // Reset state for new position
                            let mut status = shared.status.lock().unwrap();
                            status.board_hash = b.get_hash();
                            status.best_move = None;
                            status.best_score = 0;
                            status.completed_depth = 0;
                            status.eval = BoardEval::default();
                            status.root_moves.clear();
                            status.info_lines.clear();
                            status.searching = true;
                            debug!("Eval thread: new position, hash={:#x}", status.board_hash);
                        }
                        EvalCommand::Resume => {
                            if board.is_some() {
                                paused = false;
                                let mut status = shared.status.lock().unwrap();
                                status.searching = true;
                            }
                        }
                        EvalCommand::Quit => {
                            debug!("Eval thread: quit");
                            return;
                        }
                        EvalCommand::Pause => {
                            // Already paused, ignore
                        }
                    }
                }
                Err(_) => return, // channel closed
            }
            continue;
        }

        let b = board.unwrap();
        let side = b.side_to_move();

        // Generate and order moves once for the entire iterative deepening
        let mut moves = search::generate_moves_pub(&b);
        if moves.is_empty() {
            // No legal moves — update status and pause
            let mut status = shared.status.lock().unwrap();
            status.eval.score = scoring::evaluate(&b);
            status.searching = false;
            paused = true;
            shared.status_changed.notify_all();
            continue;
        }

        // Iterative deepening loop — runs continuously until NewPosition, Pause, or Quit
        let start = Instant::now();

        for depth in 1..=MAX_DEPTH {
            // Check for commands between depths
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    EvalCommand::Pause => {
                        paused = true;
                        abort_flag.store(true, Ordering::Relaxed);
                        let mut status = shared.status.lock().unwrap();
                        status.searching = false;
                        shared.status_changed.notify_all();
                    }
                    EvalCommand::NewPosition(new_b) => {
                        board = Some(new_b);
                        abort_flag.store(true, Ordering::Relaxed);
                        let mut status = shared.status.lock().unwrap();
                        status.board_hash = new_b.get_hash();
                        status.best_move = None;
                        status.best_score = 0;
                        status.completed_depth = 0;
                        status.eval = BoardEval::default();
                        status.root_moves.clear();
                        status.info_lines.clear();
                        status.searching = true;
                        shared.status_changed.notify_all();
                    }
                    EvalCommand::Quit => return,
                    EvalCommand::Resume => {
                        paused = false;
                        let mut status = shared.status.lock().unwrap();
                        status.searching = true;
                    }
                }
            }

            if paused {
                break;
            }

            // Check if position changed (new board was set during command processing)
            if board.unwrap().get_hash() != b.get_hash() {
                break; // restart outer loop with new board
            }

            // Reset abort flag before searching this depth
            abort_flag.store(false, Ordering::Relaxed);

            // Search at this depth (abort flag allows mid-depth interruption)
            let search_result = search::search_at_depth_pub(
                &b, &moves, depth, &mut tt, Some(&abort_flag),
            );

            if let Some((best_move, score, pv, root_evals, nodes)) = search_result {
                let elapsed = start.elapsed().as_millis() as u64;
                let capture_stats = search::compute_capture_stats_pub(score, &root_evals, side);

                // Reserve impact is NOT computed in the eval thread — it's too expensive
                // (5 shallow searches that block deepening). The go handler computes it
                // on-demand if needed.
                let reserve_impact = [0; NUM_NON_KING_PIECES];

                // Build root move info with actual move data
                let root_move_infos: Vec<RootMoveInfo> = root_evals.iter().map(|eval| {
                    RootMoveInfo {
                        mv: eval.mv.clone(),
                        score: eval.score,
                        captured: eval.captured,
                    }
                }).collect();

                // Update shared status and notify waiters (e.g., main thread in wait_for_depth_or_timeout)
                {
                    let mut status = shared.status.lock().unwrap();
                    status.best_move = Some(best_move.clone());
                    status.best_score = score;
                    status.completed_depth = depth;
                    status.eval = BoardEval {
                        capture_stats,
                        reserve_impact,
                        score,
                        depth,
                    };
                    status.root_moves = root_move_infos;
                    status.info_lines.push(SearchInfo {
                        depth,
                        score,
                        nodes,
                        time_ms: elapsed,
                        pv,
                    });
                }
                // Notify after releasing the lock
                shared.status_changed.notify_all();

                // Re-order moves: put best move first for next iteration
                if let Some(pos) = moves.iter().position(|m| *m == best_move) {
                    moves.swap(0, pos);
                }
            } else {
                break; // search failed (aborted or no moves)
            }
        }

        // If we exited the loop, check why:
        // - If board changed (NewPosition mid-search), continue the outer loop
        //   to restart with the new board. Do NOT set paused.
        // - If paused (Pause command or MAX_DEPTH), wait for next command.
        if !paused && board.map(|b| b.get_hash()) != Some(b.get_hash()) {
            // Board changed mid-search — restart immediately with new board
            debug!("Eval thread: position changed mid-search, restarting");
            continue;
        }
        // Otherwise, we're done (MAX_DEPTH, search failed, or paused)
        {
            let mut status = shared.status.lock().unwrap();
            status.searching = false;
            shared.status_changed.notify_all();
        }
        paused = true;
    }
}

/// Compute reserve impact using targeted drop-only search.
///
/// For each piece type not in reserves, generates only drop moves for that piece,
/// evaluates each drop position, and compares to the eval thread's best score.
/// Uses the eval thread's best_score as the alpha floor — any drop that doesn't
/// beat the current best move is pruned immediately.
///
/// Called on the main thread during handle_go. Does NOT block the eval thread.
pub fn compute_reserve_impact_fast(
    board: &Board,
    best_score: i32,
    search_depth: u32,
) -> [i32; NUM_NON_KING_PIECES] {
    use bughouse_chess::MoveGen;

    let color = board.side_to_move();
    let reserves = &board.reserves()[color.to_index()];
    let mut impact = [0i32; NUM_NON_KING_PIECES];
    let mut tt = CacheTable::new(1 << 16, TT_DEFAULT); // small 512KB TT for drop search

    for &piece in &NON_KING_PIECES {
        // Skip pieces we already have in reserves
        if reserves.count(piece) > 0 {
            continue;
        }

        // Create hypothetical board with this piece added to reserves
        let mut hypothetical = *board;
        hypothetical.add_to_reserve(color, piece);

        // Generate only drop moves for this piece type
        let drop_moves: Vec<BughouseMove> = MoveGen::drop_moves(&hypothetical)
            .into_iter()
            .filter(|m| matches!(m, BughouseMove::Drop { piece: p, .. } if *p == piece))
            .collect();

        if drop_moves.is_empty() {
            continue;
        }

        // Evaluate each drop: make the drop, then search the resulting position.
        // Use best_score as alpha floor — only drops better than current best matter.
        let mut best_drop_score = i32::MIN + 1;
        for drop_move in &drop_moves {
            if let Some(child) = match drop_move {
                BughouseMove::Drop { piece, square } => hypothetical.make_drop_new(*piece, *square),
                _ => None,
            } {
                // Search from opponent's perspective, negate back
                let score = -search::evaluate_position(&child, search_depth.saturating_sub(1), &mut tt);
                if score > best_drop_score {
                    best_drop_score = score;
                }
                // Alpha cutoff: if we already found a great drop, skip weaker ones
                if best_drop_score > best_score + 500 {
                    break; // clearly impactful, no need to check all squares
                }
            }
        }

        if best_drop_score > i32::MIN + 1 {
            impact[piece.to_index()] = best_drop_score - best_score;
        }
    }

    impact
}

// ─── Engine (spawns and manages threads) ────────────────────────────

/// Spawn an eval thread, returning a handle for communication.
pub fn spawn_eval_thread() -> EvalHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let shared = Arc::new(SharedEvalStatus::new());
    let shared_clone = Arc::clone(&shared);

    let thread = thread::spawn(move || {
        eval_thread_loop(cmd_rx, shared_clone);
    });

    EvalHandle {
        cmd_tx,
        shared,
        thread: Some(thread),
    }
}
