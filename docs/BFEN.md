# BFEN (Bughouse FEN) Specification v2.0

## Overview

BFEN (Bughouse FEN) extends the standard FEN (Forsyth-Edwards Notation) format to represent bughouse chess positions. It follows the [fairy-stockfish chess variant standards](https://fairy-stockfish.github.io/chess-variant-standards/fen.html) for consistency and interoperability.

**Key additions to standard FEN:**
- Piece reserves (captured pieces available for dropping)
- Promoted piece tracking (for proper demotion on recapture)

---

## Relationship to BPGN BFEN

BFEN builds on the original **BPGN BFEN** notation (circa 2002), which was the first standardized bughouse position format. BPGN BFEN appended reserves as a pseudo-9th rank using slash notation (`position/reserves`), making reserves syntactically identical to another rank in the FEN string.

BFEN 2.0 modernizes this approach with **bracket notation** (`position[reserves]`), aligning with [fairy-stockfish](https://fairy-stockfish.github.io/) and the crazyhouse community. This eliminates the parsing ambiguity inherent in slash-based reserves while preserving the same information.

### Comparison

| Feature | BPGN BFEN (legacy) | BFEN 2.0 |
|---------|-------------------|----------|
| Reserves | Slash `/reserves` (pseudo-9th rank) | Brackets `[reserves]` |
| Two-board | `board_a \| board_b` | `board_a \| board_b` (adopted) |
| Promoted pieces | `~` suffix (inconsistent usage) | `~` suffix (always tracked) |
| Standard alignment | Custom | fairy-stockfish compliant |
| Parsing | Ambiguous (is last rank reserves or position?) | Unambiguous (brackets delimit reserves) |

### Example

The same position in both formats:

**BPGN BFEN (legacy):**
```
r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R/NNPp w KQkq - 4 5
```

**BFEN 2.0:**
```
r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5
```

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

## Two-Board Representation

Bughouse involves two simultaneous games. BFEN 2.0 defines a standard way to represent both boards in a single string.

### Format

```
<bfen_board_a> | <bfen_board_b>
```

- Pipe `|` separator with optional surrounding whitespace
- Board A always listed first
- Each side is independently a valid single-board BFEN string
- Consistent with BPGN two-board convention

### Two-Board Examples

**Starting position (both boards):**
```
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1 | rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR[] w KQkq - 0 1
```

**Mid-game with reserves:**
```
r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R[NNPp] w KQkq - 4 5 | rnbqkb1r/ppp1pppp/8/3p4/2PP4/8/PP2PPPP/RNBQKBNR[Qbpp] b KQkq - 0 3
```

**With promoted pieces:**
```
r2q1rk1/ppp2ppp/2n1bn2/3p4/3P4/2NB~/5N2/R1BQ1RK1[QNNPrbp] b - - 0 12 | r1bqkb1r/pppp1ppp/2n2n2/4p3/4P3/5N2/PPPP1PPP/RNBQK2R[Pp] w KQkq - 2 4
```

### Parsing Two-Board BFEN

Split on ` | ` (pipe with surrounding spaces) to get two independent single-board BFEN strings. Each half can be parsed using the standard single-board parser.

**Important:** The pipe separator appears between two complete BFEN strings (each containing spaces for their own fields), so split on ` | ` rather than just `|` to avoid ambiguity.

---

## BPGN BFEN Conversion

For interoperability with legacy BPGN tools, here are conversion functions between slash-based (BPGN) and bracket-based (BFEN 2.0) reserve notation.

### Conversion Logic

```
BPGN:  position/reserves color castling ep half full
BFEN:  position[reserves] color castling ep half full
```

The slash-based reserves are the "9th rank" — the last segment of the position field (after splitting on `/`) before the space-separated metadata.

### Python

```python
def bpgn_to_bfen(bpgn_str):
    """Convert BPGN BFEN (slash reserves) to BFEN 2.0 (bracket reserves)."""
    parts = bpgn_str.split(' ')
    position_with_reserves = parts[0]
    metadata = parts[1:]  # color, castling, ep, half, full

    # Split position ranks — the 9th "rank" is reserves
    ranks = position_with_reserves.split('/')
    if len(ranks) == 9:
        position = '/'.join(ranks[:8])
        reserves = ranks[8]
    else:
        position = '/'.join(ranks)
        reserves = ''

    return f"{position}[{reserves}] {' '.join(metadata)}"


def bfen_to_bpgn(bfen_str):
    """Convert BFEN 2.0 (bracket reserves) to BPGN BFEN (slash reserves)."""
    bracket_start = bfen_str.index('[')
    bracket_end = bfen_str.index(']')

    position = bfen_str[:bracket_start]
    reserves = bfen_str[bracket_start + 1:bracket_end]
    metadata = bfen_str[bracket_end + 1:].strip()

    return f"{position}/{reserves} {metadata}"
```

### JavaScript

```javascript
function bpgnToBfen(bpgnStr) {
    const parts = bpgnStr.split(' ');
    const ranks = parts[0].split('/');
    const metadata = parts.slice(1).join(' ');

    let position, reserves;
    if (ranks.length === 9) {
        position = ranks.slice(0, 8).join('/');
        reserves = ranks[8];
    } else {
        position = ranks.join('/');
        reserves = '';
    }

    return `${position}[${reserves}] ${metadata}`;
}

function bfenToBpgn(bfenStr) {
    const bracketStart = bfenStr.indexOf('[');
    const bracketEnd = bfenStr.indexOf(']');

    const position = bfenStr.substring(0, bracketStart);
    const reserves = bfenStr.substring(bracketStart + 1, bracketEnd);
    const metadata = bfenStr.substring(bracketEnd + 1).trim();

    return `${position}/${reserves} ${metadata}`;
}
```

### Elixir

```elixir
defmodule BFENConvert do
  def bpgn_to_bfen(bpgn_str) do
    [position_with_reserves | metadata] = String.split(bpgn_str, " ", parts: 2)
    ranks = String.split(position_with_reserves, "/")

    {position, reserves} =
      if length(ranks) == 9 do
        {Enum.take(ranks, 8) |> Enum.join("/"), List.last(ranks)}
      else
        {Enum.join(ranks, "/"), ""}
      end

    "#{position}[#{reserves}] #{metadata}"
  end

  def bfen_to_bpgn(bfen_str) do
    [before_bracket, rest] = String.split(bfen_str, "[", parts: 2)
    [reserves, after_bracket] = String.split(rest, "]", parts: 2)

    "#{before_bracket}/#{reserves} #{String.trim(after_bracket)}"
  end
end
```

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
- **Move history:** Integration with PGN-style notation
- **Variant rules:** Extensions for different bughouse rule sets

---

## References

- [Fairy-Stockfish Chess Variant Standards](https://fairy-stockfish.github.io/chess-variant-standards/fen.html)
- [Standard FEN Specification](https://en.wikipedia.org/wiki/Forsyth%E2%80%93Edwards_Notation)
- [Bughouse Chess Rules](https://en.wikipedia.org/wiki/Bughouse_chess)
- [BPGN Specification](http://bughousedb.com/Lieven_BPGN_Standard.txt)
- [Bughouse Database](http://bughousedb.com/)

---

## Version History

- **v2.0** (2025-02-21) - Modernized specification
  - Two-board representation with pipe separator
  - BPGN BFEN compatibility section with conversion examples
  - Acknowledged heritage from BPGN BFEN (circa 2002)
- **v0.1** (2025-02-04) - Initial specification
