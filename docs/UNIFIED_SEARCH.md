# Unified Cross-Board Search

An alternate search architecture for bughouse that treats both boards as a single game, eliminating the need for P/C values and a separate strategy layer.

## Motivation

The current architecture (Phase D/E) searches each board independently and uses a strategy layer with P/C values to coordinate between boards. This works but has a fundamental limitation: the per-board search can't see cross-board tactics like "capture a knight on board B because it enables a mating drop on board A."

P/C (piece acquisition probability and cost) is an approximation of what the search would discover naturally if it could see both boards at once.

## Core Idea

Instead of two independent search trees with a strategy layer on top, run a single search over the combined game state `(board_a, board_b, turn_a, turn_b)`. At each node, the current team can move on whichever boards it's their turn. Reserve flow (captures on one board feeding drops on the other) emerges naturally from the search — no special cross-board logic needed.

## Combined State

```rust
struct CombinedState {
    board_a: Board,
    board_b: Board,
    turn_a: Team,   // which team moves next on board A
    turn_b: Team,   // which team moves next on board B
}
```

Hash: `board_a.get_hash() ^ board_b.get_hash() ^ turn_encoding`

## Turn Model

At any node, team 1 can move on whichever boards have `turn_x == Team1`. Three cases:

**Only one board is theirs:** Normal single-board move. ~50 options.

**Both boards are theirs:** The move set includes:
- Move only on board A (~50 options) — stall on B
- Move only on board B (~50 options) — stall on A
- Move on both boards (~50 x 50 pairs, order matters for captures since they change reserves)

This models real bughouse timing: board A might see 5 moves while board B sees 1, because team 1 keeps "stalling" on B (choosing single-board moves on A).

### Why stalling matters

If board A has checkmate in 5, board B should recognize that not moving (or playing quiet, non-committal moves) is optimal. The search discovers this naturally — moving on both boards gives the opponent a chance to create counterplay on B, while stalling on B preserves the advantage.

## Alpha-Beta Pruning

Alpha-beta still applies because we enforce alternation between teams. Within a team's ply:
- Board A moves maximize for the moving team
- Board B moves maximize for the moving team (which may be minimizing from our perspective if it's the opponent's board)

The key insight: a "double move" (moving on both boards) is treated as a single ply for the team. The opposing team then gets their ply. This preserves the minimax structure.

## Branching Factor

**Raw worst case:** ~5,100 (50 + 50 + 50x50x2) when a team can move on both boards.

**With move ordering and pruning to top ~7-10 candidates per board:**
- Single moves: 7 + 7 = 14
- Pairs: 7 x 7 = 49 (only ~10 need ordering consideration for captures)
- Total: ~63-70 options per combined ply

**With alpha-beta:** Effective branching ~sqrt(70) = ~8 per ply.

**At 4 combined plies (2 full rounds):** 8^4 = ~4,096 leaf evaluations.

This is comparable to or less than the current per-board search's node count, and each leaf can use a per-board TT for deep tactical evaluation.

## Two-Layer Architecture

```
Combined Search (meta-depth 2-3 combined plies)
├── Interleaves moves across both boards
├── Handles reserve flow naturally (captures on B feed drops on A)
├── Combined TT keyed by (board_a_hash, board_b_hash, turns)
├── Alpha-beta pruning at the combined level
└── Calls per-board eval at leaves
        │
Per-Board Evaluation (depth 6-8 with per-board TT)
├── Deep tactical search within one board
├── Standard alpha-beta with per-board TT
├── Called at combined search leaves
└── Runs in background (pondering) to keep TT warm
```

The per-board search provides deep tactical vision. The combined search provides cross-board strategic vision. No P/C values or strategy layer needed.

## Advantages Over Current Architecture

- **No P/C approximation.** Cross-board tactics emerge from the search.
- **No strategy layer.** The search IS the strategy.
- **Stall detection is natural.** The search discovers when not moving is optimal.
- **Reserve flow is exact.** Captures on one board immediately affect the other board's reserves in the search tree.
- **Combined TT captures cross-board transpositions.** Different orderings of moves across boards that reach the same combined state get free lookups.

## Disadvantages / Risks

- **Higher implementation complexity.** Turn tracking, stall modeling, and combined alpha-beta are harder than standard per-board search.
- **Partner modeling.** With a human partner, we don't control one board. Need to assume reasonable play or use a partner model.
- **Time dimension.** Real bughouse has clock pressure that affects when to stall. The search tree doesn't model real time — stalling has no cost in the tree but does on the clock.
- **Branching factor sensitivity.** If move pruning isn't aggressive enough, the combined branching factor can explode.

## Implementation Plan

1. Build per-board TT first (needed regardless — it's the leaf evaluator for both architectures)
2. Create a new binary target in the workspace for the unified search bot
3. Implement `CombinedState` with proper hashing
4. Implement combined move generation (single moves + pairs + stalls)
5. Implement combined alpha-beta with per-board TT at leaves
6. Test by having the two bots play against each other

## Comparison Testing

Both bot variants speak UBI and can be spawned by the same game server. To compare:
- Have unified-search bot play against current per-board bot
- Measure: win rate, average search depth, time per move, quality of cross-board decisions
- Key positions to test: mate-in-N requiring cross-board piece transfer, stall-vs-move decisions, reserve timing
