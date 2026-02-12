//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, RNG) and
//! `process_command()` which maps parsed UBI commands to responses.
//! No I/O — all output is returned as `Vec<UbiResponse>`.

use std::time::Instant;

use bughouse_chess::{Board, BughouseMove};
use log::{info, warn, debug};
use rand::seq::SliceRandom;

use crate::search;
use crate::strategy;
use crate::ubi::{BoardId, UbiCommand, UbiResponse, ClockTarget, PositionSpec, format_move};

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

    /// Get the clock value for a specific player.
    pub fn clock(&self, target: &ClockTarget) -> u64 {
        self.clocks[clock_index(target)]
    }
}

fn board_index(id: BoardId) -> usize {
    match id { BoardId::A => 0, BoardId::B => 1 }
}

fn clock_index(target: &ClockTarget) -> usize {
    let board_off = match target.board { BoardId::A => 0, BoardId::B => 2 };
    let color_off = match target.color {
        bughouse_chess::Color::White => 0,
        bughouse_chess::Color::Black => 1,
    };
    board_off + color_off
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

        UbiCommand::Position { board, fen, moves } => {
            handle_position(state, *board, fen, moves);
            vec![]
        }

        UbiCommand::Clock { target, millis } => {
            state.clocks[clock_index(target)] = *millis;
            vec![]
        }

        UbiCommand::Go { board } => handle_go(state, *board),

        UbiCommand::Stop { .. } => vec![],

        UbiCommand::Quit => vec![],

        UbiCommand::Unknown(line) => {
            warn!("[game:{}] Unknown command: {}", state.game_id, line);
            vec![]
        }
    }
}

// ─── Position handling ───────────────────────────────────────────────

