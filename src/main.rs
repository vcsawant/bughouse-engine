mod bup;
mod game_state;

use std::io::{self, BufRead, BufWriter, Write};

use bup::{parse_command, format_response, BupCommand};
use game_state::{EngineState, process_command};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut state = EngineState::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("bughouse-engine: stdin error: {}", e);
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let cmd = match parse_command(trimmed) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("bughouse-engine: parse error: {}", e);
                continue;
            }
        };

        let is_quit = matches!(cmd, BupCommand::Quit);
        let responses = process_command(&mut state, &cmd);
        for resp in &responses {
            writeln!(out, "{}", format_response(resp)).ok();
        }
        out.flush().ok();

        if is_quit {
            break;
        }
    }
}
