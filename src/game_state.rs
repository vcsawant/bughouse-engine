//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, RNG) and
//! `process_command()` which maps parsed BUP commands to responses.
//! No I/O — all output is returned as `Vec<BupResponse>`.

use std::time::Instant;

use bughouse_chess::{Board, BughouseMove, MoveGen};
use rand::seq::SliceRandom;

use crate::bup::{BoardId, BupCommand, BupResponse, ClockTarget, PositionSpec, format_move};

// ─── Engine State ────────────────────────────────────────────────────

pub struct EngineState {
    boards: [Option<Board>; 2],
    clocks: [u64; 4],  // white_A=0, black_A=1, white_B=2, black_B=3
    rng: rand::rngs::ThreadRng,
}

impl EngineState {
    pub fn new() -> Self {
        EngineState {
            boards: [None, None],
            clocks: [0; 4],
            rng: rand::thread_rng(),
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

/// Process a parsed BUP command and return zero or more responses.
pub fn process_command(state: &mut EngineState, cmd: &BupCommand) -> Vec<BupResponse> {
    match cmd {
        BupCommand::Bup => vec![
            BupResponse::IdName("BughouseEngine 0.1.0".to_string()),
            BupResponse::IdAuthor("Viren Sawant".to_string()),
            BupResponse::BupOk,
        ],

        BupCommand::IsReady => vec![BupResponse::ReadyOk],

        BupCommand::BupNewGame => {
            state.reset();
            vec![]
        }

        BupCommand::SetOption { .. } => vec![],

        BupCommand::Position { board, fen, moves } => {
            handle_position(state, *board, fen, moves);
            vec![]
        }

        BupCommand::Clock { target, millis } => {
            state.clocks[clock_index(target)] = *millis;
            vec![]
        }

        BupCommand::Go { board } => handle_go(state, *board),

        BupCommand::Stop { .. } => vec![],

        BupCommand::Quit => vec![],

        BupCommand::Unknown(line) => {
            eprintln!("bughouse-engine: unknown command: {}", line);
            vec![]
        }
    }
}

// ─── Position handling ───────────────────────────────────────────────

fn handle_position(state: &mut EngineState, board_id: BoardId, fen: &PositionSpec, moves: &[String]) {
    let mut board = match fen {
        PositionSpec::StartPos => Board::default(),
        PositionSpec::Bfen(s) => match s.parse::<Board>() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("bughouse-engine: invalid bfen: {}", e);
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
                        eprintln!("bughouse-engine: illegal drop: {}", move_str);
                    }
                }
            }
            Err(e) => {
                eprintln!("bughouse-engine: invalid move '{}': {}", move_str, e);
            }
        }
    }

    state.boards[board_index(board_id)] = Some(board);
}

// ─── Go handling (random move selection) ─────────────────────────────

