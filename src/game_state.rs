//! Engine state and command dispatch.
//!
//! Contains the `EngineState` struct (two boards, four clocks, eval threads) and
//! `process_command()` which maps parsed UBI commands to responses.
//! No I/O — all output is returned as `Vec<UbiResponse>`.

use bughouse_chess::{Board, CacheTable, Color, Piece, NUM_NON_KING_PIECES};
use log::{info, warn, debug};
use std::time::Instant;

use crate::book::OpeningBook;
use crate::engine::{self, EvalCommand, EvalHandle};
use crate::search::{self, BoardEval, TTEntry, TT_DEFAULT, TT_DEFAULT_SIZE};
use crate::strategy;
use crate::ubi::{BoardId, UbiCommand, UbiResponse, PositionSpec, format_move};

// ─── Partner State ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Urgency { Low, Medium, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreatLevel { Low, Medium, High, Critical }

/// Parsed state from partner's messages. Updated on each incoming partnermsg.
#[derive(Debug, Clone)]
struct PartnerState {
    needed_piece: Option<(Piece, Urgency)>,
    threat_level: Option<ThreatLevel>,
    material: Option<i32>,
    play_fast: bool,
    stall: bool,
}

impl Default for PartnerState {
    fn default() -> Self {
        PartnerState {
            needed_piece: None,
            threat_level: None,
            material: None,
            play_fast: false,
            stall: false,
        }
    }
}

impl PartnerState {
    fn parse_message(&mut self, msg: &str) {
        let parts: Vec<&str> = msg.split_whitespace().collect();
        if parts.is_empty() { return; }

        match parts[0] {
            "need" => {
                if let Some(piece) = parts.get(1).and_then(|p| parse_piece_char(p)) {
                    let urgency = if parts.get(2) == Some(&"urgency") {
                        match parts.get(3).copied() {
                            Some("high") => Urgency::High,
                            Some("medium") => Urgency::Medium,
                            _ => Urgency::Low,
                        }
                    } else {
                        Urgency::Medium // default urgency
                    };
                    self.needed_piece = Some((piece, urgency));
                }
            }
            "threat" => {
                self.threat_level = match parts.get(1).copied() {
                    Some("critical") => Some(ThreatLevel::Critical),
                    Some("high") => Some(ThreatLevel::High),
                    Some("medium") => Some(ThreatLevel::Medium),
                    Some("low") => Some(ThreatLevel::Low),
                    _ => None,
                };
            }
            "material" => {
                if let Some(val) = parts.get(1).and_then(|s| s.parse::<i32>().ok()) {
                    self.material = Some(val);
                }
            }
            "play_fast" => {
                self.play_fast = true;
            }
            "stall" => {
                self.stall = true;
            }
            _ => {}
        }
    }
}

fn parse_piece_char(s: &str) -> Option<Piece> {
    match s {
        "p" | "P" => Some(Piece::Pawn),
        "n" | "N" => Some(Piece::Knight),
        "b" | "B" => Some(Piece::Bishop),
        "r" | "R" => Some(Piece::Rook),
        "q" | "Q" => Some(Piece::Queen),
        _ => None,
    }
}

fn piece_to_msg_char(p: Piece) -> &'static str {
    match p {
        Piece::Pawn => "p",
        Piece::Knight => "n",
        Piece::Bishop => "b",
        Piece::Rook => "r",
        Piece::Queen => "q",
        Piece::King => "k",
    }
}

// ─── Outgoing Message Generation ────────────────────────────────────

