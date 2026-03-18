//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, eval threads) and
//! `process_command()` which maps parsed UBI commands to responses.
//! No I/O — all output is returned as `Vec<UbiResponse>`.

use bughouse_chess::{Board, CacheTable, Color, Piece, NUM_NON_KING_PIECES};
use log::{info, warn, debug};
use rand::seq::SliceRandom;
use std::time::Instant;

use crate::book::OpeningBook;
use crate::engine::{self, EvalCommand, EvalHandle};
use crate::search::{self, BoardEval, TTEntry, TT_DEFAULT, TT_DEFAULT_SIZE};
use crate::strategy;
use crate::ubi::{BoardId, UbiCommand, UbiResponse, PositionSpec, format_move};

// ─── Engine State ────────────────────────────────────────────────────

pub struct EngineState {
    boards: [Option<Board>; 2],
    clocks: [u64; 4],  // white_A=0, black_A=1, white_B=2, black_B=3
    rng: rand::rngs::ThreadRng,
    pub game_id: String,
    eval_handles: [EvalHandle; 2],
    /// Which boards we currently have active `go` commands for.
    active_go: [bool; 2],
    /// Which color is "our team" on each board. Set on first `go` using
    /// bughouse pairing rule (white on A = black on B).
    our_color: [Option<Color>; 2],
    book: OpeningBook,
}

impl EngineState {
    pub fn new(game_id: String) -> Self {
        EngineState {
            boards: [None, None],
            clocks: [0; 4],
            rng: rand::thread_rng(),
            game_id,
            eval_handles: [engine::spawn_eval_thread(), engine::spawn_eval_thread()],
            active_go: [false; 2],
            our_color: [None; 2],
            book: OpeningBook::new(),
        }
    }

    /// Reset all state for a new game.
    pub fn reset(&mut self) {
        self.boards = [None, None];
        self.clocks = [0; 4];
        self.active_go = [false; 2];
        self.our_color = [None; 2];
        // Shut down old eval threads and spawn new ones
        for handle in &self.eval_handles {
            handle.send(EvalCommand::Quit);
        }
        self.eval_handles = [engine::spawn_eval_thread(), engine::spawn_eval_thread()];
    }

    /// Get a reference to the board for the given board id.
    pub fn board(&self, id: BoardId) -> Option<&Board> {
        self.boards[board_index(id)].as_ref()
    }
}

impl Drop for EngineState {
    fn drop(&mut self) {
        for handle in &self.eval_handles {
            handle.send(EvalCommand::Quit);
        }
    }
}

fn board_index(id: BoardId) -> usize {
    match id { BoardId::A => 0, BoardId::B => 1 }
}

// ─── Command Dispatch ────────────────────────────────────────────────

/// Process a parsed UBI command and return zero or more responses.
pub fn process_command(state: &mut EngineState, cmd: &UbiCommand) -> Vec<UbiResponse> {
    match cmd {
        UbiCommand::Ubi => vec![
            UbiResponse::IdName("BughouseEngine 0.1.0".to_string()),
            UbiResponse::IdAuthor("Viren Sawant".to_string()),
            UbiResponse::UbiOk,
        ],

        UbiCommand::IsReady => vec![UbiResponse::ReadyOk],

        UbiCommand::UbiNewGame => {
            info!("[game:{}] New game — state reset", state.game_id);
            state.reset();
            vec![]
        }

        UbiCommand::SetOption { .. } => vec![],

        UbiCommand::Position { board_a, board_b, clocks } => {
            handle_position_board(state, BoardId::A, board_a);
            handle_position_board(state, BoardId::B, board_b);
            state.clocks = *clocks;
            vec![]
        }

        UbiCommand::Go { board } => handle_go(state, *board),

        UbiCommand::Stop { board } => {
            match board {
                Some(id) => {
                    state.eval_handles[board_index(*id)].send(EvalCommand::Pause);
                }
                None => {
                    state.eval_handles[0].send(EvalCommand::Pause);
                    state.eval_handles[1].send(EvalCommand::Pause);
                }
            }
            vec![]
        }

        UbiCommand::PartnerMsg(msg) => {
            debug!("[game:{}] Partner message: {}", state.game_id, msg);
            vec![]  // Acknowledged, no response (future: influence search)
        }

        UbiCommand::Quit => {
            for handle in &state.eval_handles {
                handle.send(EvalCommand::Quit);
            }
            vec![]
        }

        UbiCommand::Unknown(line) => {
            warn!("[game:{}] Unknown command: {}", state.game_id, line);
            vec![]
        }
    }
}

