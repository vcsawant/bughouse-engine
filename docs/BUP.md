# BUP (Bughouse Universal Protocol) v0.1

## Overview

BUP (Bughouse Universal Protocol) is a text-based communication protocol for bughouse chess engines. It extends concepts from UCI (Universal Chess Interface) while addressing bughouse-specific requirements: dual boards, piece reserves, drop moves, and optional team coordination.

**Design Philosophy:**
- Position-agnostic engine design
- Text-based, human-readable communication
- Single protocol supporting both single and dual-board play
- Optional teammate coordination
- Engine doesn't need to know its role, only positions to analyze

---

## Design Principles

### Position-Agnostic Architecture

**The engine doesn't need to know:**
- Which team it's on
- Which color it's playing
- Whether it has a partner
- Whether it's a single or dual board game

**The engine only knows:**
- Current positions (board A and/or B)
- Piece reserves for all players
- Clock times for all players
- When to analyze positions

**The GUI handles:**
- Whose turn it is
- Which engine to query
- Routing team messages between partners
- Game state and flow management

This design makes engines simpler to implement and easier to test in isolation.

---

## Communication Model

- **Transport:** stdin/stdout
- **Format:** Line-based text commands
- **Encoding:** UTF-8
- **Flow:** Asynchronous - engine can send info while thinking
- **Termination:** Newline character (`\n`)

---

## 1. Engine Initialization

### Starting the Engine

**GUI → Engine:** `bup`

Initiates BUP mode. Engine must respond with identification and capabilities.

**Engine → GUI Response:**
```
id name <string>
id author <string>
[option <definition>]*
bupok
```

**Example:**
```
GUI → Engine: bup

Engine → GUI: id name BughouseBot 1.0
Engine → GUI: id author Alice Smith
Engine → GUI: option name Hash type spin default 128 min 16 max 4096
Engine → GUI: option name Threads type spin default 1 min 1 max 64
Engine → GUI: option name TeamMessageMode type combo default consider var ignore var consider var full
Engine → GUI: bupok
```

### Options

Engines can define custom options following UCI conventions:

**Option Types:**
- `spin` - Integer with min/max (e.g., `Hash`, `Threads`)
- `combo` - Selection from predefined values (e.g., `TeamMessageMode`)
- `check` - Boolean flag
- `string` - Text input
- `button` - Trigger action

**Standard Options:**
- `Hash` - Hash table size in MB
- `Threads` - Number of search threads
- `TeamMessageMode` - Team communication mode (see section 7)

---

## 2. Setting Options

**GUI → Engine:** `setoption name <id> [value <x>]`

Set engine parameters. Must be sent before `bupnewgame`.

**Examples:**
```
setoption name Hash value 256
setoption name Threads value 4
setoption name TeamMessageMode value full
```

---

## 3. Position Setup

### Board Position

**GUI → Engine:** `position board <A|B> <startpos|bfen <bfenstring>> [moves <move1> ... <moveN>]`

Sets position for a specific board using BFEN notation (see BFEN.md).

**Parameters:**
- `board` - Either `A` or `B`
- `startpos` - Standard starting position (equivalent to `bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1`)
- `bfen <string>` - Position in BFEN format (includes reserves)
- `moves` - Optional move list in algebraic notation

**Examples:**
```
position board A startpos
position board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
position board A bfen r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5 moves g1f3
position board B bfen rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3
```

### Clock Times

**GUI → Engine:** `clock <color_board> <milliseconds>`

Set time remaining for each player. All four clocks should be provided before `go` command.

**Format:** `color_board` is `white_A`, `black_A`, `white_B`, or `black_B`

**Example:**
```
clock white_A 180000
clock black_A 175000
clock white_B 182000
clock black_B 178000
```

---

## 4. Move Notation

### Regular Moves

Standard UCI long algebraic notation:
- `e2e4` - Pawn from e2 to e4
- `g1f3` - Knight from g1 to f3
- `e1g1` - Castling kingside (O-O)
- `e1c1` - Castling queenside (O-O-O)
- `e7e8q` - Pawn promotion to queen
- `e7e8r` - Pawn promotion to rook
- `e7e8b` - Pawn promotion to bishop
- `e7e8n` - Pawn promotion to knight

### Drop Moves

**Format:** `<piece>@<square>` (lowercase piece letter)

