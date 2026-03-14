mod ubi;
mod game_state;
mod strategy;
mod scoring;
mod search;
mod time;

use std::io::{self, BufRead, BufWriter, Write};
use std::fs::File;

use clap::Parser;
use log::{info, warn};
use simplelog::*;

use ubi::{parse_command, format_response, UbiCommand};
use game_state::{EngineState, process_command};

#[derive(Parser)]
#[command(name = "bughouse_engine", about = "Bughouse chess engine (UBI protocol)")]
struct Args {
    /// Path to log file. If omitted, no logging is performed.
    #[arg(long)]
    log_file: Option<String>,

    /// Game ID for log context (passed by the game server).
    #[arg(long)]
    game_id: Option<String>,
}

fn main() {
    let args = Args::parse();

    let game_id = args.game_id.unwrap_or_else(|| "standalone".to_string());

    // Initialize file logger if --log-file was provided
    if let Some(ref log_path) = args.log_file {
        match File::create(log_path) {
            Ok(file) => {
                let config = ConfigBuilder::new()
                    .set_time_format_rfc3339()
                    .build();
                WriteLogger::init(LevelFilter::Debug, config, file).ok();
            }
            Err(e) => {
                eprintln!("bughouse-engine: failed to open log file '{}': {}", log_path, e);
            }
        }
    }

    info!("[game:{}] Engine started", game_id);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut state = EngineState::new(game_id);

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!("[game:{}] stdin error: {}", state.game_id, e);
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        info!("[game:{}] << {}", state.game_id, trimmed);

        let cmd = match parse_command(trimmed) {
            Ok(c) => c,
            Err(e) => {
                warn!("[game:{}] parse error: {}", state.game_id, e);
                continue;
            }
        };

        let is_quit = matches!(cmd, UbiCommand::Quit);
        let responses = process_command(&mut state, &cmd);
        for resp in &responses {
            let resp_line = format_response(resp);
            info!("[game:{}] >> {}", state.game_id, resp_line);
            writeln!(out, "{}", resp_line).ok();
        }
        out.flush().ok();

        if is_quit {
            break;
        }
    }

    info!("[game:{}] Engine shutting down", state.game_id);
}
