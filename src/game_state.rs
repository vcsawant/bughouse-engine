//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, eval threads) and
//! `process_command()` which maps parsed UBI commands to responses.
//! No I/O — all output is returned as `Vec<UbiResponse>`.

use bughouse_chess::{Board, CacheTable};
use log::{info, warn, debug};
use rand::seq::SliceRandom;
use std::time::Instant;

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
}

impl EngineState {
    pub fn new(game_id: String) -> Self {
        EngineState {
            boards: [None, None],
            clocks: [0; 4],
            rng: rand::thread_rng(),
            game_id,
            eval_handles: [engine::spawn_eval_thread(), engine::spawn_eval_thread()],
        }
    }

    /// Reset all state for a new game.
    pub fn reset(&mut self) {
        self.boards = [None, None];
        self.clocks = [0; 4];
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
        PositionSpec::StartPos => {
            debug!("[game:{}] Board {:?} set to startpos", state.game_id, board_id);
            Board::default()
        }
        PositionSpec::Bfen(s) => match s.parse::<Board>() {
            Ok(b) => {
                debug!("[game:{}] Board {:?} set from BFEN", state.game_id, board_id);
                b
            }
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

    // If position changed, signal eval thread to restart search
    if hash_changed {
        debug!("[game:{}] Board {:?} position changed, restarting eval", state.game_id, board_id);
        state.eval_handles[idx].send(EvalCommand::NewPosition(board));
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

    // Log clock state and budget
    let board_idx = go_idx;
    let color_idx = side.to_index();
    let our_time = state.clocks[board_idx * 2 + color_idx];
    let opp_time = state.clocks[board_idx * 2 + (1 - color_idx)];
    info!(
        "[game:{}] Board {:?} go: our_time={}ms opp_time={}ms budget={}ms style={:?}",
        state.game_id, board_id, our_time, opp_time, budget_ms, _play_style
    );

    // Check if eval thread has pondered results ready
    let eval_status = state.eval_handles[go_idx].status();
    let has_pondered = eval_status.board_hash == board.get_hash()
        && eval_status.completed_depth >= 1
        && eval_status.best_move.is_some();

    if has_pondered {
        // Eval thread has been pondering — give it the time budget to go deeper
        let deadline = Instant::now() + std::time::Duration::from_millis(budget_ms);
        state.eval_handles[go_idx].send(EvalCommand::SetDeadline(deadline));
        state.eval_handles[go_idx].wait_for_pause();
    } else {
        // No pondered results — pause eval thread and do synchronous search
        state.eval_handles[go_idx].send(EvalCommand::Pause);
        state.eval_handles[go_idx].wait_for_pause();

        debug!("[game:{}] Board {:?}: no pondered results, synchronous search", state.game_id, board_id);
        let mut info_lines = Vec::new();
        let mut fallback_tt = CacheTable::new(TT_DEFAULT_SIZE, TT_DEFAULT);
        if let Some(result) = search::find_best_move_timed(&board, budget_ms, &mut info_lines, &mut fallback_tt) {
            state.eval_handles[go_idx].send(EvalCommand::Resume);

            let chosen_str = format_move(&result.best_move);
            let team_msgs = ["need n urgency high", "need q urgency medium", "need b",
                "need r urgency low", "need p", "stall", "threat medium", "material +100"];
            let random_msg = team_msgs.choose(&mut state.rng).unwrap();
            let mut responses = vec![UbiResponse::TeamMsg(random_msg.to_string())];
            for info in &info_lines {
                responses.push(UbiResponse::Info {
                    board: board_id, depth: info.depth, nodes: info.nodes,
                    time_ms: info.time_ms, score_cp: info.score, pv: info.pv.clone(),
                });
            }
            responses.push(UbiResponse::BestMove {
                board: board_id, move_str: chosen_str,
            });
            return responses;
        }
        // If synchronous search also fails, fall through to "(none)" path below
    }

    // Read eval results (from pondering path)
    let eval_status = state.eval_handles[go_idx].status();

    // Log eval results
    let go_eval = &eval_status.eval;
    info!(
        "[game:{}] Board {:?} eval: score={} depth={} reserve_impact=[P:{} N:{} B:{} R:{} Q:{}]",
        state.game_id, board_id, go_eval.score, go_eval.depth,
        go_eval.reserve_impact[0], go_eval.reserve_impact[1],
        go_eval.reserve_impact[2], go_eval.reserve_impact[3], go_eval.reserve_impact[4]
    );

    // Peek at the other board's eval (no pause needed)
    if state.boards[other_idx].is_some() {
        let other_id = if board_id == BoardId::A { BoardId::B } else { BoardId::A };
        let other_eval_status = state.eval_handles[other_idx].status();
        info!(
            "[game:{}] Board {:?} eval: score={} depth={}",
            state.game_id, other_id, other_eval_status.eval.score, other_eval_status.eval.depth
        );
    }

    // Pick the best move from eval results
    let (chosen_move, chosen_str) = match &eval_status.best_move {
        Some(m) => (m.clone(), format_move(m)),
        None => {
            warn!("[game:{}] No best move from eval thread for board {:?}", state.game_id, board_id);
            // Resume eval thread before returning
            state.eval_handles[go_idx].send(EvalCommand::Resume);
            return vec![
                UbiResponse::BestMove {
                    board: board_id,
                    move_str: "(none)".to_string(),
                },
            ];
        }
    };

    info!(
        "[game:{}] Board {:?}: depth {} score {} cp, chose {}",
        state.game_id, board_id, eval_status.completed_depth,
        eval_status.best_score, chosen_str
    );

    // Resume eval thread for continued pondering
    state.eval_handles[go_idx].send(EvalCommand::Resume);

    // Build response: TeamMsg + per-depth Info lines + BestMove
    let team_msgs = [
        "need n urgency high",
        "need q urgency medium",
        "need b",
        "need r urgency low",
        "need p",
        "stall",
        "stall duration 2",
        "play_fast reason time",
        "play_fast reason pressure",
        "threat critical",
        "threat high",
        "threat medium",
        "threat low",
        "material +100",
        "material -50",
    ];
    let random_msg = team_msgs.choose(&mut state.rng).unwrap();

    let mut responses = vec![UbiResponse::TeamMsg(random_msg.to_string())];

    // Emit info lines from eval thread's accumulated data
    for info in &eval_status.info_lines {
        responses.push(UbiResponse::Info {
            board: board_id,
            depth: info.depth,
            nodes: info.nodes,
            time_ms: info.time_ms,
            score_cp: info.score,
            pv: info.pv.clone(),
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
        // TeamMsg + at least 1 Info line + BestMove
        assert!(resp.len() >= 3, "expected at least 3 responses, got {}", resp.len());
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::A, .. }));
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
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1".to_string()
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
        let has_info = resp.iter().any(|r| matches!(r, UbiResponse::Info { nodes, .. } if *nodes > 20));
        assert!(has_info, "should evaluate many nodes");
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
        assert!(resp.len() >= 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::A, .. }));

        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        assert!(resp.len() >= 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
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

        // Give eval threads time to ponder
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Check that eval thread A has been working
        let status = state.eval_handles[0].status();
        assert!(status.completed_depth >= 1,
            "eval thread should have pondered to at least depth 1, got depth {}",
            status.completed_depth
        );
        assert!(status.best_move.is_some(), "eval thread should have a best move");
    }
}
