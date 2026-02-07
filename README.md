# bughouse-engine

A Rust chess engine for [Bughouse](https://en.wikipedia.org/wiki/Bughouse_chess) — the 2v2 variant where captured pieces transfer to your partner's board for placement.

This binary speaks the **BUP** (Bughouse Universal Protocol) on stdin/stdout and understands **BFEN** (Bughouse FEN) for board positions. It is designed to be spawned as an Erlang/Elixir `Port` by the [bughouse](https://github.com/vcsawant/bughouse) Phoenix application.

---

## Where this fits: the 5-layer architecture

The bughouse platform is split into five layers. This engine lives at **layers 3 and 4**:

```
Layer 5  │  Phoenix LiveView UI          (Elixir — real-time browser)
         │
Layer 4  │  Game orchestration + clocks  (Elixir GenServer)
         │
Layer 3  │  ◀◀◀  bughouse-engine  ◀◀◀   (this binary — Rust)
         │       BUP protocol, move search, evaluation
         │
Layer 2  │  Port wrapper                 (Elixir — spawns this binary,
         │                                marshals BUP messages)
         │
Layer 1  │  Move validation              (Erlang — binbo_bughouse library,
         │                                source of truth for legality)
```

During early development the Rust engine is a *suggester*: it proposes moves via BUP, but the Erlang engine (binbo_bughouse) performs the authoritative legality check before the move is committed. As confidence grows, the Rust engine can take on full validation responsibility.

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

### BUP — Bughouse Universal Protocol (`docs/BUP.md`)

A line-based stdin/stdout protocol (inspired by UCI) that supports dual-board bughouse games. Key commands this engine must handle:

| Direction | Command | Description |
|-----------|---------|-------------|
| stdin  | `bup`                          | Handshake — engine must reply `bupok` |
| stdin  | `position board <A\|B> bfen <string>` | Set board state |
| stdin  | `go board <A\|B>`              | Start searching; reply with `bestmove` |
| stdin  | `clock <side>_<board> <ms> ...` | Update clock values |
| stdin  | `stop`                         | Abort search immediately |
| stdin  | `teammsg <text>`               | Message from partner engine |
| stdout | `bestmove board <A\|B> <move>` | Engine's chosen move |
| stdout | `partnermsg <text>`            | Message to partner engine |

Drop moves use `@` notation: `p@e4` places a pawn on e4.

> **Note:** `BUP.md` is the authoritative protocol spec. An earlier integration design document in the Phoenix repo (`BUGHOUSE_ENGINE_INTEGRATION.md`) sketches a simplified variant; that will be reconciled in a future phase.

---

## Build & run

```bash
# One-time: install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build (debug)
cargo build                    # → target/debug/bughouse_engine

# Build (release — optimised, no debug symbols)
cargo build --release          # → target/release/bughouse_engine

# Run interactively (type BUP commands, Ctrl-D to exit)
cargo run

# Test (46 unit tests covering protocol parsing, state management, move generation)
cargo test
```

The engine binary communicates over **stdin/stdout** only. It is not a server — it is spawned as a child process by the Elixir Port wrapper (Layer 2). To test it manually you can pipe commands into it:

```bash
echo -e "bup\nisready\nbupnewgame\nposition board A startpos\ngo board A\nquit" \
  | ./target/debug/bughouse_engine
```

Example output:

```
id name BughouseEngine 0.1.0
id author Viren Sawant
bupok
readyok
info board A depth 0 nodes 20 time 0 score cp 0
bestmove board A e2e4
```

With reserves (drops available):

```bash
echo -e "bup\nbupnewgame\nposition board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QNPqp] w KQkq - 0 1\ngo board A\nquit" \
  | ./target/debug/bughouse_engine
```

```
id name BughouseEngine 0.1.0
id author Viren Sawant
bupok
info board A depth 0 nodes 116 time 0 score cp 0
bestmove board A n@c3
```

---

## Source layout

```
src/
├── main.rs          # Thin I/O loop: stdin → parse → dispatch → format → stdout
├── bup.rs           # BUP protocol parser & formatter (pure data, no I/O)
└── game_state.rs    # EngineState + command dispatch → responses (no I/O)
```

Board representation, move generation, BFEN parsing, and drop logic all live in the [bughouse-chess](https://github.com/vcsawant/bughouse-chess) library. The engine is deliberately thin — it wires protocol parsing to the library's game logic.

Future phases will add:
- `search.rs` — tree search (minimax/alpha-beta or MCTS)
- `scoring.rs` — static evaluation function (material, position, reserves)
- `time.rs` — clock management and time allocation per move

---

## Roadmap

Development proceeds in four phases, each building on the last:

### Phase B — Random-move bot (complete)

The minimum viable engine. Parses BUP commands, maintains board state via BFEN, generates all legal moves (regular + drops), picks one at random, and returns `bestmove`. This validates the entire plumbing: Rust binary ↔ Elixir Port ↔ BUP protocol ↔ BFEN parsing ↔ move generation.

**Implemented:**
- `bup.rs` — BUP protocol parser & formatter (28 unit tests)
- `game_state.rs` — engine state, command dispatch, random move selection (18 unit tests)
- `main.rs` — thin I/O loop with BufWriter for efficient stdout
- Full BUP handshake (`bup`/`bupok`), position setup (`startpos` / `bfen`), clock tracking, `go` / `stop` / `quit`
- Drop moves from reserves with BUP-compliant lowercase notation (`p@e4`, `n@f3`)

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

## Relationship to binbo_bughouse

[binbo_bughouse](https://github.com/vcsawant/binbo-bughouse) is the Erlang chess engine that currently powers the Phoenix app. It is the **source of truth** for move validation and already ships spec-compliant BFEN (single-bracket reserves, `~` promoted pieces, full round-trip fidelity).

During development, binbo validates every move this engine suggests. This two-engine setup means:
- You can develop and test the Rust engine independently (it doesn't need to be perfect).
- Illegal moves proposed by the Rust engine are simply rejected — the app stays correct.
- Over time, as the Rust engine matures, it can absorb validation responsibility too.

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
