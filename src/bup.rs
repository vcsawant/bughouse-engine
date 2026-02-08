//! BUP (Bughouse Universal Protocol) parsing and formatting.
//!
//! This module is pure data transformation — no I/O.
//! It converts between text lines and typed command/response enums.

use bughouse_chess::{BughouseMove, Color};

// ─── Types ───────────────────────────────────────────────────────────

/// Identifies one of the two bughouse boards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardId {
    A,
    B,
}

/// Identifies a specific player's clock (color + board).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTarget {
    pub color: Color,
    pub board: BoardId,
}

/// How the position is specified in a `position` command.
#[derive(Debug, Clone, PartialEq)]
pub enum PositionSpec {
    StartPos,
    Bfen(String),
}

/// A parsed BUP command (GUI → Engine).
#[derive(Debug, Clone, PartialEq)]
pub enum BupCommand {
    Bup,
    IsReady,
    BupNewGame,
    SetOption { name: String, value: Option<String> },
    Position { board: BoardId, fen: PositionSpec, moves: Vec<String> },
    Clock { target: ClockTarget, millis: u64 },
    Go { board: BoardId },
    Stop { board: Option<BoardId> },
    Quit,
    Unknown(String),
}

/// A response the engine sends back (Engine → GUI).
#[derive(Debug, Clone, PartialEq)]
pub enum BupResponse {
    IdName(String),
    IdAuthor(String),
    BupOk,
    ReadyOk,
    Info { board: BoardId, depth: u32, nodes: usize, time_ms: u64, score_cp: i32 },
    BestMove { board: BoardId, move_str: String },
    TeamMsg(String),
}

// ─── Parsing ─────────────────────────────────────────────────────────

/// Parse a board identifier token ("A" or "B").
fn parse_board_id(s: &str) -> Result<BoardId, String> {
    match s {
        "A" => Ok(BoardId::A),
        "B" => Ok(BoardId::B),
        _ => Err(format!("invalid board id: {}", s)),
    }
}

/// Parse a clock target token like "white_A" or "black_B".
fn parse_clock_target(s: &str) -> Result<ClockTarget, String> {
    match s {
        "white_A" => Ok(ClockTarget { color: Color::White, board: BoardId::A }),
        "black_A" => Ok(ClockTarget { color: Color::Black, board: BoardId::A }),
        "white_B" => Ok(ClockTarget { color: Color::White, board: BoardId::B }),
        "black_B" => Ok(ClockTarget { color: Color::Black, board: BoardId::B }),
        _ => Err(format!("invalid clock target: {}", s)),
    }
}

/// Parse one line of stdin into a BupCommand.
pub fn parse_command(line: &str) -> Result<BupCommand, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("empty command".to_string());
    }

    match tokens[0] {
        "bup" => Ok(BupCommand::Bup),
        "isready" => Ok(BupCommand::IsReady),
        "bupnewgame" => Ok(BupCommand::BupNewGame),
        "quit" => Ok(BupCommand::Quit),

        "setoption" => parse_setoption(&tokens),
        "position" => parse_position(&tokens),
        "clock" => parse_clock(&tokens),
        "go" => parse_go(&tokens),
        "stop" => parse_stop(&tokens),

        _ => Ok(BupCommand::Unknown(line.to_string())),
    }
}

/// Parse: `setoption name <id> [value <x>]`
fn parse_setoption(tokens: &[&str]) -> Result<BupCommand, String> {
    // Find "name" keyword
    let name_idx = tokens.iter().position(|t| *t == "name")
        .ok_or("setoption: missing 'name' keyword")?;

    // Find "value" keyword (if present)
    let value_idx = tokens.iter().position(|t| *t == "value");

    let name = match value_idx {
        Some(vi) => tokens[name_idx + 1..vi].join(" "),
        None => tokens[name_idx + 1..].join(" "),
    };

    let value = value_idx.map(|vi| tokens[vi + 1..].join(" "));

    Ok(BupCommand::SetOption { name, value })
}

/// Parse: `position board <A|B> <startpos|bfen <6-field-string>> [moves <move1> ...]`
fn parse_position(tokens: &[&str]) -> Result<BupCommand, String> {
    // tokens[0] = "position", tokens[1] = "board", tokens[2] = board id
    if tokens.len() < 4 || tokens[1] != "board" {
        return Err("position: expected 'board <A|B>'".to_string());
    }
    let board = parse_board_id(tokens[2])?;

    match tokens[3] {
        "startpos" => {
            // Check for optional "moves" section
            let moves = if tokens.len() > 4 && tokens[4] == "moves" {
                tokens[5..].iter().map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };
            Ok(BupCommand::Position { board, fen: PositionSpec::StartPos, moves })
        }
        "bfen" => {
            // BFEN has exactly 6 space-separated fields
            if tokens.len() < 10 {
                return Err("position bfen: expected 6 fields".to_string());
            }
            let bfen = tokens[4..10].join(" ");

            // Check for optional "moves" section after the 6 BFEN fields
            let moves = if tokens.len() > 10 && tokens[10] == "moves" {
                tokens[11..].iter().map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };
            Ok(BupCommand::Position { board, fen: PositionSpec::Bfen(bfen), moves })
        }
        other => Err(format!("position: expected 'startpos' or 'bfen', got '{}'", other)),
    }
}

