# mtg-deck-snap

Convert photographs of Magic: The Gathering card spreads into `.dck` deck list files.

You spread your cards on a table so all the **titles** are visible, snap a photo (or several), and `mtg-deck-snap` will:

1. Load and (if needed) downscale/tile the image into AI-readable chunks
2. Send each tile to Claude for card-name extraction
3. Fuzzy-match every extracted name against the Scryfall card database (38 000+ cards)
4. Print a confidence report table
5. Validate the deck (4-of rule, size checks, land count sanity)
6. Walk you through ambiguous matches in an interactive wizard
7. Output a `.dck` file ready to load into Forge or other MTG software

## Build

Requires Rust stable (tested on 1.87+).

```bash
cargo build --release
```

## Setup

### Card database

Download the Scryfall card-name cache (runs once, then reuses):

```bash
mtg-deck-snap update-db
# → Cached 38 000+ unique card names to ~/.cache/mtg-deck-snap/scryfall-names.json
```

You can also point at a Forge `cardsfolder/` directory with `--cardsfolder <path>`.

### API key

The vision pipeline uses the Anthropic Claude API. Export your key:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

## Usage

### Scan a deck photo

```bash
# Basic — outputs .dck to stdout
mtg-deck-snap scan photo.jpg

# Full options
mtg-deck-snap scan photo1.jpg photo2.jpg \
  --deck-size 60 \
  --name "Mono Red Aggro" \
  --output my_deck.dck

# Non-interactive (auto-accept best matches, cap illegal counts, adjust lands)
mtg-deck-snap scan photo.jpg --non-interactive --deck-size 40 -o deck.dck

# Multi-pass: runs a second AI call to re-verify card counts
mtg-deck-snap scan photo.jpg --multi-pass --deck-size 60 -o deck.dck
```

### Bypass AI vision with a text file

If you've already extracted card names (one per line, duplicates included):

```bash
mtg-deck-snap scan --from-list raw_names.txt --deck-size 40 -o deck.dck
```

### Search the card database

```bash
mtg-deck-snap list-db --search "lightning"
```

### Force-refresh the database

```bash
mtg-deck-snap update-db
```

## .dck format

Output is Forge-compatible INI-style:

```ini
[metadata]
Name=My Deck

[Main]
4 Lightning Bolt
4 Monastery Swiftspear
20 Mountain

[Sideboard]
2 Smash to Smithereens
```

## Pipeline details

### Image handling

- Images over 4096px on the longest side are **downscaled** with Lanczos3 before processing.
- If the (possibly downscaled) image is still above 4096px, it's split into **overlapping 1536×1536 tiles** with 192px overlap so card titles at tile boundaries appear in full on at least one tile.
- Smaller images are sent as a single tile.

### AI vision

Each tile is sent to the Claude API with a structured prompt that asks for card names (one per line, duplicates included for multiple copies). The **raw AI output** is printed to stderr for transparency and debugging.

With `--multi-pass`, a second API call sends the same image along with the list of detected card names and asks the model to re-count how many copies of each card are visible. The conservative (lower) count wins.

### Fuzzy matching

Each AI-extracted name is scored against the card database using a weighted blend of Jaro-Winkler (70%) and normalised Levenshtein (30%) similarity. Matches are bucketed into confidence tiers:

| Tier   | Score     | Behaviour                       |
|--------|-----------|---------------------------------|
| Exact  | 1.0       | Auto-accepted                   |
| High   | ≥ 0.90    | Auto-accepted                   |
| Medium | 0.70–0.90 | Interactive confirmation        |
| Low    | < 0.70    | Interactive — show alternatives |

A confidence table is always printed to stderr showing every match:

```
  Card Name                    Extracted As               Score  Conf      Qty
  ─────────                    ────────────               ─────  ────      ───
  Lightning Bolt               =                           1.00  exact       4
  Otter-Penguin                =                           1.00  exact       2
  Some Card                    Som Card?                   0.85  medium      1
```

### Validation

- **4-of rule**: non-basic cards with > 4 copies are flagged; the wizard offers to cap.
- **Deck size**: if `--deck-size` is provided, mismatches are reported.
- **Land count**: if total exceeds expected deck size and basic lands are the culprit, the wizard asks whether to adjust. In `--non-interactive` mode, land counts are auto-adjusted proportionally.
- **Sanity bounds**: very small (< 30) or very large (> 120) totals trigger warnings.

## Test results

### Avatar: The Last Airbender draft deck

Tested with a 5712×4284 JPEG of an Avatar draft deck (~40 card limited):

```
Input:  5712×4284 → downscaled to 4096×3072 → 1 tile
Pass 1: 44 raw card names extracted in ~12 seconds
Match:  40 exact, 1 high, 3 medium confidence
Output: 21 unique cards (11 Forest, 10 Island, 2× Allies at Last, etc.)
```

The AI over-counted by 4 cards (stacked basic lands). With `--deck-size 40`, the land sanity check catches this and offers to adjust.

## Known limitations

- **Stacked/fanned basic lands** are the most common source of miscounts. The AI can't reliably count cards in a stack — use `--deck-size` to catch this, or enter correct counts interactively.
- **False positives** are possible when the AI hallucinates card names from art or partial text. The confidence table helps identify these.
- **Multi-pass** reduces count errors but doubles API cost (2× Claude calls per tile).
- **Sideboard** cards aren't automatically separated — all cards go to `[Main]`. Move sideboard cards manually in the output file.
- **Foil glare** can make titles unreadable. Try to photograph in diffuse lighting.

## Tips for good results

- Spread cards so **all titles are visible** — art and text boxes can be obscured.
- Use good lighting, avoid glare on foils.
- For large decks (60+), take 2–3 overlapping photos rather than one distant shot.
- Tell the tool your expected deck size (`--deck-size`) for better validation and land adjustment.
- Basic lands can have any count; everything else is capped at 4.

## License

MIT