fn handle_go(state: &mut EngineState, board_id: BoardId) -> Vec<BupResponse> {
    let board = match &state.boards[board_index(board_id)] {
        Some(b) => b,
        None => {
            eprintln!("bughouse-engine: go on unset board {:?}", board_id);
            return vec![];
        }
    };

    let start = Instant::now();

    // Collect all legal moves: regular board moves + drops from reserves
    let regular_moves: Vec<BughouseMove> = MoveGen::new_legal(board)
        .map(BughouseMove::Regular)
        .collect();
    let drop_moves = MoveGen::drop_moves(board);

    let mut combined: Vec<BughouseMove> = Vec::with_capacity(regular_moves.len() + drop_moves.len());
    combined.extend(regular_moves);
    combined.extend(drop_moves);

    if combined.is_empty() {
        eprintln!("bughouse-engine: no legal moves on board {:?}", board_id);
        return vec![];
    }

    let chosen = combined.choose(&mut state.rng).unwrap();
    let elapsed_ms = start.elapsed().as_millis() as u64;

    vec![
        BupResponse::Info {
            board: board_id,
            depth: 0,
            nodes: combined.len(),
            time_ms: elapsed_ms,
            score_cp: 0,
        },
        BupResponse::BestMove {
            board: board_id,
            move_str: format_move(chosen),
        },
    ]
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bup::{BoardId, BupCommand, BupResponse, ClockTarget, PositionSpec};
    use bughouse_chess::{Color, Piece, Square};
    use std::str::FromStr;

    fn new_state() -> EngineState {
        EngineState::new()
    }

    #[test]
    fn handshake_flow() {
        let mut state = new_state();
        let resp = process_command(&mut state, &BupCommand::Bup);
        assert_eq!(resp.len(), 3);
        assert!(matches!(&resp[0], BupResponse::IdName(n) if n.contains("BughouseEngine")));
        assert!(matches!(&resp[1], BupResponse::IdAuthor(_)));
        assert_eq!(resp[2], BupResponse::BupOk);
    }

    #[test]
    fn isready_readyok() {
        let mut state = new_state();
        let resp = process_command(&mut state, &BupCommand::IsReady);
        assert_eq!(resp, vec![BupResponse::ReadyOk]);
    }

    #[test]
    fn bupnewgame_resets() {
        let mut state = new_state();
        // Set up some state
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        state.clocks = [100, 200, 300, 400];

        // Reset
        process_command(&mut state, &BupCommand::BupNewGame);
        assert!(state.board(BoardId::A).is_none());
        assert!(state.board(BoardId::B).is_none());
        assert_eq!(state.clocks, [0; 4]);
    }

    #[test]
    fn position_startpos() {
        let mut state = new_state();
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let board = state.board(BoardId::A).unwrap();
        assert_eq!(*board, Board::default());
    }

    #[test]
    fn position_bfen_with_reserves() {
        let mut state = new_state();
        process_command(&mut state, &BupCommand::Position {
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
        process_command(&mut state, &BupCommand::Position {
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
        process_command(&mut state, &BupCommand::Position {
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
            process_command(&mut state, &BupCommand::Clock { target: *target, millis: *millis });
        }
        for (target, millis) in &targets {
            assert_eq!(state.clock(target), *millis);
        }
    }

    #[test]
    fn go_produces_info_and_bestmove() {
        let mut state = new_state();
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        assert_eq!(resp.len(), 2);
        assert!(matches!(&resp[0], BupResponse::Info { board: BoardId::A, .. }));
        assert!(matches!(&resp[1], BupResponse::BestMove { board: BoardId::A, .. }));
    }

    #[test]
    fn go_bestmove_is_legal() {
        let mut state = new_state();
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        let move_str = match &resp[1] {
            BupResponse::BestMove { move_str, .. } => move_str.clone(),
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
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::B });
        assert!(resp.is_empty());
    }

    #[test]
    fn go_includes_drops() {
        let mut state = new_state();
        // Position where white has pieces in reserve — drops should be possible
        // Use an empty-ish board where we block all regular moves but have reserves
        // Simpler: just set up a position with reserves and verify drops are in the count
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[N] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        // Starting position has 20 regular moves. With a knight in reserve,
        // the knight can drop on all empty squares (32 squares in middle 4 ranks).
        // Total should be > 20
        if let BupResponse::Info { nodes, .. } = &resp[0] {
            assert!(*nodes > 20, "expected drops to increase node count, got {}", nodes);
        } else {
            panic!("expected Info response");
        }
    }

    #[test]
    fn info_node_count() {
        let mut state = new_state();
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        // Starting position: 20 regular moves, 0 drops (empty reserves)
        if let BupResponse::Info { nodes, .. } = &resp[0] {
            assert_eq!(*nodes, 20);
        } else {
            panic!("expected Info response");
        }
    }

    #[test]
    fn setoption_silent() {
        let mut state = new_state();
        let resp = process_command(&mut state, &BupCommand::SetOption {
            name: "Hash".to_string(),
            value: Some("256".to_string()),
        });
        assert!(resp.is_empty());
    }

    #[test]
    fn stop_returns_empty() {
        let mut state = new_state();
        let resp = process_command(&mut state, &BupCommand::Stop { board: None });
        assert!(resp.is_empty());
    }

    #[test]
    fn unknown_returns_empty() {
        let mut state = new_state();
        let resp = process_command(&mut state, &BupCommand::Unknown("garbage".to_string()));
        assert!(resp.is_empty());
    }

    #[test]
    fn multi_command_session() {
        let mut state = new_state();

        // Handshake
        let resp = process_command(&mut state, &BupCommand::Bup);
        assert_eq!(resp.len(), 3);

        // Ready check
        let resp = process_command(&mut state, &BupCommand::IsReady);
        assert_eq!(resp.len(), 1);

        // New game
        let resp = process_command(&mut state, &BupCommand::BupNewGame);
        assert!(resp.is_empty());

        // Set up both boards
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::B, fen: PositionSpec::StartPos, moves: vec![],
        });

        // Set clocks
        process_command(&mut state, &BupCommand::Clock {
            target: ClockTarget { color: Color::White, board: BoardId::A },
            millis: 180000,
        });
        process_command(&mut state, &BupCommand::Clock {
            target: ClockTarget { color: Color::Black, board: BoardId::A },
            millis: 180000,
        });

        // Go on board A
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        assert_eq!(resp.len(), 2);
        assert!(matches!(&resp[0], BupResponse::Info { board: BoardId::A, .. }));
        assert!(matches!(&resp[1], BupResponse::BestMove { board: BoardId::A, .. }));

        // Go on board B
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::B });
        assert_eq!(resp.len(), 2);
        assert!(matches!(&resp[0], BupResponse::Info { board: BoardId::B, .. }));
        assert!(matches!(&resp[1], BupResponse::BestMove { board: BoardId::B, .. }));

        // Quit
        let resp = process_command(&mut state, &BupCommand::Quit);
        assert!(resp.is_empty());
    }

    #[test]
    fn bestmove_format_compliance() {
        let mut state = new_state();

        // Test regular move format (from starting position)
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::A, fen: PositionSpec::StartPos, moves: vec![],
        });
        let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::A });
        if let BupResponse::BestMove { move_str, .. } = &resp[1] {
            // Regular UCI move: 4 chars like "e2e4" or 5 for promotion "e7e8q"
            assert!(move_str.len() >= 4, "move too short: {}", move_str);
            // Should not contain '@' (no reserves in starting position)
            assert!(!move_str.contains('@'), "unexpected drop in startpos: {}", move_str);
        }

        // Test drop move format
        // Position where only drops are likely (give white a queen in reserve)
        process_command(&mut state, &BupCommand::Position {
            board: BoardId::B,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[Q] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
        // Run go many times — at least one should be a drop
        let mut found_drop = false;
        for _ in 0..50 {
            let resp = process_command(&mut state, &BupCommand::Go { board: BoardId::B });
            if let BupResponse::BestMove { move_str, .. } = &resp[1] {
                if move_str.contains('@') {
                    // Drop format: lowercase piece letter + @ + square
                    assert_eq!(move_str.as_bytes()[0], b'q', "expected lowercase q, got {}", move_str);
                    assert_eq!(move_str.as_bytes()[1], b'@');
                    found_drop = true;
                    break;
                }
            }
        }
        assert!(found_drop, "expected at least one drop move in 50 iterations");
    }

}