/// Parse: `clock <color_board> <milliseconds>`
fn parse_clock(tokens: &[&str]) -> Result<BupCommand, String> {
    if tokens.len() < 3 {
        return Err("clock: expected <target> <millis>".to_string());
    }
    let target = parse_clock_target(tokens[1])?;
    let millis = tokens[2].parse::<u64>()
        .map_err(|e| format!("clock: invalid millis: {}", e))?;
    Ok(BupCommand::Clock { target, millis })
}

/// Parse: `go board <A|B> [ignored params]`
fn parse_go(tokens: &[&str]) -> Result<BupCommand, String> {
    if tokens.len() < 3 || tokens[1] != "board" {
        return Err("go: expected 'board <A|B>'".to_string());
    }
    let board = parse_board_id(tokens[2])?;
    // Search params are ignored in Phase B
    Ok(BupCommand::Go { board })
}

/// Parse: `stop [board <A|B>]`
fn parse_stop(tokens: &[&str]) -> Result<BupCommand, String> {
    if tokens.len() >= 3 && tokens[1] == "board" {
        let board = parse_board_id(tokens[2])?;
        Ok(BupCommand::Stop { board: Some(board) })
    } else {
        Ok(BupCommand::Stop { board: None })
    }
}

// ─── Formatting ──────────────────────────────────────────────────────

/// Format a BupResponse into the exact stdout line (no trailing newline).
pub fn format_response(resp: &BupResponse) -> String {
    match resp {
        BupResponse::IdName(name) => format!("id name {}", name),
        BupResponse::IdAuthor(author) => format!("id author {}", author),
        BupResponse::BupOk => "bupok".to_string(),
        BupResponse::ReadyOk => "readyok".to_string(),
        BupResponse::Info { board, depth, nodes, time_ms, score_cp } => {
            let board_str = match board { BoardId::A => "A", BoardId::B => "B" };
            format!("info board {} depth {} nodes {} time {} score cp {}",
                board_str, depth, nodes, time_ms, score_cp)
        }
        BupResponse::BestMove { board, move_str } => {
            let board_str = match board { BoardId::A => "A", BoardId::B => "B" };
            format!("bestmove board {} {}", board_str, move_str)
        }
        BupResponse::TeamMsg(msg) => format!("teammsg {}", msg),
    }
}

