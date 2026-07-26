//! Shared prompts used by every vision backend.
//!
//! Keeping the prompt text in one place guarantees that switching `--backend`
//! only changes *how* the image reaches a model, never *what* we ask it to do.

/// Pass 1 — card-name identification prompt.
pub fn extract(deck_size_hint: Option<u32>) -> String {
    let hint = deck_size_hint
        .map(|n| {
            format!(
                " The full deck is expected to have roughly {n} cards total (across all images)."
            )
        })
        .unwrap_or_default();

    format!(
        "You are analyzing a photograph of Magic: The Gathering cards spread on a table. \
         The player has arranged the cards so that all card TITLES are visible (the full \
         card art/text may be obscured).{hint}\n\n\
         INSTRUCTIONS:\n\
         1. Read each card's TITLE BAR carefully — it is the text at the very top of the card.\n\
         2. Output ONLY card names, one per line, with no numbering, bullets, or commentary.\n\
         3. Include duplicates — if you see two copies of the same card, list the name twice.\n\
         4. For stacked basic lands (Island, Forest, etc.), count the number of cards in the \
            stack by looking at the visible edges/corners. Be precise — do not guess.\n\
         5. If you can only partially read a name, give your best guess followed by a ? suffix.\n\
         6. If a name is completely illegible, skip it.\n\
         7. Pay close attention to similar-looking letters: e vs o, i vs l, t vs f, etc.\n\
         8. MTG card names are proper nouns — capitalize each word."
    )
}

/// Pass 2 — count-verification prompt for a set of already-detected names.
pub fn recount(unique_names: &[String]) -> String {
    let card_list = unique_names.join("\n");
    format!(
        "You are re-examining a photograph of Magic: The Gathering cards spread on a table.\n\n\
         Here are the card names we detected in a first pass:\n{card_list}\n\n\
         Now count how many PHYSICAL COPIES of each card you can see in THIS image.\n\
         Output ONLY lines in the format:  COUNT CARD_NAME\n\
         For example:\n  2 Lightning Bolt\n  1 Mountain\n\n\
         If a card from the list is not visible in this specific image tile, omit it.\n\
         Do NOT invent cards that are not in the list above."
    )
}
