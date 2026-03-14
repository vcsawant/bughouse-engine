//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, RNG) and
//! `process_command()` which maps parsed UBI commands to responses.
//! No I/O — all output is returned as `Vec<UbiResponse>`.

use bughouse_chess::Board;
use log::{info, warn, debug};
use rand::seq::SliceRandom;

use crate::search;
use crate::strategy;
use crate::ubi::{BoardId, UbiCommand, UbiResponse, PositionSpec, format_move};

// ─── Engine State ────────────────────────────────────────────────────

pub struct EngineState {
    boards: [Option<Board>; 2],
    clocks: [u64; 4],  // white_A=0, black_A=1, white_B=2, black_B=3
    rng: rand::rngs::ThreadRng,
    pub game_id: String,
}

impl EngineState {
    pub fn new(game_id: String) -> Self {
        EngineState {
            boards: [None, None],
            clocks: [0; 4],
            rng: rand::thread_rng(),
            game_id,
        }
    }

    /// Reset all state for a new game.
    pub fn reset(&mut self) {
        self.boards = [None, None];
        self.clocks = [0; 4];
    }

    /// Get a reference to the board for the given board id.
    pub fn board(&self, id: BoardId) -> Option<&Board> {
        self.boards[board_index(id)].as_ref()
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

        UbiCommand::Stop { .. } => vec![],

        UbiCommand::PartnerMsg(msg) => {
            debug!("[game:{}] Partner message: {}", state.game_id, msg);
            vec![]  // Acknowledged, no response (future: influence search)
        }

        UbiCommand::Quit => vec![],

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

    state.boards[board_index(board_id)] = Some(board);
}

// ─── Go handling (iterative deepening search) ───────────────────────

fn handle_go(state: &mut EngineState, board_id: BoardId) -> Vec<UbiResponse> {
    let board = match &state.boards[board_index(board_id)] {
        Some(b) => b,
        None => {
            warn!("[game:{}] Go on unset board {:?}", state.game_id, board_id);
            return vec![];
        }
    };

    let side = board.side_to_move();
    let _play_style = strategy::determine_play_style(&state.clocks, board_id, side);
    let budget_ms = crate::time::allocate_time(&state.clocks, board_id, side);

    let mut info_lines = Vec::new();
    let result = match search::find_best_move_timed(board, budget_ms, &mut info_lines) {
        Some(r) => r,
        None => {
            warn!("[game:{}] No legal moves on board {:?}", state.game_id, board_id);
            return vec![
                UbiResponse::BestMove {
                    board: board_id,
                    move_str: "(none)".to_string(),
                },
            ];
        }
    };

    let chosen_str = format_move(&result.best_move);

    info!(
        "[game:{}] Board {:?}: depth {} searched {} nodes, score {} cp, chose {} in {}ms (budget {}ms)",
        state.game_id, board_id, result.depth, result.nodes, result.score, chosen_str,
        info_lines.last().map_or(0, |i| i.time_ms), budget_ms
    );

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

    for info in &info_lines {
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
        // Set up some state
        set_startpos(&mut state);
        assert!(state.board(BoardId::A).is_some());

        // Reset
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

        // Parse the returned move and check it's in the legal move list
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
        assert!(resp.is_empty());
    }

    #[test]
    fn go_includes_drops() {
        let mut state = new_state();
        // Position where white has pieces in reserve — drops should be possible
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1".to_string()
            ),
            board_b: PositionSpec::StartPos,
            clocks: [180000, 180000, 180000, 180000],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // Starting position has 20 regular moves. With a knight in reserve,
        // the knight can drop on all empty squares (32 squares in middle 4 ranks).
        // Total should be > 20
        // Check the deepest info line (second-to-last response)
        let deepest_info = &resp[resp.len() - 2];
        if let UbiResponse::Info { nodes, .. } = deepest_info {
            assert!(*nodes > 20, "expected drops to increase node count, got {}", nodes);
        } else {
            panic!("expected Info response");
        }
    }

    #[test]
    fn info_node_count() {
        let mut state = new_state();
        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // With iterative deepening, total nodes should be substantial
        let deepest_info = &resp[resp.len() - 2];
        if let UbiResponse::Info { nodes, .. } = deepest_info {
            assert!(*nodes > 20, "should evaluate many nodes, got {}", nodes);
        } else {
            panic!("expected Info response");
        }
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

        // Handshake
        let resp = process_command(&mut state, &UbiCommand::Ubi);
        assert_eq!(resp.len(), 3);

        // Ready check
        let resp = process_command(&mut state, &UbiCommand::IsReady);
        assert_eq!(resp.len(), 1);

        // New game
        let resp = process_command(&mut state, &UbiCommand::UbiNewGame);
        assert!(resp.is_empty());

        // Set up both boards with single position command
        set_startpos(&mut state);

        // Go on board A
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        assert!(resp.len() >= 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::A, .. }));
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::A, .. }));

        // Go on board B
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        assert!(resp.len() >= 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::B, .. }));
        assert!(matches!(&resp[resp.len() - 1], UbiResponse::BestMove { board: BoardId::B, .. }));

        // Quit
        let resp = process_command(&mut state, &UbiCommand::Quit);
        assert!(resp.is_empty());
    }

    #[test]
    fn bestmove_format_compliance() {
        let mut state = new_state();

        // Test regular move format (from starting position)
        set_startpos(&mut state);
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        if let UbiResponse::BestMove { move_str, .. } = &resp[resp.len() - 1] {
            // Regular UCI move: 4 chars like "e2e4" or 5 for promotion "e7e8q"
            assert!(move_str.len() >= 4, "move too short: {}", move_str);
            // Should not contain '@' (no reserves in starting position)
            assert!(!move_str.contains('@'), "unexpected drop in startpos: {}", move_str);
        }

        // Test drop move format
        // Position where a drop is clearly the best move: white has a queen in
        // reserve and an open board with few regular moves
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[Q] w KQkq - 0 1".to_string()
            ),
            clocks: [180000, 180000, 180000, 180000],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        if let UbiResponse::BestMove { move_str, .. } = &resp[resp.len() - 1] {
            if move_str.contains('@') {
                // Drop format: lowercase piece letter + @ + square
                assert_eq!(move_str.as_bytes()[0], b'q', "expected lowercase q, got {}", move_str);
                assert_eq!(move_str.as_bytes()[1], b'@');
            }
            // Either a drop or a regular move is fine — both formats are valid
            assert!(move_str.len() >= 3, "move too short: {}", move_str);
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

        // Update with new clocks
        process_command(&mut state, &UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [170000, 165000, 172000, 168000],
        });
        assert_eq!(state.clocks, [170000, 165000, 172000, 168000]);
    }
}
