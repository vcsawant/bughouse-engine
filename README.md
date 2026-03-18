# bughouse-engine

A Rust chess engine for [Bughouse](https://en.wikipedia.org/wiki/Bughouse_chess) — the 2v2 variant where captured pieces transfer to your partner's board for placement.

This binary speaks the **UBI** (Universal Bughouse Interface) on stdin/stdout and understands **BFEN** (Bughouse FEN) for board positions. Any GUI or game server can spawn it as a child process and communicate via the UBI text protocol — the same way UCI engines work for standard chess.

---

## Protocol specs

Both specs live in [`docs/`](docs/) in this repo.

### BFEN — Bughouse FEN (`docs/BFEN.md`)

An extension of standard FEN that adds a reserve bracket and a promoted-piece marker:

```
<position>[<reserves>] <side> <castling> <en-passant> <halfmove> <fullmove>
           ^^^^^^^^^^^
           e.g. [QNPqp]   ← white holds Q,N,P; black holds q,p
```

- Reserves appear in canonical order: `Q R B N P q r b n p`.
- Pieces that were promoted (e.g. a pawn promoted to a queen) are marked with a `~` suffix in the position string: `Q~`. If that piece is later captured, it *demotes* back to a pawn before entering the capturer's reserve.
- Empty reserves in bughouse mode emit `[]`.

### UBI — Universal Bughouse Interface (`docs/UBI.md`)

A line-based stdin/stdout protocol (inspired by UCI) that supports dual-board bughouse games. Key commands this engine must handle:

| Direction | Command | Description |
|-----------|---------|-------------|
| stdin  | `ubi`                          | Handshake — engine must reply `ubiok` |
| stdin  | `position board <A\|B> bfen <string>` | Set board state |
| stdin  | `go board <A\|B>`              | Start searching; reply with `bestmove` |
| stdin  | `clock <side>_<board> <ms> ...` | Update clock values |
| stdin  | `stop`                         | Abort search immediately |
| stdin  | `teammsg <text>`               | Message from partner engine |
| stdout | `bestmove board <A\|B> <move>` | Engine's chosen move |
| stdout | `partnermsg <text>`            | Message to partner engine |

Drop moves use `@` notation: `p@e4` places a pawn on e4.

---

## Build & run

```bash
# One-time: install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build (debug)
cargo build                    # → target/debug/bughouse_engine

# Build (release — optimised, no debug symbols)
cargo build --release          # → target/release/bughouse_engine

# Run interactively (type UBI commands, Ctrl-D to exit)
cargo run

# Test (145 unit tests covering protocol, state, evaluation, search, TT, quiescence, LMR, opening book, strategy, time, teammsg, pondering)
cargo test
```

The engine binary communicates over **stdin/stdout** only. It is not a server — it is spawned as a child process by a GUI or game server. To test it manually you can pipe commands into it:

```bash
echo -e "ubi\nisready\nubinewgame\nposition board A startpos\ngo board A\nquit" \
  | ./target/debug/bughouse_engine
```

Example output:

```
id name BughouseEngine 0.1.0
id author Viren Sawant
ubiok
readyok
teammsg need p
info board A depth 1 nodes 20 time 0 score cp 66 pv b1c3
info board A depth 2 nodes 391 time 4 score cp 0 pv b1c3
info board A depth 3 nodes 6901 time 68 score cp 66 pv b1c3
info board A depth 4 nodes 94708 time 825 score cp 0 pv b1c3
bestmove board A b1c3
```

With reserves (drops available):

```bash
echo -e "ubi\nubinewgame\nposition board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1\ngo board A\nquit" \
  | ./target/debug/bughouse_engine
```

```
id name BughouseEngine 0.1.0
id author Viren Sawant
ubiok
teammsg threat critical
info board A depth 1 nodes 80 time 1 score cp 617 pv q@e6
bestmove board A q@e6
```

---

## Source layout

```
src/
├── main.rs          # Thin I/O loop: stdin → parse → dispatch → format → stdout
├── ubi.rs           # UBI protocol parser & formatter (pure data, no I/O)
├── game_state.rs    # EngineState + command dispatch → responses (no I/O)
├── engine.rs        # Multi-threaded eval threads with pondering, EvalCommand/EvalStatus
├── book.rs          # Opening book: bughouse theory lines with weighted random selection
├── strategy.rs      # PlayStyle enum + time-aware style selection (stub for Phase C)
├── scoring.rs       # Static evaluation: material, reserves, PSTs, king safety, mobility, pawns
├── search.rs        # Alpha-beta negamax with iterative deepening, drop pruning, P/C computation
└── time.rs          # Time budget allocation from clock state
```

Board representation, move generation, BFEN parsing, and drop logic all live in the [bughouse-chess](https://github.com/vcsawant/bughouse-chess) library. The library also provides precomputed bitboard lookup tables (king zone, forward file masks, attack maps) that the engine uses for O(1) evaluation queries. The engine is deliberately thin — it handles search policy and protocol, while the library handles chess primitives.

Future phases will add:
- `time.rs` — clock management and time allocation per move
- `strategy.rs` — cross-board strategy layer (currently a stub with PlayStyle enum)

---

## Roadmap

Development proceeds in five phases, each building on the last:

### Phase A — Specs and scaffolding (complete)

Define the protocols and set up the project skeleton before the engine can play a single move.

**Implemented:**
- BFEN spec (`docs/BFEN.md`) — bughouse FEN with reserves bracket and promoted-piece marker
- UBI spec (`docs/UBI.md`) — line-based stdin/stdout protocol for dual-board bughouse
- Project scaffolding — Cargo.toml, directory structure, `bughouse-chess` library dependency
- Thin I/O loop (`main.rs`) — stdin → parse → dispatch → format → stdout

### Phase B — Random-move bot (complete)

The minimum viable engine. Parses UBI commands, maintains board state via BFEN, generates all legal moves (regular + drops), picks one at random, and returns `bestmove`. This validates the entire pipeline: UBI protocol ↔ BFEN parsing ↔ move generation ↔ bestmove response.

**Implemented:**
- `ubi.rs` — UBI protocol parser & formatter (30 unit tests, including `partnermsg`)
- `game_state.rs` — engine state, command dispatch, random move selection (18 unit tests)
- `main.rs` — thin I/O loop with BufWriter for efficient stdout
- Full UBI handshake (`ubi`/`ubiok`), position setup (`startpos` / `bfen`), clock tracking, `go` / `stop` / `quit`
- `partnermsg` command parsing (acknowledged for future partner coordination)
- Drop moves from reserves with UBI-compliant lowercase notation (`p@e4`, `n@f3`)

### Phase C — Static evaluation + 1-ply search (complete)

Replace random selection with a scored evaluation function and 1-ply search. The engine evaluates every legal move and picks the best based on position analysis.

**Implemented:**
- `strategy.rs` — `PlayStyle` enum with 5 variants (Blitz, Standard, Extended, Slow, Instant); stub `determine_play_style()` always returns Blitz for Phase C
- `scoring.rs` — static evaluation function (17 unit tests) with 7 components:
  - Material counting (P=100, N=320, B=330, R=500, Q=900)
  - Reserve valuation at 70% discount
  - Piece-square tables (midgame, per-piece type)
  - King safety: pawn shield, open files, attacker count, reserve amplifier
  - Mobility: piece attack squares (knights, bishops, rooks, queens)
  - Pawn structure: doubled, isolated, and passed pawn detection
  - Checkmate/stalemate terminal detection (+30000/0)
- `search.rs` — 1-ply negamax search (12 unit tests) with:
  - Score-based move selection across regular moves and drops
  - Drop pruning: generates drops only on ~30-50 relevant squares (attack zone, defense zone, center, promotion zone) instead of all ~200+ empty squares
  - Checkmate detection (score +30000 for mating opponent)
  - All moves guaranteed legal by the library (no self-check, pins enforced)
- `game_state.rs` — wired search into `handle_go()`, reports depth 1 with actual score and node count

### Phase D — Deep search + per-board evaluation (complete)

Multi-ply search with alpha-beta pruning and iterative deepening. Each board's evaluation process produces three outputs: move evaluations, piece acquisition probabilities (P), and piece acquisition costs (C). See [Evaluation architecture](#evaluation-architecture-dual-process-model) below for the full design.

**Implemented:**
- `search.rs` — alpha-beta negamax with iterative deepening (18 unit tests)
  - Recursive negamax with alpha-beta pruning to arbitrary depth
  - MVV-LVA move ordering (captures first, then drops by piece value, then quiet moves)
  - Drop pruning at every node (attack zone, defense zone, center, promotion zone)
  - Mate-distance scoring: `MATE_SCORE - ply` prefers shorter checkmates
  - Iterative deepening: depth 1, 2, 3... until time budget expires; depth 1 always completes
  - Time check every 1024 nodes; incomplete iterations are discarded
  - Per-depth `info` line streaming for real-time GUI display
  - P/C capture statistics computed at root level as search byproduct (zero overhead)
- `time.rs` — time budget allocation (6 unit tests)
  - Clock-aware: indexes the correct clock from board ID + side to move
  - Heuristic: `our_time / 30` remaining moves estimate, clamped to [100ms, 25% of time]
  - Emergency mode: < 1s remaining → 50ms budget
- `strategy.rs` — real `determine_play_style` based on clock state (5 unit tests)
  - `Instant` (< 1s), `Blitz` (< 10s), `Extended` (> 30s with 2x advantage), `Standard` (default)

**Deferred to future optimization:**
- Continuous background evaluation (pondering) with subtree reuse
- Delta pruning in quiescence search

**Recently implemented:**
- Opening book: hardcoded bughouse opening theory (~40 entries) covering the first 3-4 moves for both colors. Includes the Mongolian Attack (e4/d4/Bc4/Be3), Nh3→Ng5 f7-attack system, Italian setup, and fortress defense (e6/d6). Book positions are matched by position hash (excluding reserves) with weighted random selection for variety. Book hits return instantly, saving clock time for the middlegame.
- Late move reductions (LMR): after the first few moves are searched at full depth, later quiet moves are searched at reduced depth. If the reduced search returns a surprisingly good score, a full-depth re-search is triggered. Uses a precomputed logarithmic reduction table (`ln(depth) * ln(move_index) / 2`). Drops are never reduced (bughouse-specific — drops are high-impact tactical moves). Typically adds 1-2 effective plies of search depth for the same time budget.
- Killer moves and history heuristic for move ordering: quiet moves that cause beta cutoffs are tracked per-ply (2 killer slots) and promoted in move ordering. History heuristic tracks `[color][from][to]` cutoff frequency with depth²-scaled bonuses.
- Quiescence search: at the search horizon, continues searching capture-only moves until the position is stable. Eliminates tactical blind spots (horizon effect) where the engine would miss recaptures or hanging pieces. Uses `MoveGen::capture_moves()` from the library for efficient capture generation.
- Transposition table (hash-based position caching with Zobrist keys, 8 MB default, configurable via `setoption name Hash value <MB>`). TT best-move ordering, mate score adjustment, and always-replace policy. Shared across both boards.

### Phase E — Cross-board strategy layer (in progress)

The engine understands it's playing bughouse, not just two independent chess games. The strategy layer combines evaluations from both board processes to make cross-board-aware decisions.

**E1 — Per-board evaluation state (complete):**
- `BoardEval` struct per board: P/C capture stats, reserve impact, score, depth
- Reserve impact via difference search: for each piece type, re-search with that piece added to reserves to measure how much it would help
- Quick-eval of the non-`go` board to keep both evaluations fresh
- Both board evaluations logged for debugging

**E2a — Cross-board move selection (complete):**
- Reserve-aware move selection: bonus for captures that feed partner useful pieces
- Cross-board weight based on board control (who has active go, whose turn)

**E3 — Strategy-aware decision making (complete):**
- PlayStyle (Instant/Blitz/Standard/Extended/Slow) now drives all three decision axes:
  - **Time budget**: Blitz=time/40 max 500ms, Standard=time/30 max 2s, Extended=time/20 max 4s, Instant=50ms
  - **Cross-board aggression**: style factor multiplies cross-board weight (0.0 for Instant → 1.5 for Extended)
  - **Aggressiveness threshold**: minimum cross-board value (cp) before deviating from local best (150cp Blitz, 75cp Standard, 30cp Extended)
- Both-team-active detection: when both boards have active go (both clocks draining), biases toward Blitz under 15s

**E5 — TeamMsg / PartnerMsg integration (complete):**
- **Outgoing messages** generated from actual game state:
  - `need <piece> urgency <level>` — from reserve_impact (impact ≥200cp=high, ≥100cp=medium, ≥50cp=low)
  - `threat <level>` — from position score (≤-500cp=critical, ≤-200cp=high, ≤-100cp=medium, ≤-50cp=low)
  - `material <+/-value>` — position score rounded to nearest 50cp
  - `play_fast reason time` — when in Blitz/Instant time pressure
- **Incoming messages** parsed into PartnerState and influence behavior:
  - `need <piece>` — boosts cross-board value for captures of that piece (2x high, 1.5x medium)
  - `threat high/critical` — boosts pawn/knight capture value (defensive pieces for partner)
  - `play_fast` — overrides Standard/Extended to Blitz (faster time budget)
  - `stall` — minimizes cross-board weight (avoid sending pieces to opponent)

**Remaining deliverables (E4, E6):**
- Stall detection: expected value of waiting (P × reserve_impact) vs cost of stalling
- Background pondering

### Alternate architecture: Unified cross-board search

A separate bot variant that treats both boards as a single game, searching a combined state `(board_a, board_b)` with interleaved moves. Eliminates the need for P/C values and a strategy layer — cross-board tactics emerge naturally from the search. See [`docs/UNIFIED_SEARCH.md`](docs/UNIFIED_SEARCH.md) for the full design.

Both bot variants speak UBI and can be compared head-to-head.

---

## Evaluation architecture: dual-process model

Bughouse evaluation differs fundamentally from standard chess. Beyond material and position, the engine must reason about **four clocks**, **two boards**, **reserves**, and **partner coordination**. Two core insights drive the design:

> 1. **Each board can be evaluated independently.** Move evaluations, piece acquisition probability (P), and piece acquisition cost (C) are deterministic functions of a single board's position. Cross-board reasoning happens in the strategy layer, not the evaluation layer.
> 2. **Time dynamics determine strategic constraints _before_ move selection.** The strategy layer filters the strategic mode first, then selects moves using evaluations from both boards.

### Per-board evaluation process

Each board runs an independent, continuous evaluation process. Given a position, it produces three outputs:

#### 1. Move evaluations

Standard minimax with alpha-beta pruning over all legal moves (regular + drops from actual reserves) to depth D, with iterative deepening. This is a deepened version of the existing 1-ply search in `search.rs`. The evaluation tree alternates moves (our move → their move → our move). The process deepens continuously until interrupted by a new position or a time budget expiration.

When a new position arrives (opponent moves, or a move on the other board updates reserves), the process checks if the new position is a subtree of the previously evaluated tree. If so, it reuses that subtree and continues deepening from there. Otherwise it restarts evaluation from scratch.

#### 2. P(piece_type, color) — acquisition probability

For each piece type X and each color Y, the probability that Y captures a piece of type X on this board in the near future. Computed as the **minimum** across all instances of piece type X (we want the cheapest-to-acquire instance).

Per-instance probability is derived from the search tree:

| Condition | P |
|---|---|
| Piece is hanging (attacked, undefended or under-defended) | 1.0 |
| Piece is won via tactic (search finds net-positive capture sequence) | 0.8 |
| Piece is available via equal trade | 0.5 |
| Piece is only capturable via unfavorable trade | 0.2 |
| Piece is not capturable within search horizon | 0.0 |

These values are tunable. The key property is that P is deterministic given the position and search depth — the search already discovers which lines involve capturing which pieces.

Both colors' P values are computed because the strategy layer needs to know what our teammate can capture **and** what our opponent can capture on a given board.

#### 3. C(piece_type, color) — acquisition cost

For each piece type X and each color Y, the cost to Y of capturing X on this board. Computed as the **minimum** across all instances of piece type X, where per-instance cost is:

```
C(X, Y) = best_eval - best_eval_in_lines_where_Y_captures_X
```

Both values are from Y's perspective. If capturing X is already Y's best line, C = 0 (no extra cost). If capturing X requires a suboptimal line, C equals the eval sacrifice.

Like P, C is a deterministic byproduct of the search — we run minimax normally, note which lines capture which pieces, and the cost falls out of the eval differences.

#### Properties

- **Deterministic**: Given a position and search depth, all three outputs are fully determined. No randomness, no time dependency. This means evaluation results are memoizable and hashable (Zobrist keys from the bughouse-chess library include reserve state).
- **Continuous**: The process runs iterative deepening in the background. Deeper search refines P, C, and move evaluations. The strategy layer reads whatever depth is available when it needs to decide.
- **Independent**: Board A's process knows nothing about board B. Cross-board reasoning is the strategy layer's job.

### Strategy layer

The strategy layer activates when the engine receives a `go` command for a specific board. It reads the latest evaluation data from **both** board processes and combines them to pick a move.

#### Combining cross-board evaluations

For the board we need to move on (say board A), the strategy layer considers:

**Regular moves and drops from actual reserves:** Ranked directly by board A's minimax evaluation.

**Potential drops (pieces not yet in reserves):** For each piece type X not currently in our reserves on board A:

```
wait_value(X) = P_teammate(X) on board B
              × eval_of_best_drop(X) on board A
              − C_teammate(X) on board B
```

This tells us the expected value of waiting for a piece to arrive from board B. If `wait_value(X)` exceeds the best available move, the engine may choose to stall (if the time state permits).

**Cross-board feedback (order-1):** When considering a capture on board A that sends piece Y to board B's opponent, the strategy layer checks board B's evaluation to estimate the damage. Specifically, it looks at how Y in the opponent's reserves affects board B's eval. This prevents the engine from, e.g., capturing a knight on board A (good locally) when that knight would devastate our teammate on board B.

Higher-order feedback (the piece gets dropped, changes the position, changes P/C for other pieces) grows exponentially. The strategy layer bounds this at order-1 for now, with room to deepen when time permits and the decision is close.

#### Aggressiveness threshold

A scalar derived from the time state that modulates the team's willingness to accept cost. Governs how much C the strategy is willing to tolerate for a given P and wait_value:

- **Time advantage** → accept higher C (teammate can afford to sacrifice material to feed us pieces)
- **Time disadvantage** → only accept C ≈ 0 (take what's free, don't speculate)
- **Teammate crushing on board B** → accept very high C (teammate can afford to lose material)

Example: Board B teammate can capture a knight at C = 200cp. That knight would be worth +350cp dropped on board A. If the team has a time advantage, the strategy accepts the 200cp cost because the net team value is +150cp. In time trouble, it wouldn't — too speculative.

#### Decision flow

1. Read latest evaluations from both board processes
2. Compute `wait_value(X)` for each missing piece type
3. Compare `best_move_now` (board A's top minimax result) vs `best_wait_value`
4. Apply aggressiveness threshold based on time state
5. If waiting is better and time state allows stalling → play a quiet stall move
6. Otherwise → play `best_move_now`
7. Short-circuit: if `best_move_now` is checkmate or exceeds all alternatives by a large threshold, play it immediately regardless of wait values

The strategy allocates a time budget based on PlayStyle. It makes a preliminary decision immediately with whatever depth is available, then refines as deeper evaluation results arrive, until the time budget expires.

### Time state detection

In bughouse, exactly two of the four clocks are always running (one per board). When it is the engine's turn, the clock configurations reduce to two cases:

| Configuration | Active clocks | Implication |
|---|---|---|
| **Both team clocks active** | Player + Teammate | TIME DISADVANTAGE — both our clocks drain simultaneously. Play quickly, accept risk. |
| **One team clock + one opponent clock** | Player + Opponent's teammate | Depends on relative time — compare `player_time` vs `opponent_teammate_time`. |

A third comparison matters even when the direct opponent's clock is paused: `player_time` vs `opponent_time`. If the engine has 60s and the opponent has 10s, the engine can afford slow, forcing moves that burn the opponent's remaining time.

These combine into four strategic time states:

| State | Condition | Strategy |
|---|---|---|
| `Disadvantage` | Both team clocks running | Play fast, accept risk, shallow search |
| `PotentialAdvantage` | Player has significantly more time than the active opponent-side clock | Bank time, consider stalling, prepare tempo plays |
| `MildDisadvantage` | Opponent team has more total active time | Play solid, standard search depth |
| `LocalAdvantage` | Player has significantly more time than direct opponent (even if opponent's clock is paused) | Deep search, play forcing moves to exploit opponent's time trouble |

### PlayStyle and move selection

| Time state | Search budget | Move preference |
|---|---|---|
| `Disadvantage` / `Blitz` | ~500ms, shallow | Aggressive, forcing moves. Low aggressiveness threshold — only accept free pieces. |
| `MildDisadvantage` / `Standard` | ~1500ms, standard | Solid positional play. Moderate aggressiveness. |
| `PotentialAdvantage` / `LocalAdvantage` / `Extended` | ~3000ms, deep | Best move with full search. High aggressiveness — willing to accept cost for speculative pieces. |
| Stalling / `Slow` | Minimal | Quiet, non-committal moves that maintain flexibility and king safety. |
| Ultra-low time / `Instant` | Instant | Pre-calculated safe move. |

**Blitz (MoveFast)** is the simplest strategy and the first to implement. It strongly prefers positional play on the current board:
- Uses board A's minimax eval as the primary signal
- Low aggressiveness threshold (don't accept high cost for speculative pieces)
- Rarely waits for pieces (`wait_value` must massively exceed `best_move_now`)
- Short time budget — takes whatever search depth is available quickly
- Essentially plays "good chess" with minimal bughouse-specific speculation

### Stall move selection

When the engine decides to stall, it picks the quietest safe move available:
- No captures, no checks, no aggressive drops
- Prefer moves that improve king safety
- Prefer moves that maximize future legal move count (maintain flexibility)
- Avoid committing material or weakening pawn structure

### Edge cases

- **Immediate win available**: Always play a winning tactic regardless of time state.
- **Opponent counter-stalling**: If both sides can stall, default to normal play to avoid deadlock.
- **Ultra-low time (< 1s)**: Bypass all strategy and play a pre-calculated safe move instantly.

### Known limitations and future work

**Alternating-move assumption:** The evaluation process models each board as alternating moves (our turn → their turn). In reality, moves across the two boards are asynchronous — a player on board B might make three moves while board A makes one. This is a simplification we accept for now; future work could model probabilistic move interleaving.

**Order-1 feedback bound:** The strategy layer only considers first-order cross-board effects (capture on A → piece enters B's reserves → evaluate impact on B). Deeper feedback chains (that piece gets dropped on B, changes B's position, changes P/C for other pieces on B, which affects what board A should do) are not modeled. This bound exists because the branching factor explodes exponentially. The strategy layer can optionally explore order-2+ when time permits and the decision is close.

**P/C locality:** P and C are estimated from a single board's position without knowledge of the other board. This means the cost estimate can be wrong — e.g., C(knight) on board B looks cheap, but the traded bishop would be devastating to us on board A. The strategy layer's cross-board feedback check catches the most egregious cases (order-1), but subtle multi-step interactions are missed.

### Implementation phases

| Phase | Deliverable |
|---|---|
| **Phase C** (done) | `PlayStyle` enum, `determine_play_style()` stub, 1-ply search, static evaluation |
| **Phase D** | Multi-ply alpha-beta search, iterative deepening, P/C computation as search byproducts, `time.rs` for time budgets, continuous background evaluation |
| **Phase E** | Strategy layer combining both boards, cross-board minimax, aggressiveness threshold, wait-vs-move logic, stall move selection, `teammsg`/`partnermsg` coordination |

---

## Dependencies

### bughouse-chess

The engine uses [bughouse-chess](https://github.com/vcsawant/bughouse-chess) — a Rust move generation library forked from [jordanbray/chess](https://github.com/jordanbray/chess) and adapted for bughouse rules. It provides:

- Bitboard-based board representation with reserves and promoted-piece tracking
- Legal move generation with full check/pin enforcement (king cannot move into check, pinned pieces restricted, castling through check forbidden)
- Drop move generation from reserves with check-aware filtering (drops can block sliding checks; no drops in double check)
- Bughouse-aware checkmate detection: only unblockable checks (knight, adjacent, double check) are checkmate — distant sliding checks with empty blocking squares remain ongoing (partner can send pieces)
- BFEN parsing and emission (reserves in `[]` brackets, promoted pieces with `~` suffix)
- Capture tracking with promoted-piece demotion
- Zobrist hashing that includes reserve state

The library is linked via `Cargo.toml`:
```toml
[dependencies]
bughouse-chess = { git = "https://github.com/vcsawant/bughouse-chess", branch = "main" }
```

## Tech notes

| Topic | Detail |
|-------|--------|
| Rust edition | 2024 |
| Chess library | `bughouse-chess` v0.1.0 (git dependency from GitHub) |
| Target | `aarch64-apple-darwin` (Apple Silicon) |
| Bitboard width | 64-bit (`u64`) — one per piece-type × colour |
| Protocol | Line-based stdin/stdout (no network sockets) |
| Concurrency | Single-threaded for now; dual-board evaluation processes and search parallelism are Phase D+ considerations |
