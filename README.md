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

# Test (46 unit tests covering protocol parsing, state management, move generation)
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
info board A depth 0 nodes 20 time 0 score cp 0
bestmove board A e2e4
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
info board A depth 0 nodes 116 time 0 score cp 0
bestmove board A n@c3
```

---

## Source layout

```
src/
├── main.rs          # Thin I/O loop: stdin → parse → dispatch → format → stdout
├── ubi.rs           # UBI protocol parser & formatter (pure data, no I/O)
└── game_state.rs    # EngineState + command dispatch → responses (no I/O)
```

Board representation, move generation, BFEN parsing, and drop logic all live in the [bughouse-chess](https://github.com/vcsawant/bughouse-chess) library. The engine is deliberately thin — it wires protocol parsing to the library's game logic.

Future phases will add:
- `search.rs` — tree search (minimax/alpha-beta or MCTS)
- `scoring.rs` — static evaluation function (material, position, reserves)
- `time.rs` — clock management and time allocation per move

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
- `ubi.rs` — UBI protocol parser & formatter (28 unit tests)
- `game_state.rs` — engine state, command dispatch, random move selection (18 unit tests)
- `main.rs` — thin I/O loop with BufWriter for efficient stdout
- Full UBI handshake (`ubi`/`ubiok`), position setup (`startpos` / `bfen`), clock tracking, `go` / `stop` / `quit`
- Drop moves from reserves with UBI-compliant lowercase notation (`p@e4`, `n@f3`)

### Phase C — Basic heuristics

Replace random selection with a scored choice. No tree search yet — just evaluate each legal move with a simple scoring function and pick the best.

**Deliverables:**
- `scoring.rs` — material counting (piece values), basic positional tables
- `search.rs` — 1-ply search (evaluate all moves, pick max score)

### Phase D — Real search

Introduce tree search so the engine looks multiple moves ahead.

**Deliverables:**
- `search.rs` — minimax with alpha-beta pruning, iterative deepening
- `time.rs` — time budget allocation (how long to search per move)

### Phase E — Bughouse-specific tuning

The engine understands it's playing bughouse, not just two independent chess games.

**Deliverables:**
- Reserve valuation (a held queen is worth less than a held pawn in some situations)
- Partner awareness via `teammsg` / `partnermsg`
- Positional adjustments for bughouse (e.g. king safety is less important when your partner can drop a defender)

---

## Evaluation strategy: time-aware move selection

Bughouse evaluation differs fundamentally from standard chess. Beyond material and position, the engine must reason about **four clocks**, **two boards**, **reserves**, and **partner coordination**. The core insight driving our evaluation design:

> **Time dynamics determine strategic constraints _before_ move selection.** Rather than encoding time awareness into the evaluation function, we filter the strategic mode first, then search within those constraints.

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

### Strategic decision tree

Move selection follows a three-level decision tree:

**Level 1 — Can we stall?**
Stalling is only viable in `PotentialAdvantage` or `LocalAdvantage` states. When both team clocks are running, stalling is self-destructive.

**Level 2 — Should we stall?**
Even when stalling is possible, it must have strategic value:
- Waiting for a critical piece from the partner (e.g., partner is about to capture a queen)
- Partner is in a losing position and needs thinking time
- Direct opponent is in severe time trouble (< 5s) — stalling applies pressure
- No good moves are available — waiting for the position to improve

Counter-stalling detection: if the opponent can also stall, avoid mutual deadlock and play normally.

**Level 3 — Move selection within strategy**

| Time state | Search budget | Move preference |
|---|---|---|
| `Disadvantage` | ~500ms, shallow | Aggressive, forcing moves |
| `MildDisadvantage` | ~1500ms, standard | Solid positional play |
| `PotentialAdvantage` / `LocalAdvantage` | ~3000ms, deep | Best move with full search |
| Stalling | Minimal | Quiet, non-committal moves that maintain flexibility and king safety |
| Ultra-low time (< 1s) | Instant | Pre-calculated safe move |

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

### Implementation phases

This framework maps onto the existing roadmap:

| Phase | Time-related deliverable |
|---|---|
| **Phase C** | `TimeState` enum, `determine_time_state()`, basic search budget allocation |
| **Phase D** | `time.rs` — full time management, stall detection, search depth adjustment |
| **Phase E** | Partner-aware stalling (waiting for pieces), opponent time pressure tactics |

---

## Dependencies

### bughouse-chess

The engine uses [bughouse-chess](https://github.com/vcsawant/bughouse-chess) — a Rust move generation library forked from [jordanbray/chess](https://github.com/jordanbray/chess) and adapted for bughouse rules. It provides:

- Bitboard-based board representation with reserves and promoted-piece tracking
- Legal move generation (all piece moves + castling, no check/pin filtering per bughouse rules)
- Drop move generation from reserves (pawn rank restrictions enforced)
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
| Concurrency | Single-threaded for now; search parallelism is a Phase D+ consideration |
