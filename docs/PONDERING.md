# Pondering Architecture

Multi-threaded evaluation and search for the bughouse engine.

## Thread Model

```
┌─────────────────────────────────────────────────────────┐
│  Main Thread (I/O only)                                 │
│  stdin → parse → route commands → write stdout          │
│  Routes: position → eval threads, go → search thread    │
├─────────────────────────────────────────────────────────┤
│  Eval Thread A              │  Eval Thread B            │
│  Owns TT for board A        │  Owns TT for board B      │
│  Continuous iterative        │  Continuous iterative      │
│  deepening                   │  deepening                 │
│  Produces: root moves,      │  Produces: root moves,     │
│  P/C, reserve_impact,       │  P/C, reserve_impact,      │
│  info lines                  │  info lines                │
├─────────────────────────────────────────────────────────┤
│  Search Thread (unified, cross-board)                   │
│  Walks both TTs to find capture sequences               │
│  Applies strategy: time-aware, reserve-aware            │
│  Maintains "current best answer" for each board         │
│  Continuously refines as eval threads reach new depths  │
└─────────────────────────────────────────────────────────┘
```

## Eval Threads

Each board has a dedicated eval thread that runs iterative deepening continuously. The eval thread:

- **Owns its TT** — no sharing with the other board (positions have different Zobrist hashes, no benefit to sharing). Eliminates all TT synchronization.
- **Restarts on position changes** — when a `position` command changes the board's hash (new move or reserve change), the eval thread abandons the current search and starts from depth 1.
- **Produces BoardEval at each depth** — P/C capture statistics, reserve impact (only for pieces not in reserves), score, depth.
- **Accumulates info lines** — one per completed depth, for streaming to the GUI when `go` arrives.
- **Responds to commands** via channel: `NewPosition`, `SetDeadline`, `Pause`, `Resume`, `Quit`.

### Reserve Impact

Computed after each completed depth. For each of the 5 non-king piece types (Pawn, Knight, Bishop, Rook, Queen):
- If the piece is already in our reserves, **skip** — the search already considers drops with it.
- If the piece is NOT in our reserves, clone the board, add the piece via `add_to_reserve`, do a shallow re-search (depth 3), and measure the score delta.

This answers: "how much would having piece X improve this position?" Used by the search thread to evaluate cross-board piece flow.

## Search Thread (Final Vision)

Runs continuously once both eval threads have reached depth 2+. The search thread:

- **Walks both boards' TTs** to understand not just root move scores but the full principal variation — what captures happen deeper in each line, which pieces enter reserves.
- **Applies cross-board strategy** — adjusts root move scores based on partner's P/C values and our reserve impact. "This capture sends a knight to partner's reserves, and partner's reserve_impact[knight] = +200cp → bonus."
- **Maintains a current best answer** for each board. When an eval thread completes a new depth, the search thread re-evaluates with the updated data.
- **Handles `go` commands** — the strategy determines a time cap based on clocks. If low on time, return immediately. If plenty of time, let eval threads deepen further before finalizing.

### TT Access

When the search thread needs to walk a board's TT, the eval thread for that board pauses. The search thread reads the TT directly (no concurrent access). After the search thread is done, the eval thread resumes pondering.

## Bridge Implementation

The initial implementation establishes threading without the continuous search thread:

- **Eval threads** run continuously (pondering).
- **Cross-board logic** runs synchronously on the main thread when `go` arrives.
- When `go board A`:
  1. Signal eval thread A: deadline in X ms
  2. Wait (condvar) for eval thread A to pause
  3. Read root moves, BoardEval, info lines
  4. Peek eval thread B's BoardEval
  5. Apply cross-board adjustments (stub initially)
  6. Pick best move, return bestmove
  7. Resume eval thread A

The continuous search thread replaces step 3-6 in a future upgrade.

## Communication

```rust
// Commands from main thread to eval thread
enum EvalCommand {
    NewPosition(Board),
    SetDeadline(Instant),
    Pause,
    Resume,
    Quit,
}

// Eval thread publishes its status for the main/search thread to read
struct EvalStatus {
    board_hash: u64,
    best_move: Option<BughouseMove>,
    best_score: i32,
    completed_depth: u32,
    eval: BoardEval,
    root_moves: Vec<RootMoveInfo>,
    info_lines: Vec<SearchInfo>,
    searching: bool,
}
```

Commands via `mpsc::Sender`. Status via `Arc<Mutex<EvalStatus>>` with a `Condvar` for the main thread to sleep-wait on pause completion.
