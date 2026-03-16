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
    /// Finish current depth and pause by this deadline.
    SetDeadline(Instant),
    /// Pause after completing the current depth.
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
    other_eval: &EvalStatus,
) -> CrossBoardRanking {
    let other_impact = &other_eval.eval.reserve_impact;

    let moves = our_eval.root_moves.iter().map(|rm| {
        let cross_board_value = match rm.captured {
            Some(piece) => {
                let idx = piece.to_index();
                if idx < NUM_NON_KING_PIECES {
                    other_impact[idx]
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
        other_board_reserve_impact: *other_impact,
        other_board_depth: other_eval.completed_depth,
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

/// Shared eval status with condvar for signaling pause completion.
pub struct SharedEvalStatus {
    pub status: Mutex<EvalStatus>,
    pub paused_cond: Condvar,
}

impl SharedEvalStatus {
    pub fn new() -> Self {
        SharedEvalStatus {
            status: Mutex::new(EvalStatus::default()),
            paused_cond: Condvar::new(),
        }
    }

    /// Wait until the eval thread is no longer searching (paused or idle).
    pub fn wait_for_pause(&self) {
        let mut status = self.status.lock().unwrap();
        while status.searching {
            status = self.paused_cond.wait(status).unwrap();
        }
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
        shared.paused_cond.notify_all();
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
                        EvalCommand::Pause | EvalCommand::SetDeadline(_) => {
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
            shared.paused_cond.notify_all();
            continue;
        }

        // Iterative deepening loop
        let start = Instant::now();
        let mut deadline: Option<Instant> = None;

        // Check for pending deadline before starting
        if let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                EvalCommand::SetDeadline(t) => deadline = Some(t),
                EvalCommand::Pause => {
                    paused = true;
                    let mut status = shared.status.lock().unwrap();
                    status.searching = false;
                    shared.paused_cond.notify_all();
                    continue;
                }
                EvalCommand::NewPosition(new_b) => {
                    board = Some(new_b);
                    let mut status = shared.status.lock().unwrap();
                    status.board_hash = new_b.get_hash();
                    status.best_move = None;
                    status.best_score = 0;
                    status.completed_depth = 0;
                    status.eval = BoardEval::default();
                    status.root_moves.clear();
                    status.info_lines.clear();
                    status.searching = true;
                    continue;
                }
                EvalCommand::Quit => return,
                EvalCommand::Resume => {} // already running
            }
        }

        for depth in 1..=MAX_DEPTH {
            // Check for commands between depths
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    EvalCommand::SetDeadline(t) => deadline = Some(t),
                    EvalCommand::Pause => {
                        paused = true;
                        abort_flag.store(true, Ordering::Relaxed);
                        let mut status = shared.status.lock().unwrap();
                        status.searching = false;
                        shared.paused_cond.notify_all();
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

            // Check deadline
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    paused = true;
                    let mut status = shared.status.lock().unwrap();
                    status.searching = false;
                    shared.paused_cond.notify_all();
                    break;
                }
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

                // Compute reserve impact (only for pieces not in reserves)
                let reserve_impact = compute_reserve_impact_filtered(
                    &b, score, &mut tt,
                );

                // Build root move info from the search results
                let root_move_infos: Vec<RootMoveInfo> = root_evals.iter().map(|eval| {
                    // Find the move that produced this eval by matching captures
                    // For now, store score and captured info without the move
                    // (the move ordering may not match root_evals order)
                    RootMoveInfo {
                        mv: best_move.clone(), // placeholder — will be refined
                        score: eval.score,
                        captured: eval.captured,
                    }
                }).collect();

                // Update shared status
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

                // Re-order moves: put best move first for next iteration
                if let Some(pos) = moves.iter().position(|m| *m == best_move) {
                    moves.swap(0, pos);
                }
            } else {
                break; // search failed (time_up or no moves)
            }

            // Check deadline again after search
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    paused = true;
                    let mut status = shared.status.lock().unwrap();
                    status.searching = false;
                    shared.paused_cond.notify_all();
                    break;
                }
            }
        }

        // If we exited the loop without pausing (hit MAX_DEPTH), pause
        if !paused {
            paused = true;
            let mut status = shared.status.lock().unwrap();
            status.searching = false;
            shared.paused_cond.notify_all();
        }
    }
}

/// Compute reserve impact only for pieces not currently in reserves.
fn compute_reserve_impact_filtered(
    board: &Board,
    base_score: i32,
    tt: &mut CacheTable<TTEntry>,
) -> [i32; NUM_NON_KING_PIECES] {
    let color = board.side_to_move();
    let reserves = &board.reserves()[color.to_index()];
    let mut impact = [0i32; NUM_NON_KING_PIECES];

    for &piece in &NON_KING_PIECES {
        // Skip pieces we already have in reserves
        if reserves.count(piece) > 0 {
            continue;
        }
        let mut hypothetical = *board;
        hypothetical.add_to_reserve(color, piece);

        if let Some(result) = search::find_best_move(&hypothetical, 3) {
            impact[piece.to_index()] = result.score - base_score;
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
