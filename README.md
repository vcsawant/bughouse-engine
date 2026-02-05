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

# Run the stub directly (currently just prints "Hello, world!")
cargo run

# Test
cargo test
```

The engine binary communicates over **stdin/stdout** only. It is not a server — it is spawned as a child process by the Elixir Port wrapper (Layer 2). To test it manually you can pipe commands into it:

```bash
echo -e "bup\nquit" | ./target/debug/bughouse_engine
```

---

## Planned source layout

The engine will grow into this module structure inside `src/`:

```
src/
├── main.rs          # Entry point: BUP I/O loop (read line → dispatch → write line)
├── bup.rs           # BUP protocol parser & formatter
├── bfen.rs          # BFEN parser & emitter
├── board.rs         # Bitboard board state (u64 per piece type × colour)
├── moves.rs         # Move generation (pseudo-legal + legality filter)
├── search.rs        # Tree search (minimax/alpha-beta or MCTS)
├── scoring.rs       # Static evaluation function (material, position, ...)
├── bughouse.rs      # Bughouse-specific logic: drops, reserve tracking, demotion
└── time.rs          # Clock management: time allocation per move
```

Each module has a single responsibility. The dependency graph flows top-down: `main` → `bup` → `bfen`/`board` → `moves` → `search`/`scoring`. `bughouse` is a cross-cutting concern that touches `board`, `moves`, and `scoring`.

---

## Roadmap

Development proceeds in four phases, each building on the last:

### Phase B — Random-move bot (current)

The minimum viable engine. Parses BUP commands, maintains board state via BFEN, generates all legal moves, picks one at random, and returns `bestmove`. This validates the entire plumbing: Rust binary ↔ Elixir Port ↔ BUP protocol ↔ BFEN parsing ↔ move generation.

**Deliverables:**
- `bup.rs` — full handshake + command dispatch
- `bfen.rs` — parse and emit spec-compliant BFEN (including `~` and `[]`)
- `board.rs` — bitboard representation, FEN ↔ board conversion
- `moves.rs` — pseudo-legal move gen + legality filter (king not left in check)
- `bughouse.rs` — drop move generation from reserves
- `main.rs` — I/O loop that ties it all together

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

## Tech notes

| Topic | Detail |
|-------|--------|
| Rust edition | 2024 |
| Target | `aarch64-apple-darwin` (Apple Silicon) |
| Bitboard width | 64-bit (`u64`) — one per piece-type × colour |
| Protocol | Line-based stdin/stdout (no network sockets) |
| Concurrency | Single-threaded for now; search parallelism is a Phase D+ consideration |
