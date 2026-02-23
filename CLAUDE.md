# CLAUDE.md — Contributing guide for bughouse-engine

## What this is

A Rust bughouse chess engine that communicates over stdin/stdout using the UBI protocol and understands BFEN notation. Currently at Phase C (1-ply search with static evaluation). See `README.md` for full architecture and roadmap.

## Build & test

```bash
cargo build              # debug build
cargo build --release    # optimized build
cargo test               # run all unit tests (86 currently)
cargo run                # interactive mode — type UBI commands, Ctrl-D to exit
```

Always run `cargo test` after changes. All tests must pass before committing.

## Project structure

```
src/
├── main.rs          # Thin I/O loop only. No logic here.
├── ubi.rs           # UBI protocol parsing & formatting (pure data, no I/O)
├── game_state.rs    # EngineState, command dispatch, move selection (no I/O)
├── strategy.rs      # PlayStyle enum + time-aware style selection
├── scoring.rs       # Static evaluation function (material, PSTs, king safety, etc.)
└── search.rs        # 1-ply search with drop pruning, checkmate scoring
docs/
├── BFEN.md          # Bughouse FEN spec v0.1 (authoritative)
└── UBI.md           # Universal Bughouse Interface spec v0.1 (authoritative)
```

Future files (not yet created):
- `src/time.rs` — clock management and time-aware strategy (Phase D)

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
- **Phase C**: `scoring.rs` — material counting, positional tables, 1-ply search
- **Phase D**: `search.rs` — minimax with alpha-beta, iterative deepening; `time.rs` — time budget allocation
- **Phase E**: Bughouse-specific tuning — reserve valuation, partner coordination, time-aware stalling (see "Evaluation strategy" in README)

The time-aware evaluation framework (README § "Evaluation strategy") defines four `TimeState` variants (`Disadvantage`, `PotentialAdvantage`, `MildDisadvantage`, `LocalAdvantage`) that should drive search budget and move selection strategy.

## Commit conventions

- Keep commits focused on a single concern
- Prefix commit messages with the area: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`
- Run `cargo test` before committing — all tests must pass

## Updating the README

When adding significant new functionality or changing architecture, update `README.md` to reflect the changes. Keep the roadmap section current with what's been implemented.
