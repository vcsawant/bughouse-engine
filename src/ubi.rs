//! UBI (Universal Bughouse Interface) parsing and formatting.
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

/// A parsed UBI command (GUI → Engine).
#[derive(Debug, Clone, PartialEq)]
pub enum UbiCommand {
    Ubi,
    IsReady,
    UbiNewGame,
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
pub enum UbiResponse {
    IdName(String),
    IdAuthor(String),
    UbiOk,
    ReadyOk,
    Info { board: BoardId, depth: u32, nodes: usize, time_ms: u64, score_cp: i32, pv: Vec<String> },
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

/// Parse one line of stdin into a UbiCommand.
pub fn parse_command(line: &str) -> Result<UbiCommand, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("empty command".to_string());
    }

    match tokens[0] {
        "ubi" => Ok(UbiCommand::Ubi),
        "isready" => Ok(UbiCommand::IsReady),
        "ubinewgame" => Ok(UbiCommand::UbiNewGame),
        "quit" => Ok(UbiCommand::Quit),

        "setoption" => parse_setoption(&tokens),
        "position" => parse_position(&tokens),
        "clock" => parse_clock(&tokens),
        "go" => parse_go(&tokens),
        "stop" => parse_stop(&tokens),

        _ => Ok(UbiCommand::Unknown(line.to_string())),
    }
}

/// Parse: `setoption name <id> [value <x>]`
fn parse_setoption(tokens: &[&str]) -> Result<UbiCommand, String> {
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

    Ok(UbiCommand::SetOption { name, value })
}

/// Parse: `position board <A|B> <startpos|bfen <6-field-string>> [moves <move1> ...]`
fn parse_position(tokens: &[&str]) -> Result<UbiCommand, String> {
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
            Ok(UbiCommand::Position { board, fen: PositionSpec::StartPos, moves })
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
            Ok(UbiCommand::Position { board, fen: PositionSpec::Bfen(bfen), moves })
        }
        other => Err(format!("position: expected 'startpos' or 'bfen', got '{}'", other)),
    }
}

/// Parse: `clock <color_board> <milliseconds>`
fn parse_clock(tokens: &[&str]) -> Result<UbiCommand, String> {
    if tokens.len() < 3 {
        return Err("clock: expected <target> <millis>".to_string());
    }
    let target = parse_clock_target(tokens[1])?;
    let millis = tokens[2].parse::<u64>()
        .map_err(|e| format!("clock: invalid millis: {}", e))?;
    Ok(UbiCommand::Clock { target, millis })
}

/// Parse: `go board <A|B> [ignored params]`
fn parse_go(tokens: &[&str]) -> Result<UbiCommand, String> {
    if tokens.len() < 3 || tokens[1] != "board" {
        return Err("go: expected 'board <A|B>'".to_string());
    }
    let board = parse_board_id(tokens[2])?;
    // Search params are ignored in Phase B
    Ok(UbiCommand::Go { board })
}

/// Parse: `stop [board <A|B>]`
fn parse_stop(tokens: &[&str]) -> Result<UbiCommand, String> {
    if tokens.len() >= 3 && tokens[1] == "board" {
        let board = parse_board_id(tokens[2])?;
        Ok(UbiCommand::Stop { board: Some(board) })
    } else {
        Ok(UbiCommand::Stop { board: None })
    }
}

// ─── Formatting ──────────────────────────────────────────────────────

/// Format a UbiResponse into the exact stdout line (no trailing newline).
pub fn format_response(resp: &UbiResponse) -> String {
    match resp {
        UbiResponse::IdName(name) => format!("id name {}", name),
        UbiResponse::IdAuthor(author) => format!("id author {}", author),
        UbiResponse::UbiOk => "ubiok".to_string(),
        UbiResponse::ReadyOk => "readyok".to_string(),
        UbiResponse::Info { board, depth, nodes, time_ms, score_cp, pv } => {
            let board_str = match board { BoardId::A => "A", BoardId::B => "B" };
            let mut s = format!("info board {} depth {} nodes {} time {} score cp {}",
                board_str, depth, nodes, time_ms, score_cp);
            if !pv.is_empty() {
                s.push_str(" pv ");
                s.push_str(&pv.join(" "));
            }
            s
        }
        UbiResponse::BestMove { board, move_str } => {
            let board_str = match board { BoardId::A => "A", BoardId::B => "B" };
            format!("bestmove board {} {}", board_str, move_str)
        }
        UbiResponse::TeamMsg(msg) => format!("teammsg {}", msg),
    }
}