**Examples:**
- `p@e4` - Drop pawn at e4
- `n@f3` - Drop knight at f3
- `b@c4` - Drop bishop at c4
- `r@a1` - Drop rook at a1
- `q@d5` - Drop queen at d5

**Legality Requirements:**
1. Piece must exist in player's reserve
2. Target square must be empty
3. Cannot drop pawn on 1st or 8th rank
4. Cannot drop to give immediate checkmate (rule varies by tournament)

---

## 5. Search Commands

### Basic Search

**GUI → Engine:** `go board <A|B> [searchparams]`

Start calculating best move for specified board.

**Search Parameters:**
- `movetime <ms>` - Search for exactly this many milliseconds
- `wtime <ms>` - White's remaining time on this board
- `btime <ms>` - Black's remaining time on this board
- `winc <ms>` - White's increment per move
- `binc <ms>` - Black's increment per move
- `depth <n>` - Search to depth n plies
- `nodes <n>` - Search up to n nodes
- `infinite` - Search until `stop` command

**Examples:**
```
go board A movetime 5000
go board B wtime 180000 btime 175000 winc 0 binc 0
go board A depth 12
go board B infinite
```

### Dual-Board Coordination

When a single engine is playing both boards, GUI can send:
```
go board A movetime 3000
go board B movetime 3000
```

The engine can coordinate its thinking across both boards to optimize team strategy.

### Stop Search

**GUI → Engine:** `stop [board <A|B>]`

Stop calculating immediately.

**Behavior:**
- `stop` - Stop all active searches
- `stop board A` - Stop search on board A only

**Example:**
```
stop board A
stop
```

### Updates During Search

Commands can be sent to engines during active search with the following behavior:

**Non-blocking updates (can be sent anytime):**
- `clock` commands - Engine should read and update time management on next opportunity
- `position` commands for boards **not currently being analyzed** - Context updates that engine can incorporate when ready

**Blocking updates (require `stop` first):**
- `position` commands with **reserve changes** on any board - Invalidates evaluation, must stop all affected engines
- `position` commands for the board **currently being analyzed** - Position is now stale, must stop and restart

**Expected GUI Behavior:**

When a move occurs on a board:

1. **Quiet move (no capture):**
   - Send `stop` only to engines analyzing that specific board
   - Update `position` for all engines (those not analyzing this board receive it as context)
   - Update `clock` for all engines (non-blocking)
   - Send `go` to restart engines that were stopped

2. **Capture (reserves change):**
   - Send `stop` to all engines (reserves affect both boards)
   - Update `position` for both boards with new reserves
   - Update `clock` for all engines
   - Send `go` to restart all engines whose turn it is

This ensures engines always have accurate information while minimizing search interruptions.

---

## 6. Engine Output

### Search Information

**Engine → GUI:** `info board <A|B> [infotype <value>]*`

Provides search progress information for a specific board.