fn handle_position(state: &mut EngineState, board_id: BoardId, fen: &PositionSpec, moves: &[String]) {
    let mut board = match fen {
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

    for move_str in moves {
        match move_str.parse::<BughouseMove>() {
            Ok(BughouseMove::Regular(cm)) => {
                board = board.make_move_new(cm);
            }
            Ok(BughouseMove::Drop { piece, square }) => {
                match board.make_drop_new(piece, square) {
                    Some(new_board) => board = new_board,
                    None => {
                        warn!("[game:{}] Illegal drop: {}", state.game_id, move_str);
                    }
                }
            }
            Err(e) => {
                warn!("[game:{}] Invalid move '{}': {}", state.game_id, move_str, e);
            }
        }
    }

    state.boards[board_index(board_id)] = Some(board);
}

// ─── Go handling (1-ply search) ──────────────────────────────────────

fn handle_go(state: &mut EngineState, board_id: BoardId) -> Vec<UbiResponse> {
    let board = match &state.boards[board_index(board_id)] {
        Some(b) => b,
        None => {
            warn!("[game:{}] Go on unset board {:?}", state.game_id, board_id);
            return vec![];
        }
    };

    let start = Instant::now();

    let play_style = strategy::determine_play_style(&state.clocks);
    let result = match search::find_best_move(board, play_style) {
        Some(r) => r,
        None => {
            warn!("[game:{}] No legal moves on board {:?}", state.game_id, board_id);
            return vec![];
        }
    };

    let chosen_str = format_move(&result.best_move);
    let elapsed_ms = start.elapsed().as_millis() as u64;

    info!(
        "[game:{}] Board {:?}: searched {} nodes, score {} cp, chose {} in {}ms",
        state.game_id, board_id, result.nodes, result.score, chosen_str, elapsed_ms
    );

    // Send a random team message before the best move (for testing bot→human comms)
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

    vec![
        UbiResponse::TeamMsg(random_msg.to_string()),
        UbiResponse::Info {
            board: board_id,
            depth: 1,
            nodes: result.nodes,
            time_ms: elapsed_ms,
            score_cp: result.score,
        },
        UbiResponse::BestMove {
            board: board_id,
            move_str: chosen_str,
        },
    ]
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ubi::{BoardId, UbiCommand, UbiResponse, ClockTarget, PositionSpec};
    use bughouse_chess::{BughouseMove, Color, MoveGen, Piece, Square};
    use std::str::FromStr;

    fn new_state() -> EngineState {
        EngineState::new("test".to_string())
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
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        state.clocks = [100, 200, 300, 400];

        // Reset
        process_command(&mut state, &UbiCommand::UbiNewGame);
        assert!(state.board(BoardId::A).is_none());
        assert!(state.board(BoardId::B).is_none());
        assert_eq!(state.clocks, [0; 4]);
    }

    #[test]
    fn position_startpos() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let board = state.board(BoardId::A).unwrap();
        assert_eq!(*board, Board::default());
    }

    #[test]
    fn position_bfen_with_reserves() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
        let board = state.board(BoardId::A).unwrap();
        assert_eq!(board.reserves(Color::White).count(Piece::Queen), 1);
        assert_eq!(board.reserves(Color::White).count(Piece::Knight), 1);
        assert_eq!(board.reserves(Color::White).count(Piece::Pawn), 1);
        assert_eq!(board.reserves(Color::Black).count(Piece::Queen), 1);
        assert_eq!(board.reserves(Color::Black).count(Piece::Pawn), 1);
    }

    #[test]
    fn position_with_regular_moves() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::StartPos,
            moves: vec!["e2e4".to_string(), "d7d5".to_string()],
        });
        let board = state.board(BoardId::A).unwrap();
        // After e2e4 d7d5, it's white to move
        assert_eq!(board.side_to_move(), Color::White);
        // e4 should have a pawn, d5 should have a pawn
        assert!(board.piece_on(Square::from_str("e4").unwrap()).is_some());
        assert!(board.piece_on(Square::from_str("d5").unwrap()).is_some());
        // e2 and d7 should be empty
        assert!(board.piece_on(Square::from_str("e2").unwrap()).is_none());
        assert!(board.piece_on(Square::from_str("d7").unwrap()).is_none());
    }

    #[test]
    fn position_with_drop_moves() {
        let mut state = new_state();
        // Use a BFEN with knight in white's reserve, white to move
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::B,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1".to_string()
            ),
            moves: vec!["n@e4".to_string()],
        });
        let board = state.board(BoardId::B).unwrap();
        // Knight should be on e4
        assert_eq!(board.piece_on(Square::from_str("e4").unwrap()), Some(Piece::Knight));
        // Reserve should be decremented
        assert_eq!(board.reserves(Color::White).count(Piece::Knight), 0);
    }

    #[test]
    fn clock_updates() {
        let mut state = new_state();
        let targets = [
            (ClockTarget { color: Color::White, board: BoardId::A }, 180000u64),
            (ClockTarget { color: Color::Black, board: BoardId::A }, 175000u64),
            (ClockTarget { color: Color::White, board: BoardId::B }, 182000u64),
            (ClockTarget { color: Color::Black, board: BoardId::B }, 178000u64),
        ];
        for (target, millis) in &targets {
            process_command(&mut state, &UbiCommand::Clock { target: *target, millis: *millis });
        }
        for (target, millis) in &targets {
            assert_eq!(state.clock(target), *millis);
        }
    }

    #[test]
    fn go_produces_teammsg_info_and_bestmove() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        assert_eq!(resp.len(), 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::A, .. }));
        assert!(matches!(&resp[2], UbiResponse::BestMove { board: BoardId::A, .. }));
    }

    #[test]
    fn go_bestmove_is_legal() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        let move_str = match &resp[2] {
            UbiResponse::BestMove { move_str, .. } => move_str.clone(),
            _ => panic!("expected BestMove"),
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
        // Use an empty-ish board where we block all regular moves but have reserves
        // Simpler: just set up a position with reserves and verify drops are in the count
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // Starting position has 20 regular moves. With a knight in reserve,
        // the knight can drop on all empty squares (32 squares in middle 4 ranks).
        // Total should be > 20
        if let UbiResponse::Info { nodes, .. } = &resp[1] {
            assert!(*nodes > 20, "expected drops to increase node count, got {}", nodes);
        } else {
            panic!("expected Info response");
        }
    }

    #[test]
    fn info_node_count() {
        let mut state = new_state();
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        // Starting position: 20 regular moves, 0 drops (empty reserves)
        if let UbiResponse::Info { nodes, .. } = &resp[1] {
            assert_eq!(*nodes, 20);
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

        // Set up both boards
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::B, fen: PositionSpec::StartPos, moves: vec![],
        });

        // Set clocks
        process_command(&mut state, &UbiCommand::Clock {
            target: ClockTarget { color: Color::White, board: BoardId::A },
            millis: 180000,
        });
        process_command(&mut state, &UbiCommand::Clock {
            target: ClockTarget { color: Color::Black, board: BoardId::A },
            millis: 180000,
        });

        // Go on board A
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        assert_eq!(resp.len(), 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::A, .. }));
        assert!(matches!(&resp[2], UbiResponse::BestMove { board: BoardId::A, .. }));

        // Go on board B
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        assert_eq!(resp.len(), 3);
        assert!(matches!(&resp[0], UbiResponse::TeamMsg(_)));
        assert!(matches!(&resp[1], UbiResponse::Info { board: BoardId::B, .. }));
        assert!(matches!(&resp[2], UbiResponse::BestMove { board: BoardId::B, .. }));

        // Quit
        let resp = process_command(&mut state, &UbiCommand::Quit);
        assert!(resp.is_empty());
    }

    #[test]
    fn bestmove_format_compliance() {
        let mut state = new_state();

        // Test regular move format (from starting position)
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::A });
        if let UbiResponse::BestMove { move_str, .. } = &resp[2] {
            // Regular UCI move: 4 chars like "e2e4" or 5 for promotion "e7e8q"
            assert!(move_str.len() >= 4, "move too short: {}", move_str);
            // Should not contain '@' (no reserves in starting position)
            assert!(!move_str.contains('@'), "unexpected drop in startpos: {}", move_str);
        }

        // Test drop move format
        // Position where a drop is clearly the best move: white has a queen in
        // reserve and an open board with few regular moves
        process_command(&mut state, &UbiCommand::Position {
            board: BoardId::B,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[Q] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
        let resp = process_command(&mut state, &UbiCommand::Go { board: BoardId::B });
        if let UbiResponse::BestMove { move_str, .. } = &resp[2] {
            if move_str.contains('@') {
                // Drop format: lowercase piece letter + @ + square
                assert_eq!(move_str.as_bytes()[0], b'q', "expected lowercase q, got {}", move_str);
                assert_eq!(move_str.as_bytes()[1], b'@');
            }
            // Either a drop or a regular move is fine — both formats are valid
            assert!(move_str.len() >= 3, "move too short: {}", move_str);
        }
    }

}
