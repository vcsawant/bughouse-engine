//! UBI (Universal Bughouse Interface) parsing and formatting.
//!
//! This module is pure data transformation — no I/O.
//! It converts between text lines and typed command/response enums.

use bughouse_chess::BughouseMove;

// ─── Types ───────────────────────────────────────────────────────────

/// Identifies one of the two bughouse boards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardId {
    A,
    B,
}

/// How a board's position is specified in a `position` command.
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
    /// Atomic position: both boards + all four clocks.
    /// Format: `position <bfen_a> | <bfen_b> clock <wA> <bA> <wB> <bB>`
    Position {
        board_a: PositionSpec,
        board_b: PositionSpec,
        clocks: [u64; 4],  // [white_A, black_A, white_B, black_B]
    },
    Go { board: BoardId },
    Stop { board: Option<BoardId> },
    PartnerMsg(String),
    /// Match metadata from the GUI (optional, for logging/analysis).
    /// Format: `metadata <key> <value>`
    Metadata { key: String, value: String },
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

/// Parse one line of stdin into a UbiCommand.
pub fn parse_command(line: &str) -> Result<UbiCommand, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty command".to_string());
    }

    // Determine the command keyword from the first token
    let keyword = trimmed.split_whitespace().next().unwrap();

    match keyword {
        "ubi" => Ok(UbiCommand::Ubi),
        "isready" => Ok(UbiCommand::IsReady),
        "ubinewgame" => Ok(UbiCommand::UbiNewGame),
        "quit" => Ok(UbiCommand::Quit),

        "setoption" => {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            parse_setoption(&tokens)
        }
        "position" => parse_position(trimmed),
        "go" => {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            parse_go(&tokens)
        }
        "stop" => {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            parse_stop(&tokens)
        }
        "partnermsg" => {
            let body = trimmed.strip_prefix("partnermsg").unwrap().trim().to_string();
            Ok(UbiCommand::PartnerMsg(body))
        }
        "metadata" => {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            if tokens.len() >= 3 {
                let key = tokens[1].to_string();
                let value = tokens[2..].join(" ");
                Ok(UbiCommand::Metadata { key, value })
            } else {
                Err("metadata: expected 'metadata <key> <value>'".to_string())
            }
        }

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

/// Parse a board spec string: either "startpos" or a 6-field BFEN string.
fn parse_board_spec(s: &str) -> Result<PositionSpec, String> {
    let trimmed = s.trim();
    if trimmed == "startpos" {
        Ok(PositionSpec::StartPos)
    } else {
        // Validate it has 6 space-separated fields
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != 6 {
            return Err(format!("position: expected 'startpos' or 6-field BFEN, got {} fields: '{}'", fields.len(), trimmed));
        }
        Ok(PositionSpec::Bfen(trimmed.to_string()))
    }
}

/// Parse: `position <bfen_a> | <bfen_b> clock <wA> <bA> <wB> <bB>`
///
/// Uses substring search rather than token splitting because BFEN strings
/// contain spaces. The `|` character never appears in valid BFEN, so
/// `find(" | ")` is unambiguous.
fn parse_position(line: &str) -> Result<UbiCommand, String> {
    // Strip "position " prefix
    let rest = line.strip_prefix("position ")
        .ok_or("position: missing command body")?;

    // Split at " | " to get board A spec and the remainder
    let pipe_pos = rest.find(" | ")
        .ok_or("position: missing ' | ' separator between boards")?;
    let board_a_str = &rest[..pipe_pos];
    let after_pipe = &rest[pipe_pos + 3..]; // skip " | "

    // Split at " clock " to get board B spec and clock values
    let clock_pos = after_pipe.find(" clock ")
        .ok_or("position: missing ' clock ' after board B")?;
    let board_b_str = &after_pipe[..clock_pos];
    let clock_str = &after_pipe[clock_pos + 7..]; // skip " clock "

    // Parse board specs
    let board_a = parse_board_spec(board_a_str)?;
    let board_b = parse_board_spec(board_b_str)?;

    // Parse 4 clock values
    let clock_tokens: Vec<&str> = clock_str.split_whitespace().collect();
    if clock_tokens.len() != 4 {
        return Err(format!("position: expected 4 clock values, got {}", clock_tokens.len()));
    }
    let mut clocks = [0u64; 4];
    for (i, tok) in clock_tokens.iter().enumerate() {
        clocks[i] = tok.parse::<u64>()
            .map_err(|e| format!("position: invalid clock value '{}': {}", tok, e))?;
    }

    Ok(UbiCommand::Position { board_a, board_b, clocks })
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
    fn parse_position_both_startpos() {
        let cmd = parse_command(
            "position startpos | startpos clock 180000 180000 180000 180000"
        ).unwrap();
        assert_eq!(cmd, UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::StartPos,
            clocks: [180000, 180000, 180000, 180000],
        });
    }

    #[test]
    fn parse_position_both_bfen() {
        let cmd = parse_command(
            "position rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 | rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1 clock 180000 175000 182000 178000"
        ).unwrap();
        assert_eq!(cmd, UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1".to_string()
            ),
            board_b: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1".to_string()
            ),
            clocks: [180000, 175000, 182000, 178000],
        });
    }

    #[test]
    fn parse_position_mixed_startpos_bfen() {
        let cmd = parse_command(
            "position startpos | rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1 clock 180000 180000 177000 180000"
        ).unwrap();
        assert_eq!(cmd, UbiCommand::Position {
            board_a: PositionSpec::StartPos,
            board_b: PositionSpec::Bfen(
                "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1".to_string()
            ),
            clocks: [180000, 180000, 177000, 180000],
        });
    }

    #[test]
    fn parse_position_with_reserves() {
        let cmd = parse_command(
            "position r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5 | rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3 clock 165000 168000 170000 172000"
        ).unwrap();
        assert_eq!(cmd, UbiCommand::Position {
            board_a: PositionSpec::Bfen(
                "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5".to_string()
            ),
            board_b: PositionSpec::Bfen(
                "rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3".to_string()
            ),
            clocks: [165000, 168000, 170000, 172000],
        });
    }

    #[test]
    fn parse_position_missing_pipe() {
        assert!(parse_command(
            "position startpos startpos clock 180000 180000 180000 180000"
        ).is_err());
    }

    #[test]
    fn parse_position_missing_clock() {
        assert!(parse_command(
            "position startpos | startpos 180000 180000 180000 180000"
        ).is_err());
    }

    #[test]
    fn parse_position_wrong_clock_count() {
        assert!(parse_command(
            "position startpos | startpos clock 180000 180000 180000"
        ).is_err());
    }

    #[test]
    fn parse_position_invalid_clock_value() {
        assert!(parse_command(
            "position startpos | startpos clock 180000 abc 180000 180000"
        ).is_err());
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
    fn parse_partnermsg() {
        let cmd = parse_command("partnermsg need n urgency high").unwrap();
        assert_eq!(cmd, UbiCommand::PartnerMsg("need n urgency high".to_string()));
    }

    #[test]
    fn parse_partnermsg_empty_body() {
        let cmd = parse_command("partnermsg").unwrap();
        assert_eq!(cmd, UbiCommand::PartnerMsg("".to_string()));
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
    fn format_bestmove_none() {
        let resp = UbiResponse::BestMove {
            board: BoardId::A,
            move_str: "(none)".to_string(),
        };
        assert_eq!(format_response(&resp), "bestmove board A (none)");
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
        let m = BughouseMove::Drop { piece: Piece::Pawn, square: Square::from_str("e4").unwrap() };
        assert_eq!(format_move(&m), "p@e4");
    }
}