/// Format a BughouseMove for UBI output.
/// Delegates to BughouseMove::Display which is already UBI-compliant
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
    fn parse_ubi() {
        assert_eq!(parse_command("ubi").unwrap(), UbiCommand::Ubi);
    }

    #[test]
    fn parse_isready() {
        assert_eq!(parse_command("isready").unwrap(), UbiCommand::IsReady);
    }

    #[test]
    fn parse_ubinewgame() {
        assert_eq!(parse_command("ubinewgame").unwrap(), UbiCommand::UbiNewGame);
    }

    #[test]
    fn parse_quit() {
        assert_eq!(parse_command("quit").unwrap(), UbiCommand::Quit);
    }

    #[test]
    fn parse_setoption_with_value() {
        let cmd = parse_command("setoption name Hash value 256").unwrap();
        assert_eq!(cmd, UbiCommand::SetOption {
            name: "Hash".to_string(),
            value: Some("256".to_string()),
        });
    }

    #[test]
    fn parse_setoption_no_value() {
        let cmd = parse_command("setoption name Clear Hash").unwrap();
        assert_eq!(cmd, UbiCommand::SetOption {
            name: "Clear Hash".to_string(),
            value: None,
        });
    }

    #[test]
    fn parse_position_startpos() {
        let cmd = parse_command("position board A startpos").unwrap();
        assert_eq!(cmd, UbiCommand::Position {
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
        assert_eq!(cmd, UbiCommand::Position {
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
        assert_eq!(cmd, UbiCommand::Position {
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
        assert_eq!(cmd, UbiCommand::Position {
            board: BoardId::A,
            fen: PositionSpec::StartPos,
            moves: vec!["e2e4".to_string(), "n@f3".to_string()],
        });
    }

    #[test]
    fn parse_clock_white_a() {
        let cmd = parse_command("clock white_A 180000").unwrap();
        assert_eq!(cmd, UbiCommand::Clock {
            target: ClockTarget { color: Color::White, board: BoardId::A },
            millis: 180000,
        });
    }

    #[test]
    fn parse_clock_black_b() {
        let cmd = parse_command("clock black_B 175000").unwrap();
        assert_eq!(cmd, UbiCommand::Clock {
            target: ClockTarget { color: Color::Black, board: BoardId::B },
            millis: 175000,
        });
    }

    #[test]
    fn parse_go() {
        let cmd = parse_command("go board A").unwrap();
        assert_eq!(cmd, UbiCommand::Go { board: BoardId::A });
    }

    #[test]
    fn parse_go_with_params() {
        // Extra search params are ignored in Phase B
        let cmd = parse_command("go board B movetime 5000").unwrap();
        assert_eq!(cmd, UbiCommand::Go { board: BoardId::B });
    }

    #[test]
    fn parse_stop() {
        let cmd = parse_command("stop").unwrap();
        assert_eq!(cmd, UbiCommand::Stop { board: None });
    }

    #[test]
    fn parse_stop_board() {
        let cmd = parse_command("stop board A").unwrap();
        assert_eq!(cmd, UbiCommand::Stop { board: Some(BoardId::A) });
    }

    #[test]
    fn parse_unknown() {
        let cmd = parse_command("garbage xyz").unwrap();
        assert_eq!(cmd, UbiCommand::Unknown("garbage xyz".to_string()));
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
        let resp = UbiResponse::IdName("Foo".to_string());
        assert_eq!(format_response(&resp), "id name Foo");
    }

    #[test]
    fn format_ubiok() {
        assert_eq!(format_response(&UbiResponse::UbiOk), "ubiok");
    }

    #[test]
    fn format_readyok() {
        assert_eq!(format_response(&UbiResponse::ReadyOk), "readyok");
    }

    #[test]
    fn format_info() {
        let resp = UbiResponse::Info {
            board: BoardId::A,
            depth: 12,
            nodes: 150000,
            time_ms: 2000,
            score_cp: 45,
            pv: vec!["e2e4".into()],
        };
        assert_eq!(
            format_response(&resp),
            "info board A depth 12 nodes 150000 time 2000 score cp 45 pv e2e4"
        );
    }

    #[test]
    fn format_info_empty_pv() {
        let resp = UbiResponse::Info {
            board: BoardId::B,
            depth: 1,
            nodes: 20,
            time_ms: 0,
            score_cp: -10,
            pv: vec![],
        };
        assert_eq!(
            format_response(&resp),
            "info board B depth 1 nodes 20 time 0 score cp -10"
        );
    }

    #[test]
    fn format_bestmove_regular() {
        let resp = UbiResponse::BestMove {
            board: BoardId::A,
            move_str: "e2e4".to_string(),
        };
        assert_eq!(format_response(&resp), "bestmove board A e2e4");
    }

    #[test]
    fn format_bestmove_drop() {
        let resp = UbiResponse::BestMove {
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
