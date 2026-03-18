# CLAUDE.md — Contributing guide for bughouse-engine

## What this is

A Rust bughouse chess engine that communicates over stdin/stdout using the UBI protocol and understands BFEN notation. Currently at Phase D (alpha-beta search with iterative deepening, P/C computation). See `README.md` for full architecture and roadmap.

## Build & test

```bash
cargo build              # debug build
cargo build --release    # optimized build
cargo test               # run all unit tests (104 currently)
cargo run                # interactive mode — type UBI commands, Ctrl-D to exit
```

Always run `cargo test` after changes. All tests must pass before committing.

## Project structure

```
src/
├── main.rs          # Thin I/O loop only. No logic here.
├── ubi.rs           # UBI protocol parsing & formatting (pure data, no I/O)
├── game_state.rs    # EngineState, command dispatch, move selection (no I/O)
├── engine.rs        # Multi-threaded eval threads with pondering
├── book.rs          # Opening book: bughouse theory lines with weighted random selection
├── config.rs        # EngineConfig: all tunable parameters, setoption handling
├── strategy.rs      # PlayStyle enum + time-aware style selection
├── scoring.rs       # Static evaluation function (material, PSTs, king safety, etc.)
├── search.rs        # Alpha-beta negamax with iterative deepening, P/C computation
└── time.rs          # Time budget allocation from clock state
docs/
├── BFEN.md          # Bughouse FEN spec v0.1 (authoritative)
└── UBI.md           # Universal Bughouse Interface spec v0.1 (authoritative)
```

Future files (not yet created):
- Cross-board strategy layer module (Phase E)

## Architecture principles

1. **Pure data flow**: Parsing → enums → dispatch → formatting. No I/O in `ubi.rs` or `game_state.rs`. Only `main.rs` touches stdin/stdout.
2. **Thin main loop**: `main.rs` is glue code. All logic lives in modules.
3. **Protocol specs are authoritative**: `docs/UBI.md` and `docs/BFEN.md` define the protocol. Code must conform to the specs, not the other way around.

## Key conventions

- **Rust 2024 edition** — see `Cargo.toml`
- **No `unwrap()` in production paths** — use proper error handling. `unwrap()` is acceptable in tests.
- **Unit tests go in the same file** as the code they test, inside a `#[cfg(test)] mod tests` block.
- **Move notation**: Regular moves use coordinate notation (`e2e4`, `e1g1` for castling, `e7e8q` for promotion). Drop moves use `@` notation with lowercase piece (`p@e4`, `n@f3`).
- **BFEN reserves**: Always canonical order `QRBNPqrbnp`. Empty reserves = `[]`. Promoted pieces marked with `~`.
- **Board indexing**: Board A = index 0, Board B = index 1. Four clocks: `[white_A, black_A, white_B, black_B]`.

## External dependency: bughouse-chess

When looking up bughouse-chess library source code, check `../bughouse-chess` first (the local sibling checkout). Only fall back to `~/.cargo/git/checkouts/bughouse-chess-*` if the local checkout isn't there.

The engine depends on [bughouse-chess](https://github.com/vcsawant/bughouse-chess) — a fork of `jordanbray/chess` adapted for bughouse. It provides:
- Bitboard board representation with reserves
- Legal move generation (regular + drops)
- BFEN parsing/emission
- Promoted-piece tracking and capture demotion

Linked via git in `Cargo.toml`. Changes to the `bughouse-chess` library are encouraged — if the engine needs new capabilities (e.g. new board queries for evaluation, additional move metadata, reserve introspection), modify the library directly at `bughouse-chess` and update the dependency. The library and engine are designed to evolve together.

## Adding a new UBI command

1. Add a variant to `UbiCommand` enum in `ubi.rs`
2. Add parsing logic in `parse_command()` (or a new `parse_*()` helper)
3. Add handling in `game_state.rs` `process_command()` dispatch
4. Add unit tests in both `ubi.rs` and `game_state.rs`
5. Update `docs/UBI.md` if this is a new protocol addition

## Adding evaluation / search logic

Follow the roadmap phases in order:
- **Phase C** (done): `scoring.rs` — material counting, positional tables, 1-ply search
- **Phase D**: `search.rs` — multi-ply alpha-beta with iterative deepening, P/C computation as search byproducts; `time.rs` — time budget allocation
- **Phase E**: Cross-board strategy layer — combines evaluations from both boards, cross-board minimax, aggressiveness threshold, wait-vs-move logic, stall move selection

The evaluation architecture (README § "Evaluation architecture: dual-process model") defines the three-output per-board evaluation model (move evals, P, C) and the strategy layer that combines them. The four `TimeState` variants (`Disadvantage`, `PotentialAdvantage`, `MildDisadvantage`, `LocalAdvantage`) drive search budgets and aggressiveness thresholds.

**Key design rule**: Evaluation (scoring, search, P, C) is per-board and deterministic. Cross-board reasoning belongs in the strategy layer only.

## Commit conventions

- Keep commits focused on a single concern
- Prefix commit messages with the area: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`
- Run `cargo test` before committing — all tests must pass

## Updating the README

When adding significant new functionality or changing architecture, update `README.md` to reflect the changes. Keep the roadmap section current with what's been implemented.
