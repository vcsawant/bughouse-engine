# Universal Bughouse Interface (UBI) Protocol Specification

**Version:** 1.1
**Date:** July 25, 2026
**Author:** Viren Sawant

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Design Philosophy](#2-design-philosophy)
3. [Communication](#3-communication)
4. [Commands: GUI to Engine](#4-commands-gui-to-engine)
5. [Commands: Engine to GUI](#5-commands-engine-to-gui)
6. [Move Notation](#6-move-notation)
7. [Search State Management](#7-search-state-management)
8. [Team Communication](#8-team-communication-optional)
9. [Example Sessions](#9-example-sessions)
10. [Implementation Notes](#10-implementation-notes)
11. [FAQ](#11-faq)

---

## 1. Introduction

### 1.1 Purpose

The Universal Bughouse Interface (UBI) is a text-based protocol for communication between bughouse chess graphical user interfaces (GUIs) and chess engines. It is designed specifically for bughouse chess, a four-player, two-board chess variant.

### 1.2 Relation to UCI

UBI is inspired by the Universal Chess Interface (UCI) protocol but adapted for bughouse-specific requirements:
- Dual board positions
- Piece reserves (captured pieces available for dropping)
- Drop moves
- Four independent clocks
- Full-state position updates
- Optional team coordination

### 1.3 Key Differences from UCI

| Feature | UCI | UBI |
|---------|-----|-----|
| Boards | 1 | 2 (A and B) |
| Position Notation | FEN | BFEN (with reserves) |
| Move Types | Regular only | Regular + drops |
| Clocks | 2 (white, black) | 4 (white_A, black_A, white_B, black_B) |
| Position Updates | Incremental (with moves) | Full state (both boards + all clocks) |
| Time Management | GUI supplies limits per `go` | Engine-driven from full clock state |
| Team Play | N/A | Optional team messages |
| Hash Tables | Position only | Position + reserves |

### 1.4 Conventions

Throughout this document:
- `<text>` denotes a required parameter
- `[text]` denotes an optional parameter
- `|` denotes alternatives (except in position command where it is a literal separator)
- Lines are terminated with newline (`\n`)
- All communication is in UTF-8 encoding

---

## 2. Design Philosophy

### 2.1 Position-Agnostic Engine

The engine does not need to know:
- Which team it is on
- Which color(s) it is playing
- Whether it has a partner
- Its role in the game

The engine only needs:
- Current positions of both boards
- Piece reserves for all players
- Clock times for all players
- Which board(s) to analyze

The GUI handles:
- Whose turn it is
- Which engine to query
- Routing team messages between partners
- Game state and flow management

This separation of concerns simplifies engine implementation and testing. There is deliberately no capability negotiation: like a UCI engine, a UBI engine is never told what role it plays. `go` commands mean "evaluate this position and answer with a move" — the GUI decides what to do with the answer.

### 2.2 Full-State Updates

Unlike UCI's incremental position updates (`position ... moves ...`), UBI always sends the complete game state:
- Both board positions (as BFEN strings)
- All reserves (encoded in the BFEN)
- All four clock times

This design eliminates state synchronization issues and simplifies the protocol.

### 2.3 Per-Board Search State

Each board (A and B) has independent search state:
- **IDLE**: Not currently searching
- **SEARCHING**: Actively calculating best move

An engine can search one or both boards simultaneously.

### 2.4 Time Is First-Class

UBI engines are built to *play* bughouse, not merely evaluate it. Bughouse is won and lost on the clock: a team can be checkmated on one board because the other board fed pieces too slowly, and "sitting" (declining to move) is a legitimate strategic device. UCI treats time as a per-search limit supplied with `go`; UBI instead treats the four-clock state as part of the game state, delivered with every `position` update. Engines are expected to own their time management — deciding how long to think from the full clock picture (their own clock, their direct opponent's, and both clocks on the partner board).

---

## 3. Communication

### 3.1 Transport

- **Method:** Standard input/output (stdin/stdout)
- **Format:** Line-based text protocol
- **Encoding:** UTF-8
- **Termination:** Newline character (`\n`)

### 3.2 Direction

- **GUI → Engine:** Commands to control engine behavior
- **Engine → GUI:** Responses, search information, best moves, and team messages

### 3.3 Timing

- Commands are asynchronous
- Engine can send multiple `info` messages while searching
- Engine must process commands in the order received

---

## 4. Commands: GUI to Engine

| Command | Description |
|---------|-------------|
| `ubi` | Initialize UBI mode |
| `isready` | Check if engine is ready |
| `setoption name <id> [value <x>]` | Set engine parameter |
| `ubinewgame` | Start a new game |
| `position <bfen_a> \| <bfen_b> clock <wA> <bA> <wB> <bB>` | Set complete game state |
| `partnermsg <type> [params]` | Forward partner's team message |
| `go board <A\|B> [searchparams]` | Start search on a board |
| `stop [board <A\|B>]` | Cancel search |
| `metadata <key> <value>` | Attach match/game metadata (optional) |
| `quit` | Terminate engine |

### 4.1 `ubi`

**Description:** Initialize UBI mode.

**Format:**
```
ubi
```

**Response:** The engine must identify itself and send options:
```
id name <engine_name>
id author <author_name>
[option <definition>]*
ubiok
```

**Example:**
```
GUI → Engine: ubi
Engine → GUI: id name BughouseBot 1.0
Engine → GUI: id author Alice Smith
Engine → GUI: option name Hash type spin default 128 min 16 max 4096
Engine → GUI: option name Threads type spin default 1 min 1 max 64
Engine → GUI: option name TeamMessageMode type combo default consider var ignore var consider var full
Engine → GUI: ubiok
```

**Notes:**
- This must be the first command sent to the engine
- The engine should not send any output before receiving this command
- The engine must send `ubiok` after all identification and options

---

### 4.2 `isready`

**Description:** Synchronization command to check if engine is ready.

**Format:**
```
isready
```

**Response:**
```
readyok
```

**Notes:**
- Engine must finish processing all previous commands before responding
- Used by GUI to ensure engine is ready for new commands
- Engine must always respond with `readyok`

---

### 4.3 `setoption`

**Description:** Set engine parameters.

**Format:**
```
setoption name <id> [value <x>]
```

**Parameters:**
- `<id>`: Name of the option
- `<x>`: Value to set (optional for button-type options)

**Example:**
```
setoption name Hash value 256
setoption name Threads value 4
setoption name TeamMessageMode value full
```

**Notes:**
- Typically sent after `ubiok` and before `ubinewgame`
- Engines MAY accept `setoption` at any time, including mid-game — this supports runtime tuning and benchmarking. An engine that cannot apply an option mid-game should ignore it rather than error
- Values must match the option's type and constraints
- For button-type options, no value is needed
- Engines should silently ignore unknown option names

**Standard Option Types:**

| Type | Description | Example |
|------|-------------|---------|
| `spin` | Integer with min/max range | `option name Hash type spin default 128 min 16 max 4096` |
| `combo` | Selection from predefined values | `option name Style type combo default normal var aggressive var normal` |
| `check` | Boolean flag | `option name OwnBook type check default false` |
| `string` | Text value | `option name BookFile type string default <empty>` |
| `button` | Trigger action (no value) | `option name ClearHash type button` |

**Standard Options:**
- `Hash` — Hash table size in MB
- `Threads` — Number of search threads
- `TeamMessageMode` — Team communication mode (see Section 8)

---

### 4.4 `ubinewgame`

**Description:** Start a new game. Engine should clear hash tables and reset state.

**Format:**
```
ubinewgame
```

**Response:** None required.

**Notes:**
- Sent at the start of each new game
- Engine should clear transposition tables, history, etc.
- Previous game state should be completely discarded

---

### 4.5 `position`

**Description:** Set the complete game state for both boards.

**Format:**
```
position <bfen_board_a> | <bfen_board_b> clock <white_A> <black_A> <white_B> <black_B>
```

**Parameters:**
- `<bfen_board_a>`: BFEN string for board A (or `startpos`)
- `<bfen_board_b>`: BFEN string for board B (or `startpos`)
- `<white_A>`: White's remaining time on board A (milliseconds)
- `<black_A>`: Black's remaining time on board A (milliseconds)
- `<white_B>`: White's remaining time on board B (milliseconds)
- `<black_B>`: Black's remaining time on board B (milliseconds)

**BFEN Format:**
```
<position>[<reserves>] <color> <castling> <ep> <halfmove> <fullmove>
```

See [BFEN 2.0 Specification](./BFEN.md) for details.

**Examples:**
```
position rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 | rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 clock 180000 180000 180000 180000
```

**Shorthand for starting position:**
```
position startpos | startpos clock 180000 180000 180000 180000
```

Which is equivalent to:
```
position rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 | rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 clock 180000 180000 180000 180000
```

**Mixed shorthand is also valid:**
```
position startpos | r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5 clock 180000 180000 165000 168000
```

**Notes:**
- Always sends complete state for both boards
- Can be sent during search (see Section 7: Search State Management)
- Engine must update internal state immediately
- The `|` separator must have exactly one space before and after
- The `|` character never appears in valid BFEN, making it an unambiguous delimiter

---

### 4.6 `partnermsg`

**Description:** Forward a team message from the engine's partner. Format identical to `teammsg`.

**Format:**
```
partnermsg <message_type> [params]
```

See Section 8 for message types and parameters.

**Examples:**
```
partnermsg need b urgency medium
partnermsg threat high
partnermsg stall duration 1
partnermsg material +200
partnermsg play_fast reason time
```

**Notes:**
- GUI only sends `partnermsg` when:
  1. Engine has a partner (not playing both boards)
  2. Partner engine sent a `teammsg`
  3. Engine's `TeamMessageMode` is not `ignore`
- Engine should consider partner messages in its evaluation (see Section 8)

---

### 4.7 `go`

**Description:** Start calculating the best move for the specified board.

**Format:**
```
go board <A|B> [searchparams]
```

**Parameters:**
- `board <A|B>`: Which board to analyze (required)

**Time management:** The core form is a bare `go board <X>`. UBI engines own their time management (see Section 2.4): the engine decides how long to think using the four clock values from the most recent `position` command. This is a deliberate departure from UCI, where the GUI supplies time limits with every `go`. A well-behaved engine should move faster when short on time, may invest more when holding a time advantage over its direct opponent, and should account for the partner board's clocks when both boards are under its control.

**Search Parameters (optional hints):**

| Parameter | Description |
|-----------|-------------|
| `movetime <ms>` | Search for exactly this duration |
| `depth <n>` | Search to specified depth in plies |
| `nodes <n>` | Search up to this many nodes |
| `infinite` | Search until `stop` command |

**Examples:**
```
go board A
go board B
go board A movetime 3000
go board A depth 12
```

**Notes:**
- Search parameters are hints intended for analysis, testing, and benchmarking. Engines MAY ignore them in normal play — the clock state from `position` is the authoritative time source
- If an engine honors search parameters, whichever limit is reached first ends the search
- Transitions specified board from IDLE to SEARCHING state
- Multiple `go` commands can be active simultaneously (one per board)

---

### 4.8 `stop`

**Description:** Cancel the pending search on one or both boards.

**Format:**
```
stop [board <A|B>]
```

**Variants:**
- `stop` — Cancel all active searches (both boards)
- `stop board A` — Cancel search on board A only
- `stop board B` — Cancel search on board B only

**Response:** None. The engine aborts the search for the specified board(s) and transitions them to IDLE **without sending `bestmove`**. This matches the internal-abort behavior in Section 7.3: a cancelled search never produces a move. If the GUI wants a move after cancelling, it sends a fresh `go`.

**Notes:**
- Engine must process the cancellation quickly (within a few milliseconds)
- If no search is active for the specified board, the command is ignored
- Typical uses: invalidating a pending `go` before a position update, or ending an `infinite` analysis session

---

### 4.9 `metadata`

**Description:** Attach arbitrary key-value metadata to the engine instance. Metadata is purely informational — engines store it for logging and analysis but do not act on it.

**Format:**
```
metadata <key> <value>
```

Keys and values are freeform strings; the engine does not validate them. Metadata persists for the lifetime of the engine process. Each `metadata` command sets or overwrites the value for that key.

**Examples:**
```
metadata match_id bench_20260318_001
metadata total_games 100
metadata time_control 120000
metadata opponent baseline_stable_v1
metadata game_number 42
metadata position white_A,black_B
```

**Conventional Keys:**

These keys are not required, but using them enables consistent log analysis across different GUIs:

| Key | Description | Example |
|-----|-------------|---------|
| `match_id` | Unique identifier for a batch of games | `bench_20260318_001` |
| `total_games` | Number of games in the match | `100` |
| `time_control` | Starting time in milliseconds | `120000` |
| `opponent` | Opposing team identifier | `baseline_stable_v1` |
| `game_number` | Current game number in the match | `42` |
| `position` | Board seats this engine plays (comma-separated) | `white_A,black_B` |
| `game_result` | Outcome of the game | `win`, `loss`, `draw`, `timeout` |

**Game Result:** When the GUI sends `metadata game_result <value>`, the engine should log a game summary. This is the signal that the game has ended.

**Engine behavior:**
- Store all metadata key-value pairs
- Include metadata in structured log output (game start/end markers)
- Do not change search, evaluation, or move selection based on metadata
- Gracefully handle unknown keys (store and log them like any other)

---

### 4.10 `quit`

**Description:** Quit the engine as soon as possible.

**Format:**
```
quit
```

**Response:** None required.

**Notes:**
- Engine should exit cleanly
- No need to send any final messages
- GUI should wait for process to terminate

---

## 5. Commands: Engine to GUI

| Command | Description |
|---------|-------------|
| `id name <name>` | Engine name |
| `id author <author>` | Engine author |
| `option name <id> type <type> ...` | Declare configurable option |
| `ubiok` | Initialization complete |
| `readyok` | Ready acknowledgment |
| `info board <A\|B> [key value]*` | Search information |
| `bestmove board <A\|B> <move>` | Best move found |
| `teammsg <type> [params]` | Team message to partner |

### 5.1 `id`

**Description:** Identify the engine during initialization.

**Format:**
```
id name <engine_name>
id author <author_name>
```

**Notes:**
- Sent in response to `ubi` command
- Must be sent before `ubiok`
- Both `name` and `author` are required

---

### 5.2 `option`

**Description:** Declare an engine option that can be configured by the GUI.

**Format:**
```
option name <id> type <type> [default <x>] [min <x>] [max <x>] [var <x>]*
```

**Notes:**
- Sent during `ubi` initialization, before `ubiok`
- All options must be declared upfront
- GUI can set options using `setoption` command

---

### 5.3 `ubiok`

**Description:** Acknowledge completion of UBI initialization.

**Format:**
```
ubiok
```

**Notes:**
- Sent after all `id` and `option` commands
- Signals engine is ready to receive commands

---

### 5.4 `readyok`

**Description:** Acknowledge that engine is ready.

**Format:**
```
readyok
```

**Notes:**
- Sent in response to `isready` command
- Indicates all previous commands have been processed

---

### 5.5 `info`

**Description:** Send search information to the GUI.

**Format:**
```
info board <A|B> [<key> <value>]*
```

**Standard Info Keys:**

| Key | Description |
|-----|-------------|
| `depth <n>` | Current search depth in plies |
| `seldepth <n>` | Selective search depth (max depth reached) |
| `time <ms>` | Time spent searching in milliseconds |
| `nodes <n>` | Number of nodes searched |
| `nps <n>` | Nodes per second |
| `score cp <n>` | Evaluation in centipawns (positive = good for side to move) |
| `score mate <n>` | Mate in n moves (positive = we mate, negative = we're mated) |
| `pv <move1> ... <moveN>` | Principal variation (best line) |

**Bughouse-Specific Info Keys:**

| Key | Description |
|-----|-------------|
| `reserve_value <n>` | Estimated value of engine's reserves in centipawns |
| `partner_reserve_value <n>` | Estimated value of partner's reserves |
| `string <message>` | Arbitrary text message for display |

**Examples:**
```
info board A depth 12 nodes 150000 nps 75000 time 2000 score cp 45 pv e2e4 e7e5 g1f3
info board B depth 10 score cp -20 reserve_value 300
info board A depth 15 score mate 5 pv n@e5 f6e4 q@h5 g7g6 h5e5
```

**Notes:**
- Multiple key-value pairs can be sent in one `info` command
- Engine can send `info` at any time during search
- `info` commands are optional and for informational purposes only

---

### 5.6 `bestmove`

**Description:** Report the best move found for a board.

**Format:**
```
bestmove board <A|B> <move>
```

**Parameters:**
- `board <A|B>`: Which board this move is for
- `<move>`: Move in algebraic notation (see Section 6)

**Examples:**
```
bestmove board A e2e4
bestmove board B n@f3
bestmove board A p@e5
```

**No legal moves:**
```
bestmove board A (none)
```

**Notes:**
- Sent when a search completes normally (time limit, depth reached, or other search-parameter limit)
- Never sent in response to `stop` or an internal abort (see Sections 4.8 and 7.3)
- Automatically transitions that board from SEARCHING to IDLE
- Engine should stop thinking about that board after sending `bestmove`
- Other board (if searching) continues unaffected
- Must be a legal move in the current position
- If no legal move exists (checkmate/stalemate), send `bestmove board <X> (none)`

---

### 5.7 `teammsg`

**Description:** Send a coordination message to the engine's partner.

**Format:**
```
teammsg <message_type> [params]
```

See Section 8 for message types and parameters.

**Notes:**
- GUI routes this to the partner engine as `partnermsg`
- Only sent when `TeamMessageMode` is `full`
- Engine can send team messages at any time (typically alongside `bestmove`)

---

## 6. Move Notation

### 6.1 Regular Moves

UBI uses long algebraic notation (same as UCI):

```
e2e4    — Pawn from e2 to e4
g1f3    — Knight from g1 to f3
e1g1    — Castling kingside (O-O)
e1c1    — Castling queenside (O-O-O)
e7e8q   — Pawn promotion to queen
e7e8r   — Pawn promotion to rook
e7e8b   — Pawn promotion to bishop
e7e8n   — Pawn promotion to knight
```

### 6.2 Drop Moves

**Format:** `<piece>@<square>` (lowercase piece letter)

```
p@e4    — Drop pawn at e4
n@f3    — Drop knight at f3
b@c4    — Drop bishop at c4
r@a1    — Drop rook at a1
q@d5    — Drop queen at d5
```

**Note:** UBI mandates lowercase piece letters for drops. This differs from the crazyhouse UCI convention (e.g. Fairy-Stockfish, Lichess), which uses uppercase (`N@f3`). GUIs bridging to crazyhouse tooling should normalize case.

**Legality Requirements:**
1. Piece must exist in player's reserve
2. Target square must be empty
3. Cannot drop pawn on 1st or 8th rank
4. Drop checkmate is legal (a piece drop that delivers checkmate wins the game)

### 6.3 Special Cases

**No legal move (checkmate/stalemate):**
```
bestmove board A (none)
```

---

## 7. Search State Management

### 7.1 Per-Board State Model

Each board (A and B) maintains independent state:

**IDLE:**
- Not currently searching
- Waiting for `go` command
- Initial state after `ubinewgame`

**SEARCHING:**
- Actively calculating best move
- Sending `info` messages
- Can be interrupted by `position` updates

### 7.2 State Transitions

```
IDLE → SEARCHING    go board <X>
SEARCHING → IDLE    bestmove board <X> (automatic, search completed)
SEARCHING → IDLE    stop [board <X>] (manual cancel — no bestmove)
SEARCHING → IDLE    Internal abort due to position invalidation (no bestmove)
```

### 7.3 Position Updates During Search

When engine receives `position` command while searching:

**Engine MUST:**
- Update internal representation of both boards
- Update all clock times

**Engine SHOULD abort search if:**
- Position of the board being analyzed has changed
- Reserves on the board being analyzed have changed

**Engine MAY continue search if:**
- Only partner board position changed (context update)
- Only clock times changed (time management update)

**If engine aborts search internally:**
- Transition board to IDLE state
- Do NOT send `bestmove`
- Wait for next `go` command

### 7.4 Pondering

Engines MAY think on the opponent's time. Because every `position` command delivers the full game state, a pondering engine simply keeps analyzing the current position between `go` commands and answers from its accumulated work when `go` arrives. No `ponderhit` mechanism is needed: a `position` update that changes a board invalidates any pondering on it (Section 7.3), and pondering produces no output until a `go` is answered.

### 7.5 Multiple Simultaneous Searches

An engine can search both boards simultaneously:

```
go board A
go board B
```

**State:** Both A and B are SEARCHING

When engine completes search on board A:
```
bestmove board A e2e4
```

**State:** A is IDLE, B is still SEARCHING

Engine continues calculating for board B independently.

### 7.6 Example State Transitions

```
Initial: A=IDLE, B=IDLE

GUI → go board A
State: A=SEARCHING, B=IDLE

Engine → bestmove board A e2e4
State: A=IDLE, B=IDLE

GUI → go board A
GUI → go board B
State: A=SEARCHING, B=SEARCHING

Engine → bestmove board B d2d4
State: A=SEARCHING, B=IDLE

GUI → stop board A        (no bestmove will follow)
State: A=IDLE, B=IDLE
```

---

## 8. Team Communication (Optional)

### 8.1 Team Message Mode

Controlled by the `TeamMessageMode` engine option:
- `ignore` — Never send or receive team messages
- `consider` — Receive and consider messages but don't send
- `full` — Both send and receive messages

### 8.2 Sending Messages (Engine → GUI)

**Format:** `teammsg <message_type> [params]`

Engine can send coordination messages to its partner. The GUI routes these to the partner engine as `partnermsg`.

### 8.3 Receiving Messages (GUI → Engine)

**Format:** `partnermsg <message_type> [params]`

The GUI forwards a partner's `teammsg` to the engine. Format is identical to `teammsg`.

**Note:** GUI only sends `partnermsg` when:
1. Engine has a partner (not playing both boards)
2. Partner engine sent a `teammsg`
3. Engine's `TeamMessageMode` is not `ignore`

### 8.4 Message Types

#### Need Piece
```
teammsg need <piece> [urgency <low|medium|high>]
```
Request a specific piece from partner.

**Examples:**
```
teammsg need n urgency high
teammsg need q urgency medium
teammsg need b
```

#### Stall
```
teammsg stall [duration <moves>]
```
Request partner to avoid captures temporarily (to deny opponent pieces).

**Examples:**
```
teammsg stall duration 2
teammsg stall
```

#### Play Fast
```
teammsg play_fast [reason <time|pressure>]
```
Request partner to move quickly.

**Examples:**
```
teammsg play_fast reason time
teammsg play_fast reason pressure
```

#### Material Status
```
teammsg material <+/-><value>
```
Report material advantage/disadvantage in centipawns.

**Examples:**
```
teammsg material +350
teammsg material -150
```

#### Threat Level
```
teammsg threat <low|medium|high|critical>
```
Communicate current danger level on this board.

**Examples:**
```
teammsg threat critical
teammsg threat medium
```

---

## 9. Example Sessions

### 9.1 Simple Single-Board Session

```
GUI → Engine: ubi
Engine → GUI: id name SimpleBot 1.0
Engine → GUI: id author Carol
Engine → GUI: option name Hash type spin default 128 min 16 max 1024
Engine → GUI: option name TeamMessageMode type combo default full var ignore var consider var full
Engine → GUI: ubiok

GUI → Engine: setoption name Hash value 256
GUI → Engine: setoption name TeamMessageMode value full
GUI → Engine: ubinewgame
GUI → Engine: isready
Engine → GUI: readyok

# Opening position
GUI → Engine: position startpos | startpos clock 180000 180000 180000 180000
GUI → Engine: go board A

Engine → GUI: info board A depth 1 nodes 20 score cp 15
Engine → GUI: info board A depth 8 nodes 15000 score cp 30
Engine → GUI: info board A depth 10 nodes 45000 time 2950 score cp 35 pv e2e4 e7e5 g1f3
Engine → GUI: bestmove board A e2e4

# After opponent's move — full state update
GUI → Engine: position rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1 | startpos clock 177000 177000 180000 180000

# Partner sends a message
GUI → Engine: partnermsg need n urgency medium

GUI → Engine: go board A
Engine → GUI: info board A depth 12 score cp 20
Engine → GUI: teammsg material +50
Engine → GUI: bestmove board A g1f3

# Later — using reserves
GUI → Engine: position r1bqkb1r/pp1p1ppp/2n2n2/2p1p3/2B1P3/2N2N2/PPPP1PPP/R1BQK2R[NNPp] w KQkq - 0 6 | rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3 clock 165000 168000 170000 172000
GUI → Engine: go board A

Engine → GUI: info board A depth 11 score cp 55 pv n@e5 reserve_value 300
Engine → GUI: teammsg need b urgency high
Engine → GUI: bestmove board A n@e5

GUI → Engine: quit
```

### 9.2 Dual-Board Session

```
GUI → Engine: ubi
Engine → GUI: id name DualMaster 2.0
Engine → GUI: id author Dave
Engine → GUI: option name Hash type spin default 512 min 16 max 8192
Engine → GUI: option name Threads type spin default 2 min 1 max 16
Engine → GUI: ubiok

GUI → Engine: setoption name Hash value 1024
GUI → Engine: setoption name Threads value 4
GUI → Engine: ubinewgame
GUI → Engine: isready
Engine → GUI: readyok

# Opening — both boards at startpos
GUI → Engine: position startpos | startpos clock 180000 180000 180000 180000

# Engine thinks about both boards simultaneously
GUI → Engine: go board A
GUI → Engine: go board B

# Engine coordinates between boards
Engine → GUI: info board A depth 8 score cp 30
Engine → GUI: info board B depth 8 score cp 25
Engine → GUI: bestmove board A e2e4
Engine → GUI: bestmove board B d2d4

# After opponents' moves on both boards
GUI → Engine: position rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 2 | rnbqkb1r/pppp1ppp/5n2/4p3/3P4/8/PPP1PPPP/RNBQKBNR[] w KQkq - 0 2 clock 177000 177000 177000 177000

GUI → Engine: go board A
GUI → Engine: go board B

Engine → GUI: info board A depth 10 score cp 35
Engine → GUI: info board B depth 10 score cp 30
Engine → GUI: bestmove board A g1f3
Engine → GUI: bestmove board B b1c3

# Later — with reserves
GUI → Engine: position r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[Np] w KQkq - 4 5 | rnbqkb1r/ppp2ppp/4pn2/3p4/2PP4/2N5/PP2PPPP/R1BQKBNR[P] b KQkq - 0 4 clock 165000 168000 170000 172000

GUI → Engine: go board A
GUI → Engine: go board B

Engine → GUI: info board A depth 11 reserve_value 300
Engine → GUI: info board B depth 11 reserve_value 100
Engine → GUI: bestmove board A n@e5
Engine → GUI: bestmove board B e6e5

GUI → Engine: quit
```

### 9.3 Benchmarking Session with Metadata

```
# Match-level metadata (set once)
GUI → Engine: metadata match_id bench_20260318_001
GUI → Engine: metadata total_games 100
GUI → Engine: metadata opponent baseline_stable_v1
GUI → Engine: metadata time_control 120000

# Game-level metadata (set before each game)
GUI → Engine: metadata game_number 1
GUI → Engine: metadata position white_A,black_B
GUI → Engine: ubinewgame
GUI → Engine: isready
Engine → GUI: readyok
# ... position + go commands ...
GUI → Engine: metadata game_result win

# Next game
GUI → Engine: metadata game_number 2
GUI → Engine: metadata position black_A,white_B
GUI → Engine: ubinewgame
# ...
```

---

## 10. Implementation Notes

### 10.1 For Engine Developers

#### State Management
- Track positions for both boards (even if only playing one)
- Maintain reserves per board from BFEN
- Use clock information for time management
- Consider partner's position when making decisions (if dual-board)

#### Move Generation
```python
def generate_moves(position, reserves):
    moves = generate_regular_moves(position)

    # Add drop moves for each piece in reserve
    for piece in reserves:
        for square in empty_squares(position):
            if is_legal_drop(piece, square, position):
                moves.append(f"{piece.lower()}@{square}")

    return moves
```

#### Evaluation Considerations
- **Reserve value**: Pieces in hand have value
- **Piece types matter**: Knights and bishops often more valuable than rooks in bughouse
- **Coordination**: Position on one board affects strategy on other
- **Time pressure**: Speed matters more than perfect accuracy
- **King safety**: Drops can create sudden attacks

#### Time Management
```python
def calculate_search_time(clocks, board, side):
    # Engine-driven: derive budget from the full clock state.
    my_time = clocks[board][side]
    opp_time = clocks[board][other(side)]

    if my_time < 3500:                 # emergency — answer instantly
        return 50
    if my_time < 10000:                # low time — play fast
        return min(my_time / 40, 500)
    if my_time > opp_time + 15000:     # time advantage — invest it
        return min(my_time / 20, 4000)
    return min(my_time / 30, 2000)     # normal pace
```

#### Team Messages
```python
def should_send_message(position, reserves, partner_position):
    if reserves.count('n') == 0 and evaluate(position) < -200:
        return "teammsg need n urgency high"

    if material_advantage(position) > 300:
        return f"teammsg material +{material_advantage(position)}"

    if is_under_attack(position):
        return "teammsg threat high"

    return None
```

### 10.2 For GUI Developers

#### Engine Lifecycle
1. Start engine process
2. Send `ubi` and wait for `ubiok`
3. Configure options with `setoption`
4. Send `ubinewgame` to start
5. Use `isready`/`readyok` for synchronization

#### Position Updates
- Always send complete state (both boards, all clocks) in a single `position` command
- Use BFEN format with reserves
- Send after every move on either board
- Include updated clock times

#### Search Control
- Send `go board X` to start search
- Can send for one or both boards
- Track which boards are searching
- After `bestmove board X`, that board is IDLE
- `stop` cancels a search without producing a move — do not wait for `bestmove` after sending it
- Don't send `stop` after `bestmove` (redundant)

#### Move Handling
- Parse regular moves: `e2e4`, `e7e8q`
- Parse drop moves: `n@f3`, `p@e4`
- Validate moves before applying
- Update reserves after captures
- Send position update after move

#### Team Message Routing
```javascript
function routeTeamMessage(fromEngine, message) {
    // Parse message
    const [, type, ...params] = message.split(' ');

    // Find partner engine
    const partnerEngine = getPartnerEngine(fromEngine);

    // Forward as partnermsg if partner exists and accepts messages
    if (partnerEngine && partnerEngine.teamMessageMode !== 'ignore') {
        partnerEngine.send(`partnermsg ${type} ${params.join(' ')}`);
    }
}
```

#### Partner Message Forwarding
When receiving `teammsg` from one engine, forward to partner engine as `partnermsg`:
```javascript
function onEngineTeamMsg(engine, msg) {
    const partner = getPartnerEngine(engine);
    if (partner && partner.teamMessageMode !== 'ignore') {
        // Forward with same format: teammsg → partnermsg
        partner.send(msg.replace('teammsg', 'partnermsg'));
    }
}
```

#### Error Handling
- Set timeout for `isready` (recommended: 5 seconds)
- Handle invalid moves from engine
- Restart engine if it crashes
- Validate all engine output

### 10.3 Differences from UCI

| Aspect | UCI | UBI |
|--------|-----|-----|
| **Position** | `position [startpos\|fen <fen>] [moves ...]` | `position <bfen_a> \| <bfen_b> clock <wA> <bA> <wB> <bB>` |
| **Go** | `go [searchparams]` | `go board <A\|B> [searchparams]` — params optional, clocks authoritative |
| **Stop** | `stop` → engine sends `bestmove` | `stop [board <A\|B>]` → search cancelled, no `bestmove` |
| **Bestmove** | `bestmove <move> [ponder <move>]` | `bestmove board <A\|B> <move>` |
| **Info** | `info [key value]*` | `info board <A\|B> [key value]*` |
| **State** | Single global (IDLE/SEARCHING) | Per-board (A and B independent) |
| **Team play** | N/A | `teammsg` / `partnermsg` |
| **Metadata** | N/A | `metadata <key> <value>` |

---

## 11. FAQ

**Q: Do I need to implement both single-board and dual-board modes?**
A: No. Start with single-board. Dual-board is an optimization where one engine instance handles both boards.

**Q: How do I know which color I'm playing?**
A: Check the BFEN's active color field. The position tells you whose turn it is.

**Q: What if I receive `go` for both boards simultaneously?**
A: You're a dual-board engine. Coordinate your thinking and send two `bestmove` commands.

**Q: Why doesn't the GUI tell me how long to think?**
A: It can (via optional `go` hints), but it doesn't have to. Time management is a core engine skill in bughouse, so UBI delivers the full four-clock state with every `position` and lets the engine decide. See Section 2.4.

**Q: Can drops give check?**
A: Yes! Drops are regular moves and can check or attack the king.

**Q: Can I drop to give checkmate?**
A: Yes! Drop checkmate is legal in UBI. A piece drop that delivers checkmate wins the game.

**Q: How do I handle promoted pieces?**
A: Parse `~` suffix from BFEN. When capturing `Q~`, add `P` (not `Q`) to partner's reserve.

**Q: Should I respond to `partnermsg` if `TeamMessageMode` is `ignore`?**
A: No. The GUI shouldn't send `partnermsg` in that mode, but if it does, ignore it.

**Q: How do I test my engine without a full GUI?**
A: Use command-line input. Type commands and verify responses. Many UCI GUIs can be adapted.

**Q: What if there are no legal moves when I receive `go`?**
A: Send `bestmove board <X> (none)`. The game should already be ending via checkmate/stalemate detection on the GUI side.

---

## Appendix A: Quick Reference

### Commands: GUI to Engine

```
ubi
isready
setoption name <id> [value <x>]
ubinewgame
position <bfen_a> | <bfen_b> clock <wA> <bA> <wB> <bB>
position startpos | startpos clock <wA> <bA> <wB> <bB>
partnermsg <type> [params]
go board <A|B> [movetime <ms>] [depth <n>] [nodes <n>] [infinite]
stop [board <A|B>]
metadata <key> <value>
quit
```

### Commands: Engine to GUI

```
id name <name>
id author <author>
option name <id> type <type> [default <x>] [min <x>] [max <x>] [var <x>]*
ubiok
readyok
info board <A|B> [depth <n>] [nodes <n>] [time <ms>] [score cp <n>] [score mate <n>] [pv <moves>]
bestmove board <A|B> <move>
bestmove board <A|B> (none)
teammsg <type> [params]
```

### Move Notation

```
Regular: e2e4, g1f3, e7e8q, e1g1
Drops:   n@f3, p@e4, q@d5, b@c4
```

### State Transitions

```
IDLE → SEARCHING: go board X
SEARCHING → IDLE: bestmove board X (automatic)
SEARCHING → IDLE: stop [board X] (manual cancel, no bestmove)
SEARCHING → IDLE: internal abort on position invalidation (no bestmove)
```

---

## Version History

- **v1.1** (2026-07-25) — Added `metadata` command (§4.9); `stop` no longer elicits `bestmove`, aligning with the internal-abort model in §7.3; `go` search parameters downgraded to optional hints — engine-driven time management from the four-clock state is authoritative (§2.4, §4.7); `setoption` permitted mid-game for runtime tuning; documented pondering model (§7.4); noted lowercase drop-notation divergence from crazyhouse UCI (§6.2)
- **v1.0** (2025-02-21) — Atomic `position` command (both boards + clocks), formal search state model, removed separate `clock` command and `moves` list, added `bestmove (none)`, removed `ponder`
- **v0.2** (2025-02-21) — Drop checkmate legality, BFEN 2.0 alignment
- **v0.1** (2025-02-04) — Initial specification

---

## References

- [UCI Protocol Specification](http://wbec-ridderkerk.nl/html/UCIProtocol.html)
- [BFEN 2.0 Specification](./BFEN.md) (single-board and two-board position notation)
- [Bughouse Chess Rules](https://en.wikipedia.org/wiki/Bughouse_chess)
- [Fairy-Stockfish](https://fairy-stockfish.github.io/)
- [BPGN Specification](http://bughousedb.com/Lieven_BPGN_Standard.txt)