/// Generate contextual team messages based on board evaluation and play style.
fn generate_team_messages(
    our_reserve_impact: &[i32; NUM_NON_KING_PIECES],
    best_score: i32,
    play_style: strategy::PlayStyle,
) -> Vec<String> {
    let mut messages = Vec::new();
    let piece_order = [Piece::Pawn, Piece::Knight, Piece::Bishop, Piece::Rook, Piece::Queen];

    // 1. Need message: find the piece with highest reserve_impact on our board
    let mut best_impact = 0;
    let mut best_piece = None;
    for (i, &piece) in piece_order.iter().enumerate() {
        if our_reserve_impact[i] > best_impact {
            best_impact = our_reserve_impact[i];
            best_piece = Some(piece);
        }
    }
    if best_impact >= 50 {
        if let Some(piece) = best_piece {
            let urgency = if best_impact >= 200 { "high" }
                else if best_impact >= 100 { "medium" }
                else { "low" };
            messages.push(format!("need {} urgency {}", piece_to_msg_char(piece), urgency));
        }
    }

    // 2. Threat message based on our score
    if best_score <= -500 {
        messages.push("threat critical".to_string());
    } else if best_score <= -200 {
        messages.push("threat high".to_string());
    } else if best_score <= -100 {
        messages.push("threat medium".to_string());
    } else if best_score <= -50 {
        messages.push("threat low".to_string());
    }

    // 3. Material report (rounded to nearest 50)
    let rounded = (best_score / 50) * 50;
    let sign = if rounded >= 0 { "+" } else { "" };
    messages.push(format!("material {}{}", sign, rounded));

    // 4. Play fast if we're in time trouble
    if matches!(play_style, strategy::PlayStyle::Blitz | strategy::PlayStyle::Instant) {
        messages.push("play_fast reason time".to_string());
    }

    messages
}

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
    /// Parsed state from partner's messages.
    partner_state: PartnerState,
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
            partner_state: PartnerState::default(),
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
        self.partner_state = PartnerState::default();
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
            state.partner_state.parse_message(msg);
            debug!("[game:{}] Partner message: {} → state={:?}", state.game_id, msg, state.partner_state);
            vec![]
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
    let both_team_active = state.active_go[other_idx]; // other board already has active go
    let mut play_style = strategy::determine_play_style(&state.clocks, board_id, side, both_team_active);

    // Partner play_fast override: downgrade to Blitz if partner is in time trouble
    if state.partner_state.play_fast
        && matches!(play_style, strategy::PlayStyle::Standard | strategy::PlayStyle::Extended)
    {
        debug!("[game:{}] Partner requested play_fast — downgrading {:?} to Blitz",
            state.game_id, play_style);
        play_style = strategy::PlayStyle::Blitz;
    }

    let budget_ms = crate::time::allocate_time(&state.clocks, board_id, side, play_style);

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
        state.game_id, board_id, our_time, opp_time, budget_ms, play_style
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
            // Apply partner need boost: if partner explicitly requested a piece,
            // amplify that piece's reserve_impact before ranking
            let mut adjusted_other_ri = other_reserve_impact;
            if let Some((piece, urgency)) = state.partner_state.needed_piece {
                let idx = piece.to_index();
                if idx < NUM_NON_KING_PIECES {
                    let factor = match urgency {
                        Urgency::High => 2.0,
                        Urgency::Medium => 1.5,
                        Urgency::Low => 1.0,
                    };
                    adjusted_other_ri[idx] = (adjusted_other_ri[idx] as f32 * factor) as i32;
                    if factor > 1.0 {
                        debug!("[game:{}] Partner needs {:?} (urgency {:?}) — boosting reserve_impact {}→{}",
                            state.game_id, piece, urgency, other_reserve_impact[idx], adjusted_other_ri[idx]);
                    }
                }
            }

            // If partner is under high/critical threat, boost pawn/knight captures
            // (defensive pieces for blocking checks and covering squares)
            if matches!(state.partner_state.threat_level, Some(ThreatLevel::High) | Some(ThreatLevel::Critical)) {
                let boost = 1.5_f32;
                adjusted_other_ri[Piece::Pawn.to_index()] = (adjusted_other_ri[Piece::Pawn.to_index()] as f32 * boost) as i32;
                adjusted_other_ri[Piece::Knight.to_index()] = (adjusted_other_ri[Piece::Knight.to_index()] as f32 * boost) as i32;
                debug!("[game:{}] Partner threat {:?} — boosting pawn/knight reserve_impact",
                    state.game_id, state.partner_state.threat_level);
            }

            let ranking = engine::compute_cross_board_ranking(&eval_status, &adjusted_other_ri, other_eval_status.completed_depth);
            let base_weight = cross_board_weight(
                state.active_go[other_idx],
                state.boards[other_idx].as_ref(),
                state.our_color[other_idx],
            );

            // Partner stall override: minimize cross-board influence
            let style_factor = if state.partner_state.stall {
                debug!("[game:{}] Partner requested stall — minimizing cross-board weight", state.game_id);
                0.1
            } else {
                play_style.style_factor()
            };
            let weight = base_weight * style_factor;

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

            // Aggressiveness threshold: only allow cross-board override if the
            // cross-board value exceeds the threshold for our PlayStyle.
            let threshold = play_style.aggressiveness_threshold();
            let best_local_move = scored_moves.iter().max_by_key(|m| m.1).map(|m| &m.0);
            let (adjusted_best_str, _, adjusted_best_cross, adjusted_best_score, _) = &scored_moves[0];

            let chosen = if let Some(blm) = best_local_move {
                if blm != adjusted_best_str && adjusted_best_cross.abs() < threshold {
                    // Cross-board adjustment wants to override, but doesn't meet threshold
                    info!("[game:{}] Board {:?}: cross-board override blocked by {:?} threshold ({} < {}cp), keeping local best {}",
                        state.game_id, board_id, play_style, adjusted_best_cross, threshold, blm);
                    blm.clone()
                } else {
                    if blm != adjusted_best_str {
                        info!("[game:{}] Board {:?}: CROSS-BOARD OVERRIDE: {} (adjusted={}) over {} (local best), style={:?}",
                            state.game_id, board_id, adjusted_best_str, adjusted_best_score, blm, play_style);
                    }
                    adjusted_best_str.clone()
                }
            } else {
                adjusted_best_str.clone()
            };

            let log_str = if *adjusted_best_cross != 0 {
                format!("cross_board={} weight={:.2} adjusted={} style={:?}", adjusted_best_cross, weight, adjusted_best_score, play_style)
            } else {
                format!("local (no cross-board impact) style={:?}", play_style)
            };
            info!("[game:{}] Board {:?}: depth {} chose {} ({})",
                state.game_id, board_id, eval_status.completed_depth, chosen, log_str);

            chosen
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

    // Build response: TeamMsg(s) + per-depth Info lines + BestMove
    let team_messages = generate_team_messages(
        &eval_status.eval.reserve_impact,
        eval_status.best_score,
        play_style,
    );

    let mut responses: Vec<UbiResponse> = team_messages.iter()
        .map(|msg| UbiResponse::TeamMsg(msg.clone()))
        .collect();

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

    // ─── Partner State Tests ────────────────────────────────────────

    #[test]
    fn parse_partner_need_message() {
        let mut ps = PartnerState::default();
        ps.parse_message("need n urgency high");
        assert_eq!(ps.needed_piece, Some((Piece::Knight, Urgency::High)));

        ps.parse_message("need q urgency medium");
        assert_eq!(ps.needed_piece, Some((Piece::Queen, Urgency::Medium)));

        ps.parse_message("need p");
        assert_eq!(ps.needed_piece, Some((Piece::Pawn, Urgency::Medium))); // default urgency
    }

    #[test]
    fn parse_partner_threat_message() {
        let mut ps = PartnerState::default();
        ps.parse_message("threat critical");
        assert_eq!(ps.threat_level, Some(ThreatLevel::Critical));

        ps.parse_message("threat low");
        assert_eq!(ps.threat_level, Some(ThreatLevel::Low));
    }

    #[test]
    fn parse_partner_material_message() {
        let mut ps = PartnerState::default();
        ps.parse_message("material -150");
        assert_eq!(ps.material, Some(-150));

        ps.parse_message("material +200");
        assert_eq!(ps.material, Some(200));
    }

    #[test]
    fn parse_partner_play_fast_and_stall() {
        let mut ps = PartnerState::default();
        assert!(!ps.play_fast);
        assert!(!ps.stall);

        ps.parse_message("play_fast reason time");
        assert!(ps.play_fast);

        ps.parse_message("stall");
        assert!(ps.stall);
    }

    #[test]
    fn parse_partner_replaces_previous() {
        let mut ps = PartnerState::default();
        ps.parse_message("need n urgency high");
        ps.parse_message("need q urgency low");
        // Latest message wins
        assert_eq!(ps.needed_piece, Some((Piece::Queen, Urgency::Low)));
    }

    #[test]
    fn generate_team_messages_need_piece() {
        // High reserve_impact for knight → "need n urgency high"
        let ri = [0, 250, 0, 0, 0]; // knight=250cp
        let msgs = generate_team_messages(&ri, 0, strategy::PlayStyle::Standard);
        assert!(msgs.iter().any(|m| m == "need n urgency high"),
            "expected 'need n urgency high' in {:?}", msgs);
    }

    #[test]
    fn generate_team_messages_threat() {
        let ri = [0; NUM_NON_KING_PIECES];
        let msgs = generate_team_messages(&ri, -300, strategy::PlayStyle::Standard);
        assert!(msgs.iter().any(|m| m == "threat high"),
            "expected 'threat high' for score=-300 in {:?}", msgs);
    }

    #[test]
    fn generate_team_messages_no_threat_when_positive() {
        let ri = [0; NUM_NON_KING_PIECES];
        let msgs = generate_team_messages(&ri, 100, strategy::PlayStyle::Standard);
        assert!(!msgs.iter().any(|m| m.starts_with("threat")),
            "should not have threat message for positive score, got {:?}", msgs);
    }

    #[test]
    fn generate_team_messages_material_report() {
        let ri = [0; NUM_NON_KING_PIECES];
        let msgs = generate_team_messages(&ri, 130, strategy::PlayStyle::Standard);
        assert!(msgs.iter().any(|m| m == "material +100"),
            "expected 'material +100' (rounded from 130) in {:?}", msgs);
    }

    #[test]
    fn generate_team_messages_play_fast_in_blitz() {
        let ri = [0; NUM_NON_KING_PIECES];
        let msgs = generate_team_messages(&ri, 0, strategy::PlayStyle::Blitz);
        assert!(msgs.iter().any(|m| m == "play_fast reason time"),
            "expected play_fast in Blitz mode, got {:?}", msgs);
    }

    #[test]
    fn generate_team_messages_no_play_fast_in_standard() {
        let ri = [0; NUM_NON_KING_PIECES];
        let msgs = generate_team_messages(&ri, 0, strategy::PlayStyle::Standard);
        assert!(!msgs.iter().any(|m| m.starts_with("play_fast")),
            "should not have play_fast in Standard mode, got {:?}", msgs);
    }

    #[test]
    fn partner_play_fast_overrides_style() {
        let mut state = new_state();
        state.partner_state.parse_message("play_fast reason time");
        set_startpos(&mut state);
        // With 60s on clock, normally Standard. But partner play_fast → Blitz.
        state.clocks = [60000, 60000, 60000, 60000];
        let style = strategy::determine_play_style(&state.clocks, BoardId::A, Color::White, false);
        assert_eq!(style, strategy::PlayStyle::Standard);
        // The override happens in handle_go, not in determine_play_style
        assert!(state.partner_state.play_fast);
    }
}