// ─── Position handling ───────────────────────────────────────────────

fn handle_position_board(state: &mut EngineState, board_id: BoardId, spec: &PositionSpec) {
    let board = match spec {
        PositionSpec::StartPos => Board::default(),
        PositionSpec::Bfen(s) => match s.parse::<Board>() {
            Ok(b) => b,
            Err(e) => {
                warn!("[game:{}] Invalid BFEN for board {:?}: {}", state.game_id, board_id, e);
                return;
            }
        },
    };

    let idx = board_index(board_id);

    // Check if the position actually changed
    let hash_changed = match &state.boards[idx] {
        Some(old) => old.get_hash() != board.get_hash(),
        None => true,
    };

    state.boards[idx] = Some(board);

    // Log position with BFEN and hash
    debug!(
        "[game:{}] Board {:?}: {} to move, hash={:#x}{}",
        state.game_id, board_id,
        if board.side_to_move() == Color::White { "white" } else { "black" },
        board.get_hash(),
        if hash_changed { " (CHANGED)" } else { " (unchanged)" }
    );

    // If position changed, signal eval thread to restart search
    if hash_changed {
        state.eval_handles[idx].send(EvalCommand::NewPosition(board));
    }
}

// ─── Cross-Board Strategy ────────────────────────────────────────────

/// Compute the cross-board weight for the Standard strategy.
///
/// Determines how much to trust cross-board reserve_impact when adjusting
/// move scores, based on whether we control the other board and whose turn it is.
fn cross_board_weight(
    active_go_other: bool,
    other_board: Option<&Board>,
    our_color_on_other: Option<Color>,
) -> f32 {
    let our_teams_turn = match (other_board, our_color_on_other) {
        (Some(b), Some(c)) => b.side_to_move() == c,
        _ => false, // unknown — conservative
    };

    match (active_go_other, our_teams_turn) {
        (true, true)   => 1.0,   // We control both boards, our turn on other
        (true, false)  => 0.5,   // We control both boards, opponent's turn on other
        (false, true)  => 0.5,   // Partner controls other board, their turn
        (false, false) => 0.25,  // Partner controls other, opponent's turn
    }
}

// ─── Go handling (uses eval thread pondering) ───────────────────────