**Standard Info Types:**
- `depth <n>` - Current search depth in plies
- `seldepth <n>` - Selective search depth (max depth reached)
- `nodes <n>` - Nodes searched
- `nps <n>` - Nodes per second
- `time <ms>` - Time spent searching in milliseconds
- `score cp <n>` - Evaluation in centipawns (positive = good for side to move)
- `score mate <n>` - Mate in n moves (positive = we mate, negative = we're mated)
- `pv <move1> ... <moveN>` - Principal variation (best line)

**Bughouse-Specific Info Types:**
- `reserve_value <n>` - Estimated value of our reserves in centipawns
- `partner_reserve_value <n>` - Estimated value of partner's reserves

**Examples:**
```
info board A depth 12 nodes 150000 nps 75000 time 2000 score cp 45 pv e2e4 e7e5 g1f3
info board B depth 10 score cp -20 reserve_value 300
info board A depth 15 score mate 5 pv n@e5 f6e4 q@h5 g7g6 h5e5
```

### Best Move

**Engine → GUI:** `bestmove board <A|B> <move> [ponder <move>]`

Reports the chosen move for a specific board.

**Parameters:**
- `board` - Board identifier (A or B)
- `move` - Move in algebraic notation (regular or drop)
- `ponder` - Optional move engine expects opponent to play

**Examples:**
```
bestmove board A e2e4
bestmove board A n@f3 ponder e7e5
bestmove board B d2d4 ponder d7d5
bestmove board A p@e5
```

For dual-board engines thinking about both boards, send separate `bestmove` lines:
```
bestmove board A e2e4
bestmove board B d2d4
```

---

## 7. Team Communication (Optional)

### Team Message Mode

Controlled by `TeamMessageMode` option:
- `ignore` - Never send/receive team messages
- `consider` - Receive and consider messages but don't send
- `full` - Both send and receive messages

### Sending Messages

**Engine → GUI:** `teammsg <message_type> [params]`

Engine can send coordination messages to partner.

**Message Types:**

#### Need Piece
```
teammsg need <piece> [urgency <low|medium|high>]
```
Request specific piece from partner.

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

### Receiving Messages

**GUI → Engine:** `partnermsg <message_type> [params]`

GUI forwards partner's message to engine. Format identical to `teammsg`.

**Examples:**
```
partnermsg need b urgency medium
partnermsg threat high
partnermsg stall duration 1
partnermsg material +200
partnermsg play_fast reason time
```

**Note:** GUI only sends `partnermsg` when:
1. Engine has a partner (not playing both boards)
2. Partner engine sent a `teammsg`
3. Engine's `TeamMessageMode` is not `ignore`

---

## 8. Game Management

### New Game

**GUI → Engine:** `bupnewgame`

Prepare for new game. Engine should:
- Clear all hash tables
- Reset state
- Clear any cached data

**Example:**
```
bupnewgame
```

### Ready Check

**GUI → Engine:** `isready`

Synchronization point. Engine must finish processing all previous commands and respond.

**Engine → GUI:** `readyok`

**Example:**
```
GUI → Engine: isready
Engine → GUI: readyok
```

### Quit

**GUI → Engine:** `quit`

Terminate engine gracefully.

**Example:**
```
quit
```

---

## 9. Complete Example Sessions

### Example 1: Single Board Engine

```
GUI → Engine: bup
Engine → GUI: id name SimpleBot 1.0
Engine → GUI: id author Carol
Engine → GUI: option name Hash type spin default 128 min 16 max 1024
Engine → GUI: option name TeamMessageMode type combo default full var ignore var consider var full
Engine → GUI: bupok

GUI → Engine: setoption name Hash value 256
GUI → Engine: setoption name TeamMessageMode value full
GUI → Engine: bupnewgame
GUI → Engine: isready
Engine → GUI: readyok

# Opening position
GUI → Engine: position board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
GUI → Engine: clock white_A 180000
GUI → Engine: clock black_A 180000
GUI → Engine: clock white_B 180000
GUI → Engine: clock black_B 180000

GUI → Engine: go board A movetime 3000
Engine → GUI: info board A depth 1 nodes 20 score cp 15
Engine → GUI: info board A depth 8 nodes 15000 score cp 30
Engine → GUI: info board A depth 10 nodes 45000 score cp 35 pv e2e4 e7e5 g1f3
Engine → GUI: bestmove board A e2e4

# After opponent's move
GUI → Engine: position board A bfen rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR[] b KQkq - 0 1
GUI → Engine: clock white_A 177000
GUI → Engine: clock black_A 177000

# Partner sends message
GUI → Engine: partnermsg need n urgency medium

GUI → Engine: go board A movetime 3000
Engine → GUI: info board A depth 12 score cp 20
Engine → GUI: teammsg material +50
Engine → GUI: bestmove board A g1f3

# Later - using reserves
GUI → Engine: position board A bfen r1bqkb1r/pp1p1ppp/2n2n2/2p1p3/2B1P3/2N2N2/PPPP1PPP/R1BQK2R[NNPp] w KQkq - 0 6
GUI → Engine: clock white_A 165000
GUI → Engine: clock black_A 168000

GUI → Engine: go board A movetime 2500
Engine → GUI: info board A depth 11 score cp 55 pv n@e5 reserve_value 300
Engine → GUI: teammsg need b urgency high
Engine → GUI: bestmove board A n@e5

GUI → Engine: quit
```

### Example 2: Dual-Board Engine

```
GUI → Engine: bup
Engine → GUI: id name DualMaster 2.0
Engine → GUI: id author Dave
Engine → GUI: option name Hash type spin default 512 min 16 max 8192
Engine → GUI: option name Threads type spin default 2 min 1 max 16
Engine → GUI: bupok

GUI → Engine: setoption name Hash value 1024
GUI → Engine: setoption name Threads value 4
GUI → Engine: bupnewgame
GUI → Engine: isready
Engine → GUI: readyok

# Setup both boards
GUI → Engine: position board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
GUI → Engine: position board B bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
GUI → Engine: clock white_A 180000
GUI → Engine: clock black_A 180000
GUI → Engine: clock white_B 180000
GUI → Engine: clock black_B 180000

# Engine thinks about both boards simultaneously
GUI → Engine: go board A movetime 3000
GUI → Engine: go board B movetime 3000

# Engine coordinates between boards
Engine → GUI: info board A depth 8 score cp 30
Engine → GUI: info board B depth 8 score cp 25
Engine → GUI: bestmove board A e2e4
Engine → GUI: bestmove board B d2d4

# After opponents' moves on both boards
GUI → Engine: position board A bfen rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR[] w KQkq - 0 2
GUI → Engine: position board B bfen rnbqkb1r/pppp1ppp/5n2/4p3/3P4/8/PPP1PPPP/RNBQKBNR[] w KQkq - 0 2
GUI → Engine: clock white_A 177000
GUI → Engine: clock black_A 177000
GUI → Engine: clock white_B 177000
GUI → Engine: clock black_B 177000

GUI → Engine: go board A movetime 3000
GUI → Engine: go board B movetime 3000

Engine → GUI: info board A depth 10 score cp 35
Engine → GUI: info board B depth 10 score cp 30
Engine → GUI: bestmove board A g1f3
Engine → GUI: bestmove board B b1c3

# Later - with reserves
GUI → Engine: position board A bfen r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[Np] w KQkq - 4 5
GUI → Engine: position board B bfen rnbqkb1r/ppp2ppp/4pn2/3p4/2PP4/2N5/PP2PPPP/R1BQKBNR[P] b KQkq - 0 4
GUI → Engine: clock white_A 165000
GUI → Engine: clock black_A 168000
GUI → Engine: clock white_B 170000
GUI → Engine: clock black_B 172000

GUI → Engine: go board A movetime 2500
GUI → Engine: go board B movetime 2500

Engine → GUI: info board A depth 11 reserve_value 300
Engine → GUI: info board B depth 11 reserve_value 100
Engine → GUI: bestmove board A n@e5
Engine → GUI: bestmove board B e6e5

GUI → Engine: quit
```

---

## 10. Implementation Notes

### For Engine Developers

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
def calculate_search_time(my_time, opp_time, increment, move_number):
    # Consider both board clocks if dual-board
    # Account for faster pace of bughouse
    # Reserve time for critical moments
    base_time = my_time / 40  # Rough heuristic
    return min(base_time, my_time * 0.05)  # Don't use more than 5% of remaining time
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

### For GUI Developers

#### Position Routing
```javascript
function sendPositionToEngine(engine, board, position, reserves, clocks) {
    // Convert game state to BFEN
    const bfen = generateBFEN(position, reserves);
    
    // Send to engine
    engine.send(`position board ${board} bfen ${bfen}`);
    engine.send(`clock white_${board} ${clocks.white}`);
    engine.send(`clock black_${board} ${clocks.black}`);
    
    // Send partner board info if dual-board engine
    if (engine.isDualBoard) {
        const partnerBoard = (board === 'A') ? 'B' : 'A';
        const partnerBFEN = generateBFEN(partnerPosition, partnerReserves);
        engine.send(`position board ${partnerBoard} bfen ${partnerBFEN}`);
        engine.send(`clock white_${partnerBoard} ${partnerClocks.white}`);
        engine.send(`clock black_${partnerBoard} ${partnerClocks.black}`);
    }
}
```

#### Message Routing
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

#### Capture Handling
```javascript
function onCapture(piece, board, capturer) {
    // Demote promoted pieces
    if (piece.endsWith('~')) {
        piece = capturer === 'white' ? 'P' : 'p';
    }
    
    // Route to partner's board
    const partnerBoard = (board === 'A') ? 'B' : 'A';
    const partnerColor = (capturer === 'white') ? 'white' : 'black';
    
    // Add to partner's reserve
    reserves[partnerBoard][partnerColor].push(piece);
    
    // Update positions for all engines
    updateAllEngines();
}
```

---

## 11. Error Handling

### Invalid Commands
Engines should ignore invalid commands and continue processing. Optionally, engines can log errors for debugging.

### Position Errors
If position is invalid (impossible material, illegal king positions, etc.), engine should:
1. Send `info string Error: Invalid position`
2. Wait for valid position or `quit` command

### Time Trouble
If search is interrupted before completion:
```
bestmove board A e2e4
```
Return best move found so far.

---

## 12. Extension Points for Future Versions

- **Opening books** - Bughouse-specific opening theory
- **Endgame tablebases** - For specific reserve configurations
- **Opponent modeling** - Track opponent tendencies
- **Team profiles** - Aggressive, defensive, balanced coordination styles
- **Multi-variant support** - Different time controls, drop rules
- **Analysis mode** - Deep position analysis with multiple lines
- **Training mode** - Engine explains its reasoning

---

## 13. Differences from UCI

| Feature | UCI | BUP |
|---------|-----|-----|
| Board count | 1 | 2 (A and B) |
| Notation | FEN | BFEN (with reserves) |
| Move types | Regular only | Regular + drops |
| Team play | N/A | Optional messages |
| Position context | Agnostic | Agnostic |
| Hash tables | Position only | Position + reserves |
| Time management | Per game | Per board (4 clocks) |

---

## 14. Testing Your Engine

### Basic Tests
1. Parse `startpos` correctly
2. Generate legal moves (including drops)
3. Respond to `isready` with `readyok`
4. Return `bestmove` within time limit
5. Handle both boards independently

### Advanced Tests
1. Coordinate strategy across boards (dual-board)
2. Evaluate reserve values correctly
3. Send appropriate team messages
4. Handle promoted piece demotions
5. Manage time under pressure

### Test Positions
```
# Empty reserves
position board A bfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1

# Rich reserves
position board A bfen r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[QRBNNPPpp] w KQkq - 4 5

# Promoted pieces
position board A bfen r2q1rk1/ppp2ppp/2n1bn2/3p4/3P4/2NB~/5N2/R1BQ1RK1[QNNPrbp] b - - 0 12

# Critical position
position board A bfen r1bq1rk1/ppp2ppp/2n5/3p4/1bPP4/2N1PN2/PP3PPP/R1BQKB1R[NNPPp] w KQ - 0 10
```

---

## 15. FAQ

**Q: Do I need to implement both single-board and dual-board modes?**  
A: No. Start with single-board. Dual-board is an optimization where one engine instance handles both boards.

**Q: How do I know which color I'm playing?**  
A: Check the BFEN's active color field. The position tells you whose turn it is.

**Q: What if I receive `go` for both boards simultaneously?**  
A: You're a dual-board engine. Coordinate your thinking and send two `bestmove` commands.

**Q: Should I respond to `partnermsg` if `TeamMessageMode` is `ignore`?**  
A: No. The GUI shouldn't send `partnermsg` in that mode, but if it does, ignore it.

**Q: How do I test my engine without a full GUI?**  
A: Use command-line input. Type commands and verify responses. Many UCI GUIs can be adapted.

**Q: Can drops give check?**  
A: Yes! Drops are regular moves and can check or attack the king.

**Q: Can I drop to give checkmate?**  
A: Depends on tournament rules. Some ban "drop mate," others allow it. Implement both modes.

**Q: How do I handle promoted pieces?**  
A: Parse `~` suffix from BFEN. When capturing `Q~`, add `P` (not `Q`) to partner's reserve.

---

## 16. Quick Reference

### Essential Commands
```
# Initialization
bup
bupnewgame
isready / readyok
quit

# Position
position board <A|B> bfen <string> [moves ...]
clock <color_board> <ms>

# Search
go board <A|B> [params]
stop [board <A|B>]

# Output
info board <A|B> <key> <value> ...
bestmove board <A|B> <move>

# Team (optional)
teammsg <type> [params]
partnermsg <type> [params]
```

### Move Notation
```
# Regular
e2e4, g1f3, e1g1, e7e8q

# Drops
p@e4, n@f3, b@c4, r@a1, q@d5
```

---

## Version History

- **v0.1** (2025-02-04) - Initial specification

---

## References

- [UCI Protocol Specification](http://wbec-ridderkerk.nl/html/UCIProtocol.html)
- [BFEN Specification](./BFEN.md)
- [Bughouse Chess Rules](https://en.wikipedia.org/wiki/Bughouse_chess)
- [Fairy-Stockfish](https://fairy-stockfish.github.io/)
