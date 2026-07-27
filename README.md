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

## Installation

### Download release (Linux x86_64)

```bash
# Download latest release
curl -L https://github.com/rrnewton/mtg-deck-snap/releases/latest/download/mtg-deck-snap-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv mtg-deck-snap /usr/local/bin/
```

Or grab the tarball from the [Releases page](https://github.com/rrnewton/mtg-deck-snap/releases).

### Build from source

Requires Rust stable (tested on 1.87+).

```bash
cargo build --release
# binary at target/release/mtg-deck-snap
```

## Setup

### Card database

Download the Scryfall card-name cache (runs once, then reuses):

```bash
mtg-deck-snap update-db
# → Cached 38 000+ unique card names to ~/.cache/mtg-deck-snap/scryfall-names.json
```

You can also point at a Forge `cardsfolder/` directory with `--cardsfolder <path>`.

### Vision backend

The vision pipeline is **pluggable** — pick how card-name extraction talks to a
model with `--backend`:

| `--backend`      | How it works                                        | API key? | Cost                |
|------------------|-----------------------------------------------------|----------|---------------------|
| `claude-cli`     | Shells out to the local `claude` CLI **(default)**  | No       | Uses your Claude subscription |
| `gemini`         | Shells out to the local `gemini` CLI                | No       | Uses your Gemini subscription |
| `codex`          | Shells out to the local `codex` CLI                 | No       | Uses your Codex subscription  |
| `anthropic-api`  | Calls `api.anthropic.com` directly                  | Yes      | Per-call API billing          |

The default, `claude-cli`, needs **no API key**: if you're signed in to a Claude
Max/Pro subscription (`claude` on your PATH), extraction runs against your
subscription with no per-call billing. The `gemini` and `codex` backends work
the same way with their respective CLIs and subscriptions.

Use `--model` to override the backend's default model (a backend-specific slug,
e.g. `--backend gemini --model "gemini-2.5-flash"`).

Only the `anthropic-api` backend needs a key:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
mtg-deck-snap scan photo.jpg --backend anthropic-api
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

# Pick a vision backend (default is claude-cli — no API key needed)
mtg-deck-snap scan photo.jpg --backend claude-cli
mtg-deck-snap scan photo.jpg --backend gemini --model "gemini-2.5-flash"
mtg-deck-snap scan photo.jpg --backend anthropic-api   # needs ANTHROPIC_API_KEY
```

### Batch mode — process a whole tree of decks

`batch` walks a directory tree and converts many deck photos in one
non-interactive run (auto-accept; it never blocks on prompts):

```bash
# Walk examples/inputs/ recursively and write results to examples/outputs/
mtg-deck-snap batch --deck-size 40

# Custom roots, and reprocess everything even if outputs already exist
mtg-deck-snap batch --inputs-root photos --outputs-root results --force

# Process only specific deck directories (still mirrored under --outputs-root)
mtg-deck-snap batch examples/inputs/secrets_of_strixhaven_2026/amber_strix1_BG
```

How it works:

- **Deck directory** = any directory holding exactly one top-level image
  (`jpg`/`jpeg`/`png`/`heic`/`webp`). That image is the primary photo. A
  `.DS_Store` or other non-image file is ignored, and any `extra/` subfolder is
  ignored for now (reserved for a future multi-image-confidence feature).
- Directories with no top-level image (e.g. a batch folder like
  `secrets_of_strixhaven_2026/`, or the inputs root itself) are containers and
  are descended into recursively.
- Each deck dir's path *relative to `--inputs-root`* is **mirrored** into
  `--outputs-root`. For example
  `examples/inputs/secrets_of_strixhaven_2026/amber_strix1_BG/IMG_4171.jpeg`
  produces
  `examples/outputs/secrets_of_strixhaven_2026/amber_strix1_BG/{amber_strix1_BG.dck, metadata.json}`.
- `metadata.json` is machine-readable: the raw extracted names, every fuzzy
  match with score + confidence, the final deck list, validation results, the
  backend/model used, and a timestamp. The output directory is designed to hold
  more metadata as the pipeline improves.
- **Idempotent**: a deck dir whose output `.dck` already exists is skipped
  (no vision call). Pass `--force` to reprocess.
- Uses the `claude-cli` backend by default (no API key — your Claude
  subscription).

### Example inputs / outputs

The `examples/` directory demonstrates batch mode:

- `examples/inputs/` — the source photos. These are large (~140 MB) and are
  **not committed** (git-ignored). Populate them with
  `examples/fetch-inputs.sh` (downloads a public Google Drive folder via
  `gdown`; run `make install-deps` first to provision `gdown` via `uv`).
- `examples/outputs/` — the generated `.dck` + `metadata.json` fixtures. These
  are small and **are committed**, so the expected results are reviewable
  without the images.

Because outputs mirror inputs' subpaths and never share filenames with the
images, `rsync examples/outputs/ examples/inputs/` reproduces the interleaved
view without overwriting anything.

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

Each tile is sent to the selected vision backend (see [Vision backend](#vision-backend)) with a structured prompt that asks for card names (one per line, duplicates included for multiple copies). All backends share the same prompt and the same output parsing — `--backend` only changes *how* the image reaches a model. The **raw AI output** is printed to stderr for transparency and debugging.

With `--multi-pass`, a second call sends the same image along with the list of detected card names and asks the model to re-count how many copies of each card are visible. The conservative (lower) count wins.

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

## Backlog

v0.1 — shipped June 2026
- Photo → .dck pipeline with Claude vision + Scryfall fuzzy match
- Set-coherence outlier detection
- Interactive wizard, confidence table, validation

v0.2
- `--backend` flag — pluggable vision backends: `claude-cli` (default, no API
  key), `gemini`, `codex`, and `anthropic-api`. The CLI backends run on a
  Claude/Gemini/Codex subscription with no per-call billing.

Planned:
- OCR via Tesseract for offline / no-API-key mode
- Sideboard detection and `[Sideboard]` output
- Multi-image deck stitching with duplicate suppression
- Collection mode — scan binders/boxes to CSV inventory
- Price lookup via Scryfall / TCGplayer

## License

MIT