fn handle_go(state: &mut EngineState, board_id: BoardId) -> Vec<UbiResponse> {
    let board = match state.boards[board_index(board_id)] {
        Some(b) => b,
        None => {
            warn!("[game:{}] Go on unset board {:?}", state.game_id, board_id);
            return vec![];
        }
    };

    let go_idx = board_index(board_id);
    let other_idx = 1 - go_idx;
    let side = board.side_to_move();
    let _play_style = strategy::determine_play_style(&state.clocks, board_id, side);
    let budget_ms = crate::time::allocate_time(&state.clocks, board_id, side);

    // Track active go and team colors
    state.active_go[go_idx] = true;
    if state.our_color[go_idx].is_none() {
        // First go — set colors using bughouse pairing rule
        state.our_color[go_idx] = Some(side);
        state.our_color[other_idx] = Some(!side);
        debug!("[game:{}] Team colors set: board {:?}={:?}, other={:?}",
            state.game_id, board_id, side, !side);
    }

    // Log clock state and budget
    let other_id = if board_id == BoardId::A { BoardId::B } else { BoardId::A };
    let board_idx = go_idx;
    let color_idx = side.to_index();
    let our_time = state.clocks[board_idx * 2 + color_idx];
    let opp_time = state.clocks[board_idx * 2 + (1 - color_idx)];
    info!(
        "[game:{}] Board {:?} go: our_time={}ms opp_time={}ms budget={}ms style={:?}",
        state.game_id, board_id, our_time, opp_time, budget_ms, _play_style
    );

    // Opening book check — instant response if position is in book
    if let Some(book_move) = state.book.lookup(&board, &mut state.rng) {
        let move_str = format_move(&book_move);
        info!("[game:{}] Board {:?}: BOOK HIT — playing {} instantly",
            state.game_id, board_id, move_str);
        state.active_go[go_idx] = false;
        return vec![
            UbiResponse::Info {
                board: board_id, depth: 0, nodes: 0, time_ms: 0,
                score_cp: 0, pv: vec![move_str.clone()],
            },
            UbiResponse::BestMove { board: board_id, move_str },
        ];
    }

    // Wait for the eval thread to search within our time budget.
    // The eval thread started when the position command arrived.
    // We use wait_for_depth_or_timeout with a very high min_depth — the timeout
    // is what actually controls when we stop. This way the condvar wakes us on
    // each completed depth (for future use), and we stop at the budget.
    let timeout = std::time::Duration::from_millis(budget_ms);
    let expected_hash = board.get_hash();
    let eval_status = state.eval_handles[go_idx].shared
        .wait_for_depth_or_timeout(expected_hash, 64, timeout); // 64 = effectively "wait for timeout"

    // Peek other board's eval (no waiting, eval thread keeps running)
    let other_eval_status = state.eval_handles[other_idx].status();

    // Compute reserve impact for the OTHER board (what pieces would help them?)
    // This runs on the main thread and does NOT block the eval threads.
    let other_reserve_impact = if other_eval_status.completed_depth >= 1 {
        if let Some(other_board) = state.boards[other_idx] {
            let ri = engine::compute_reserve_impact_fast(
                &other_board,
                other_eval_status.best_score,
                2, // depth 2 for drop search — fast but meaningful with quiescence
            );
            debug!(
                "[game:{}] Board {:?} reserve_impact (fast): [P:{} N:{} B:{} R:{} Q:{}]",
                state.game_id, other_id, ri[0], ri[1], ri[2], ri[3], ri[4]
            );
            ri
        } else {
            [0; NUM_NON_KING_PIECES]
        }
    } else {
        [0; NUM_NON_KING_PIECES]
    };

    // Log eval results
    let go_eval = &eval_status.eval;
    info!(
        "[game:{}] Board {:?} eval: score={} depth={}",
        state.game_id, board_id, go_eval.score, go_eval.depth,
    );
    if other_eval_status.completed_depth >= 1 {
        info!(
            "[game:{}] Board {:?} eval: score={} depth={}",
            state.game_id, other_id, other_eval_status.eval.score, other_eval_status.eval.depth
        );
    }

    // Cross-board move selection
    let has_other_eval = other_eval_status.completed_depth >= 1;
    let chosen_str = if eval_status.completed_depth >= 1 && !eval_status.root_moves.is_empty() {
        if has_other_eval {
            // Full cross-board analysis
            let ranking = engine::compute_cross_board_ranking(&eval_status, &other_reserve_impact, other_eval_status.completed_depth);
            let weight = cross_board_weight(
                state.active_go[other_idx],
                state.boards[other_idx].as_ref(),
                state.our_color[other_idx],
            );

            // Debug: log what the other board needs
            {
                let impact = &other_eval_status.eval.reserve_impact;
                let piece_names = ["pawn", "knight", "bishop", "rook", "queen"];
                let mut needs = Vec::new();
                for (i, name) in piece_names.iter().enumerate() {
                    if impact[i] != 0 {
                        needs.push(format!("{}({:+})", name, impact[i]));
                    }
                }
                let needs_str = if needs.is_empty() { "nothing".to_string() } else { needs.join(", ") };
                debug!("[game:{}] Board {:?} needs from reserves: {} (depth {})",
                    state.game_id, other_id, needs_str, other_eval_status.completed_depth);
            }

            // Debug: log weight reasoning
            {
                let our_turn_other = match (state.boards[other_idx], state.our_color[other_idx]) {
                    (Some(b), Some(c)) => if b.side_to_move() == c { "our team's turn" } else { "opponent's turn" },
                    _ => "unknown",
                };
                debug!("[game:{}] Cross-board weight: active_go[{:?}]={} other_board={} → weight={:.2}",
                    state.game_id, other_id, state.active_go[other_idx], our_turn_other, weight);
            }

            // Apply weights and rank all moves
            let mut scored_moves: Vec<(String, i32, i32, i32, Option<Piece>)> = Vec::new();
            for am in &ranking.moves {
                let adjusted = am.local_score + (weight * am.cross_board_value as f32) as i32;
                scored_moves.push((format_move(&am.mv), am.local_score, am.cross_board_value, adjusted, am.captured));
            }
            scored_moves.sort_by(|a, b| b.3.cmp(&a.3));

            // Debug: log top 5 moves
            for (i, (mv_str, local, cross, adjusted, captured)) in scored_moves.iter().take(5).enumerate() {
                let cap_str = match captured { Some(p) => format!(" captures {:?}", p), None => String::new() };
                let cross_str = if *cross != 0 {
                    format!(" cross={:+}×{:.2}={:+}", cross, weight, (*cross as f32 * weight) as i32)
                } else { String::new() };
                debug!("[game:{}] Board {:?} move {}: {} local={:+}{}{} → adjusted={:+}",
                    state.game_id, board_id, i + 1, mv_str, local, cap_str, cross_str, adjusted);
            }

            // Check for cross-board override
            let best_local_move = scored_moves.iter().max_by_key(|m| m.1).map(|m| &m.0);
            let (best_move_str, _, best_cross, best_adjusted, _) = &scored_moves[0];
            if let Some(blm) = best_local_move {
                if blm != best_move_str {
                    info!("[game:{}] Board {:?}: CROSS-BOARD OVERRIDE: {} (adjusted={}) over {} (local best)",
                        state.game_id, board_id, best_move_str, best_adjusted, blm);
                }
            }

            let log_str = if *best_cross != 0 {
                format!("local cross_board={} weight={:.2} adjusted={}", best_cross, weight, best_adjusted)
            } else {
                format!("local (no cross-board impact)")
            };
            info!("[game:{}] Board {:?}: depth {} chose {} ({})",
                state.game_id, board_id, eval_status.completed_depth, best_move_str, log_str);

            best_move_str.clone()
        } else {
            // No other board eval — use local best
            let move_str = format_move(eval_status.best_move.as_ref().unwrap());
            info!("[game:{}] Board {:?}: depth {} score {} cp, chose {} (no cross-board data)",
                state.game_id, board_id, eval_status.completed_depth, eval_status.best_score, move_str);
            move_str
        }
    } else {
        // Eval thread didn't reach depth 1 — shouldn't happen but handle gracefully
        match &eval_status.best_move {
            Some(m) => {
                let move_str = format_move(m);
                warn!("[game:{}] Board {:?}: only reached depth {}, chose {}",
                    state.game_id, board_id, eval_status.completed_depth, move_str);
                move_str
            }
            None => {
                warn!("[game:{}] No moves available for board {:?}", state.game_id, board_id);
                "(none)".to_string()
            }
        }
    };

    state.active_go[go_idx] = false;

    // Build response: TeamMsg + per-depth Info lines + BestMove
    let team_msgs = [
        "need n urgency high", "need q urgency medium", "need b",
        "need r urgency low", "need p", "stall", "stall duration 2",
        "play_fast reason time", "play_fast reason pressure",
        "threat critical", "threat high", "threat medium", "threat low",
        "material +100", "material -50",
    ];
    let random_msg = team_msgs.choose(&mut state.rng).unwrap();

    let mut responses = vec![UbiResponse::TeamMsg(random_msg.to_string())];

    for info in &eval_status.info_lines {
        responses.push(UbiResponse::Info {
            board: board_id, depth: info.depth, nodes: info.nodes,
            time_ms: info.time_ms, score_cp: info.score, pv: info.pv.clone(),
        });
    }

    responses.push(UbiResponse::BestMove {
        board: board_id,
        move_str: chosen_str,
    });

    responses
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ubi::{BoardId, UbiCommand, UbiResponse, PositionSpec};
    use bughouse_chess::{BughouseMove, Color, MoveGen, Piece};

    fn new_state() -> EngineState {
        EngineState::new("test".to_string())
    }

    /// Helper: send a position command with both boards at startpos and default clocks.
    fn set_startpos(state: &mut EngineState) {
        process_command(state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [180000, 180000, 180000, 180000],
        });
    }

    #[test]
    fn handshake_flow() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::Ubi);
        assert_eq!(resp.len(), 3);
        assert!(matches!(&resp[0], UbiResponse::IdName(n) if n.contains("BughouseEngine")));
        assert!(matches!(&resp[1], UbiResponse::IdAuthor(_)));
        assert_eq!(resp[2], UbiResponse::UbiOk);
    }

    #[test]
    fn isready_readyok() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::IsReady);
        assert_eq!(resp, vec![UbiResponse::ReadyOk]);
    }

    #[test]
    fn ubinewgame_resets() {
        let mut state = new_state();
        set_startpos(&mut state);
        assert!(state.board(BoardId::A).is_some());

        process_command(&mut state, &UbiCommand::UbiNewGame);
        assert!(state.board(BoardId::A).is_none());
        assert!(state.board(BoardId::B).is_none());
        assert_eq!(state.clocks, [0; 4]);
    }

    #[test]
    fn position_sets_both_boards_and_clocks() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [180000, 175000, 182000, 178000],
        });
        assert_eq!(*state.board(BoardId::A).unwrap(), Board::default());
        assert_eq!(*state.board(BoardId::B).unwrap(), Board::default());
        assert_eq!(state.clocks, [180000, 175000, 182000, 178000]);
    }

    #[test]
    fn position_bfen_with_reserves() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1".to_string()
            ),
            board_b: PositionSpec::StartPos,
            clocks: [180000, 180000, 180000, 180000],
        });
        let board = state.board(BoardId::A).unwrap();
        assert_eq!(board.reserves()[Color::White.to_index()].count(Piece::Queen), 1);
        assert_eq!(board.reserves()[Color::White.to_index()].count(Piece::Knight), 1);
        assert_eq!(board.reserves()[Color::White.to_index()].count(Piece::Pawn), 1);
        assert_eq!(board.reserves()[Color::Black.to_index()].count(Piece::Queen), 1);
        assert_eq!(board.reserves()[Color::Black.to_index()].count(Piece::Pawn), 1);
    }

    #[test]
    fn go_produces_teammsg_info_and_bestmove() {
        let mut state = new_state();
        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // Book hit: Info + BestMove (2 responses)
        // Normal: TeamMsg + Info lines + BestMove (3+ responses)
        assert!(resp.len() >= 2, "expected at least 2 responses, got {}", resp.len());
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::A, .. }));
    }

    #[test]
    fn go_bestmove_is_legal() {
        let mut state = new_state();
        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        let move_str = match &resp[resp.len() - 1] {
            UbiResponse::BestMove { move_str, .. } => move_str.clone(),
            _ => panic!("expected BestMove as last response"),
        };

        let bm: BughouseMove = move_str.parse().unwrap();
        let board = state.board(BoardId::A).unwrap();
        let legal_regular: Vec<BughouseMove> = MoveGen::new_legal(board)
            .map(BughouseMove::Regular)
            .collect();
        let legal_drops = MoveGen::drop_moves(board);
        let all_legal: Vec<BughouseMove> = legal_regular.into_iter().chain(legal_drops).collect();
        assert!(all_legal.contains(&bm), "bestmove {} not in legal moves", move_str);
    }

    #[test]
    fn go_unset_board() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        assert!(resp.len() <= 1, "should return empty or bestmove none");
    }

    #[test]
    fn go_includes_drops() {
        // Use a midgame position with reserves that is NOT in the opening book.
        // This position has pieces developed beyond any book line.
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "r1bqk2r/pppp1ppp/2n2n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQ1RK1[N] b kq - 5 4".to_string()
            ),
            board_b: PositionSpec::StartPos,
            clocks: [180000, 180000, 180000, 180000],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // Should have info lines with nodes > 20 (drops add candidates)
        let has_info = resp.iter().any(|r| matches!(r, UbiResponse::Info { nodes, .. } if *nodes > 20));
        assert!(has_info, "expected drops to increase node count");
    }

    #[test]
    fn info_node_count() {
        let mut state = new_state();
        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        let info_count = resp.iter().filter(|r| matches!(r, UbiResponse::Info { .. })).count();
        assert!(info_count >= 1, "should have at least 1 info line, got {}", info_count);
    }

    #[test]
    fn setoption_silent() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::SetOption {
            name: "Hash".to_string(),
            value: Some("256".to_string()),
        });
        assert!(resp.is_empty());
    }

    #[test]
    fn stop_returns_empty() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::Stop { board: None });
        assert!(resp.is_empty());
    }

    #[test]
    fn unknown_returns_empty() {
        let mut state = new_state();
        let resp = process_command(&mut state, &UbiCommand::Unknown("garbage".to_string()));
        assert!(resp.is_empty());
    }

    #[test]
    fn multi_command_session() {
        let mut state = new_state();

        let resp = process_command(&mut state, &UbiCommand::Ubi);
        assert_eq!(resp.len(), 3);

        let resp = process_command(&mut state, &UbiCommand::IsReady);
        assert_eq!(resp.len(), 1);

        let resp = process_command(&mut state, &UbiCommand::UbiNewGame);
        assert!(resp.is_empty());

        set_startpos(&mut state);

        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        assert!(resp.len() >= 2, "expected at least 2 responses for board A, got {}", resp.len());
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::A, .. }));

        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        assert!(resp.len() >= 2, "expected at least 2 responses for board B, got {}", resp.len());
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::B, .. }));

        let resp = process_command(&mut state, &UbiCommand::Quit);
        assert!(resp.is_empty());
    }

    #[test]
    fn bestmove_format_compliance() {
        let mut state = new_state();

        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        if let UbiResponse::BestMove { move_str, .. } = &resp[resp.len() - 1] {
            assert!(move_str.len() >= 4, "move too short: {}", move_str);
            assert!(!move_str.contains('@'), "unexpected drop in startpos: {}", move_str);
        }
    }

    #[test]
    fn position_updates_clocks() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [180000, 175000, 182000, 178000],
        });
        assert_eq!(state.clocks, [180000, 175000, 182000, 178000]);

        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [170000, 165000, 172000, 168000],
        });
        assert_eq!(state.clocks, [170000, 165000, 172000, 168000]);
    }

    #[test]
    fn eval_thread_ponders_and_produces_results() {
        let mut state = new_state();
        set_startpos(&mut state);

        // Give eval threads time to ponder (generous for CI/parallel test runs)
        std::thread::sleep(std::time::Duration::from_millis(1000));

        // Check that eval thread A has been working
        let status = state.eval_handles[0].status();
        assert!(status.completed_depth >= 1,
            "eval thread should have pondered to at least depth 1, got depth {}",
            status.completed_depth
        );
        assert!(status.best_move.is_some(), "eval thread should have a best move");
    }
}