/// Format a BughouseMove for BUP output.
/// Delegates to BughouseMove::Display which is already BUP-compliant
/// (regular moves as "e2e4", drops as "p@e4" lowercase).
pub fn format_move(m: &BughouseMove) -> String {
    format!("{}", m)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bughouse_chess::{Piece, Square};
    use std::str::FromStr;

    // --- Parsing tests ---

    #[test]
    fn parse_bup() {
        assert_eq!(parse_command("bup").unwrap(), BupCommand::Bup);
    }

    #[test]
    fn parse_isready() {
        assert_eq!(parse_command("isready").unwrap(), BupCommand::IsReady);
    }

    #[test]
    fn parse_bupnewgame() {
        assert_eq!(parse_command("bupnewgame").unwrap(), BupCommand::BupNewGame);
    }

    #[test]
    fn parse_quit() {
        assert_eq!(parse_command("quit").unwrap(), BupCommand::Quit);
    }

    #[test]
    fn parse_setoption_with_value() {
        let cmd = parse_command("setoption name Hash value 256").unwrap();
        assert_eq!(cmd, BupCommand::SetOption {
            name: "Hash".to_string(),
            value: Some("256".to_string()),
        });
    }

    #[test]
    fn parse_setoption_no_value() {
        let cmd = parse_command("setoption name Clear Hash").unwrap();
        assert_eq!(cmd, BupCommand::SetOption {
            name: "Clear Hash".to_string(),
            value: None,
        });
    }

    #[test]
    fn parse_position_startpos() {
        let cmd = parse_command("position board A startpos").unwrap();
        assert_eq!(cmd, BupCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::StartPos,
            moves: vec![],
        });
    }

    #[test]
    fn parse_position_bfen() {
        let cmd = parse_command(
            "position board B bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1"
        ).unwrap();
        assert_eq!(cmd, BupCommand::Position {
            board: BoardId::B,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1".to_string()
            ),
            moves: vec![],
        });
    }

    #[test]
    fn parse_position_bfen_with_moves() {
        let cmd = parse_command(
            "position board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 moves e2e4 d7d5"
        ).unwrap();
        assert_eq!(cmd, BupCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1".to_string()
            ),
            moves: vec!["e2e4".to_string(), "d7d5".to_string()],
        });
    }

    #[test]
    fn parse_position_startpos_with_drop() {
        let cmd = parse_command("position board A startpos moves e2e4 n@f3").unwrap();
        assert_eq!(cmd, BupCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::StartPos,
            moves: vec!["e2e4".to_string(), "n@f3".to_string()],
        });
    }

    #[test]
    fn parse_clock_white_a() {
        let cmd = parse_command("clock white_A 180000").unwrap();
        assert_eq!(cmd, BupCommand::Clock {
            target: ClockTarget { color: Color::White, board: BoardId::A },
            millis: 180000,
        });
    }

    #[test]
    fn parse_clock_black_b() {
        let cmd = parse_command("clock black_B 175000").unwrap();
        assert_eq!(cmd, BupCommand::Clock {
            target: ClockTarget { color: Color::Black, board: BoardId::B },
            millis: 175000,
        });
    }

    #[test]
    fn parse_go() {
        let cmd = parse_command("go board A").unwrap();
        assert_eq!(cmd, BupCommand::Go { board: BoardId::A });
    }

    #[test]
    fn parse_go_with_params() {
        // Extra search params are ignored in Phase B
        let cmd = parse_command("go board B movetime 5000").unwrap();
        assert_eq!(cmd, BupCommand::Go { board: BoardId::B });
    }

    #[test]
    fn parse_stop() {
        let cmd = parse_command("stop").unwrap();
        assert_eq!(cmd, BupCommand::Stop { board: None });
    }

    #[test]
    fn parse_stop_board() {
        let cmd = parse_command("stop board A").unwrap();
        assert_eq!(cmd, BupCommand::Stop { board: Some(BoardId::A) });
    }

    #[test]
    fn parse_unknown() {
        let cmd = parse_command("garbage xyz").unwrap();
        assert_eq!(cmd, BupCommand::Unknown("garbage xyz".to_string()));
    }

    #[test]
    fn parse_empty() {
        assert!(parse_command("").is_err());
    }

    #[test]
    fn parse_invalid_board_id() {
        assert!(parse_command(
            "position board C bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1"
        ).is_err());
    }

    #[test]
    fn parse_invalid_clock() {
        assert!(parse_command("clock purple_Z 100").is_err());
    }

    // --- Formatting tests ---

    #[test]
    fn format_id_name() {
        let resp = BupResponse::IdName("Foo".to_string());
        assert_eq!(format_response(&resp), "id name Foo");
    }

    #[test]
    fn format_bupok() {
        assert_eq!(format_response(&BupResponse::BupOk), "bupok");
    }

    #[test]
    fn format_readyok() {
        assert_eq!(format_response(&BupResponse::ReadyOk), "readyok");
    }

    #[test]
    fn format_info() {
        let resp = BupResponse::Info {
            board: BoardId::A,
            depth: 12,
            nodes: 150000,
            time_ms: 2000,
            score_cp: 45,
        };
        assert_eq!(
            format_response(&resp),
            "info board A depth 12 nodes 150000 time 2000 score cp 45"
        );
    }

    #[test]
    fn format_bestmove_regular() {
        let resp = BupResponse::BestMove {
            board: BoardId::A,
            move_str: "e2e4".to_string(),
        };
        assert_eq!(format_response(&resp), "bestmove board A e2e4");
    }

    #[test]
    fn format_bestmove_drop() {
        let resp = BupResponse::BestMove {
            board: BoardId::B,
            move_str: "n@f3".to_string(),
        };
        assert_eq!(format_response(&resp), "bestmove board B n@f3");
    }

    #[test]
    fn format_move_regular() {
        use bughouse_chess::ChessMove;
        let from = Square::from_str("e2").unwrap();
        let to = Square::from_str("e4").unwrap();
        let m = BughouseMove::Regular(ChessMove::new(from, to, None));
        assert_eq!(format_move(&m), "e2e4");
    }

    #[test]
    fn format_move_drop() {
        let m = BughouseMove::drop_piece(Piece::Pawn, Square::from_str("e4").unwrap());
        assert_eq!(format_move(&m), "p@e4");
    }
}
