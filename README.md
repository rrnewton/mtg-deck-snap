# mtg-deck-snap

Convert photographs of Magic: The Gathering card spreads into `.dck` deck list files.

You spread your cards on a table so all the **titles** are visible, snap a photo (or several), and `mtg-deck-snap` will:

1. Tile the image into AI-readable chunks
2. Send each tile to Claude for card-name extraction
3. Fuzzy-match every extracted name against the Scryfall card database (38 000+ cards)
4. Validate the deck (4-of rule, size sanity checks)
5. Walk you through ambiguous matches in an interactive wizard
6. Output a `.dck` file ready to load into Forge or other MTG software

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
export ANTHROPIC_API_KEY="sk-ant-..."
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

# Non-interactive (auto-accept best matches, cap illegal counts)
mtg-deck-snap scan photo.jpg --non-interactive -o deck.dck
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

### Image tiling

Large photos (> 2048px on longest side) are split into overlapping 1536×1536 tiles
with 192px overlap so card titles that straddle a boundary still appear in full on at
least one tile.

### Fuzzy matching

Each AI-extracted name is scored against the card database using a weighted blend of
Jaro-Winkler (70%) and normalised Levenshtein (30%) similarity. Matches are bucketed
into confidence tiers:

| Tier   | Score      | Behaviour                            |
|--------|-----------|--------------------------------------|
| Exact  | 1.0        | Auto-accepted                        |
| High   | ≥ 0.90     | Auto-accepted                        |
| Medium | 0.70–0.90  | Interactive confirmation              |
| Low    | < 0.70     | Interactive — show alternatives       |

In `--non-interactive` mode, best matches are always accepted.

### Validation

- **4-of rule**: non-basic cards with > 4 copies are flagged; the wizard offers to cap.
- **Deck size**: if `--deck-size` is provided, mismatches are reported.
- **Sanity bounds**: very small (< 30) or very large (> 120) totals trigger warnings.

## Tips for good results

- Spread cards so **all titles are visible** — art and text boxes can be obscured.
- Use good lighting, avoid glare on foils.
- For large decks (60+), take 2–3 overlapping photos rather than one distant shot.
- Tell the tool your expected deck size (`--deck-size`) for better validation.
- Basic lands can have any count; everything else is capped at 4.

## License

MIT
