# BFEN (Bughouse FEN) Specification v0.1

## Overview

BFEN (Bughouse FEN) extends the standard FEN (Forsyth-Edwards Notation) format to represent bughouse chess positions. It follows the [fairy-stockfish chess variant standards](https://fairy-stockfish.github.io/chess-variant-standards/fen.html) for consistency and interoperability.

**Key additions to standard FEN:**
- Piece reserves (captured pieces available for dropping)
- Promoted piece tracking (for proper demotion on recapture)

---

## Format Structure

```
<position>[<reserves>] <color> <castling> <ep> <halfmove> <fullmove>
```

### Field Breakdown

1. **Position** - Standard FEN board representation (ranks separated by `/`)
2. **Reserves** - Pieces available for dropping, in brackets `[]`
3. **Active Color** - `w` for white, `b` for black
4. **Castling Rights** - `KQkq` or any subset, `-` if none
5. **En Passant** - Target square or `-` if none
6. **Halfmove Clock** - Moves since last capture or pawn move
7. **Fullmove Number** - Starts at 1, increments after black's move

---

## Reserves Field Specification

### Format
`[<pieces>]` immediately after position string, no space

### Piece Notation
- **Uppercase** (QRBNP) = White's reserve on this board
- **Lowercase** (qrbnp) = Black's reserve on this board
- **Empty reserves** = `[]`
- **No separators** between pieces

### Important Notes
- Each board tracks **only its own reserves**
- Reserves are pieces captured on one board that are sent to the partner on the other board
- The GUI handles routing captures between boards

### Canonical Ordering (Recommended)
For consistency and comparison, order reserves as: `QRBNPqrbnp`
- All white pieces before black pieces
- Within each color: descending value (Queen, Rook, Bishop, Knight, Pawn)

**Parser requirement:** Engines MUST accept reserves in any order but SHOULD output in canonical order.

---

## Promoted Pieces

### Notation
Use `~` suffix to mark promoted pieces on the board:
- `Q~`, `R~`, `B~`, `N~` - White promoted pieces
- `q~`, `r~`, `b~`, `n~` - Black promoted pieces

### Demotion Rule
When a promoted piece is captured, it **demotes to a pawn** before going to the partner's reserve.

**Example:**
- White promotes pawn to queen → `Q~` on board
- Black captures `Q~` → Partner receives `P` (not `Q`) in reserve

### Why Track Promotions?
Bughouse rules require captured promoted pieces to revert to pawns. Without tracking, game integrity is violated.

---

## Examples

### Starting Position
```
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
```

### After Some Play - Board A
White has 2 knights and 1 pawn; black has 1 pawn in reserve:
```
r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5
```

### Different Reserves - Board B
White has queen; black has bishop and 2 pawns:
```
rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3
```

### With Promoted Piece
Promoted bishop on e3 (originally a pawn):
```
r2q1rk1/ppp2ppp/2n1bn2/3p4/3P4/2NB~/5N2/R1BQ1RK1[QNNPrbp] b - - 0 12
```

### Complex Reserves
Many pieces in reserve:
```
r1bqk2r/pppp1ppp/2n2n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQ1RK1[QQRNNNPPPPrbbnnppp] w kq - 8 9
```

### Empty Reserves
```
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
```

### Only White Pieces in Reserve
```
r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNP] w KQkq - 4 5
```

### Only Black Pieces in Reserve
```
rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[bpp] b KQkq - 0 3
```

---

## Implementation Guide

### Parsing BFEN

**Python Example:**
```python
def parse_bfen(bfen_str):
    # Split position and rest
    pos_end = bfen_str.index('[')
    position = bfen_str[:pos_end]
    rest = bfen_str[pos_end:]
    
    # Extract reserves
    reserve_end = rest.index(']') + 1
    reserves_str = rest[1:reserve_end-1]  # Strip brackets
    
    # Parse reserves
    white_reserve = [c for c in reserves_str if c.isupper()]
    black_reserve = [c for c in reserves_str if c.islower()]
    
    # Parse remaining fields
    fields = rest[reserve_end:].strip().split()
    active_color = fields[0]
    castling = fields[1]
    en_passant = fields[2]
    halfmove = int(fields[3])
    fullmove = int(fields[4])
    
    return {
        'position': position,
        'white_reserve': white_reserve,
        'black_reserve': black_reserve,
        'active_color': active_color,
        'castling': castling,
        'en_passant': en_passant,
        'halfmove': halfmove,
        'fullmove': fullmove
    }
```

**JavaScript Example:**
```javascript
function parseBFEN(bfen) {
    const posEnd = bfen.indexOf('[');
    const position = bfen.substring(0, posEnd);
    const rest = bfen.substring(posEnd);
    
    const reserveEnd = rest.indexOf(']') + 1;
    const reservesStr = rest.substring(1, reserveEnd - 1);
    
    const whiteReserve = reservesStr.split('').filter(c => c === c.toUpperCase() && c !== c.toLowerCase());
    const blackReserve = reservesStr.split('').filter(c => c === c.toLowerCase() && c !== c.toUpperCase());
    
    const fields = rest.substring(reserveEnd).trim().split(' ');
    
    return {
        position: position,
        whiteReserve: whiteReserve,
        blackReserve: blackReserve,
        activeColor: fields[0],
        castling: fields[1],
        enPassant: fields[2],
        halfmove: parseInt(fields[3]),
        fullmove: parseInt(fields[4])
    };
}
```

### Generating BFEN

**Python Example:**
```python
def generate_bfen(position, white_reserve, black_reserve, color, castling, ep, half, full):
    # Canonical ordering
    piece_order = {'Q': 0, 'R': 1, 'B': 2, 'N': 3, 'P': 4}
    white_pieces = sorted(white_reserve, key=lambda p: piece_order.get(p, 5))
    black_pieces = sorted([p.lower() for p in black_reserve], key=lambda p: piece_order.get(p.upper(), 5))
    
    reserves = ''.join(white_pieces) + ''.join(black_pieces)
    
    return f"{position}[{reserves}] {color} {castling} {ep} {half} {full}"
```

**JavaScript Example:**
```javascript
function generateBFEN(position, whiteReserve, blackReserve, color, castling, ep, half, full) {
    const pieceOrder = { 'Q': 0, 'R': 1, 'B': 2, 'N': 3, 'P': 4 };
    
    const whitePieces = whiteReserve.sort((a, b) => 
        (pieceOrder[a] || 5) - (pieceOrder[b] || 5)
    ).join('');
    
    const blackPieces = blackReserve.sort((a, b) => 
        (pieceOrder[a.toUpperCase()] || 5) - (pieceOrder[b.toUpperCase()] || 5)
    ).join('');
    
    const reserves = whitePieces + blackPieces;
    
    return `${position}[${reserves}] ${color} ${castling} ${ep} ${half} ${full}`;
}
```

### Handling Promoted Pieces

**Checking if promoted:**
```python
def is_promoted(piece_str):
    return piece_str.endswith('~')
```

**Demoting on capture:**
```python
def demote_if_promoted(piece):
    if piece.endswith('~'):
        return 'P' if piece[0].isupper() else 'p'
    return piece
```

### Reserve Management

**Adding to reserve:**
```python
def add_to_reserve(reserve_list, piece):
    # Demote if promoted
    piece = demote_if_promoted(piece)
    reserve_list.append(piece)
    return reserve_list
```

**Removing from reserve (for drops):**
```python
def remove_from_reserve(reserve_list, piece_type):
    if piece_type.upper() in [p.upper() for p in reserve_list]:
        # Find and remove first occurrence
        for i, p in enumerate(reserve_list):
            if p.upper() == piece_type.upper():
                return reserve_list[:i] + reserve_list[i+1:]
    return reserve_list
```

---

## Validation Rules

### Reserve Validation
- Only valid pieces: Q, R, B, N, P (and lowercase)
- No kings in reserves
- Total pieces + reserves ≤ 32 per color (accounting for promotions)

### Promoted Piece Validation
- Only Q~, R~, B~, N~ are valid promoted pieces (no promoted pawns or kings)
- Promoted pieces must make sense given pawn structure and captures

### Position Validation
- Standard FEN validation rules apply
- Exactly one king per color
- Pawns not on 1st or 8th rank (unless part of FEN position string, not reserve)

---

## Comparison with Standard FEN

| Aspect | Standard FEN | BFEN |
|--------|--------------|------|
| Position | ✓ Same | ✓ Same |
| Reserves | ✗ None | ✓ In brackets after position |
| Promoted pieces | ✗ Not tracked | ✓ Marked with `~` |
| Fields | 6 fields | 6 fields (reserves integrated) |
| Compatibility | N/A | Can parse standard FEN as `position[]` |

---

## Benefits

1. **Standards-compliant** - Follows fairy-stockfish variant FEN format
2. **Self-contained** - Complete position state in single string
3. **Human-readable** - Easy to understand and debug
4. **Copy-paste friendly** - Share positions via text
5. **Interoperable** - Minor modifications to existing FEN parsers
6. **Hashable** - Position + reserves = unique state for transposition tables

---

## Edge Cases and Special Situations

### No Reserves Available
```
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
```

### Maximum Reserves (Theoretical)
If many pieces captured and not yet used:
```
r7/8/8/8/8/8/8/R6K[QQQQQQQRRRRRBBBBNNNNPPPPPPPPqqqqqqqrrrrrbbbbnnnnnpppppppp] w - - 0 50
```

### Asymmetric Reserves
One side has many pieces, other has none:
```
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[QRBNP] w KQkq - 0 1
```

### All Promoted Pieces
Multiple promotions have occurred:
```
Q~Q~Q~/Q~Q~Q~/Q~Q~Q~/8/8/q~q~q~/q~q~q~/q~q~q~[] w - - 0 100
```

---

## Future Considerations

- **Time metadata:** Currently separate from BFEN (by design)
- **Board linkage:** Mechanism to represent both boards in single notation
- **Move history:** Integration with PGN-style notation
- **Variant rules:** Extensions for different bughouse rule sets

---

## References

- [Fairy-Stockfish Chess Variant Standards](https://fairy-stockfish.github.io/chess-variant-standards/fen.html)
- [Standard FEN Specification](https://en.wikipedia.org/wiki/Forsyth%E2%80%93Edwards_Notation)
- [Bughouse Chess Rules](https://en.wikipedia.org/wiki/Bughouse_chess)

---

## Version History

- **v0.1** (2025-02-04) - Initial specification
