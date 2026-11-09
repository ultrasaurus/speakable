# speakable-normalizer

Rewrites an English sentence into the words that would be spoken by a
reader.

## Why

TTS voices misread numbers, symbols, abbreviations, citations and stray
punctuation, and some produce noise on them. Forced alignment matches
audio against words, so it needs the spoken words too. This crate writes
those words out, and keeps a map from them back to the original text.

## What it does

- `normalize(text)` returns the spoken form:
  - "1976-77" → "nineteen seventy six to seventy seven"
  - "$45.50" → "forty five dollars and fifty cents"
  - "3.14" → "three point fourteen"
  - "5 ms" → "five milliseconds", "1 lb" → "one pound": a unit after a
    number is spoken as a word, singular or plural to match. Covers `ms`,
    `µs`, `ns`, `kb`, `mb`, `gb`, `dB`, `cm`, `cm2`, `ft`, `lb`, `lbs`.
  - "cycles/sec" → "cycles per second"
  - "9:00 AM" → "nine A M": on-the-hour minutes are dropped, and AM/PM is
    spelled out after a time.
  - "Mt. 5:30" → "Matthew chapter five verse thirty"
  - `[1]` → "Footnote one."
  - URLs → "link"
  - Short all-caps acronyms are spelled out.
  - Brackets and stray punctuation are removed.
- `normalize_with_spans(text)` also returns a map from ranges of the output
  back to ranges of the input, for mapping forced-alignment word times
  onto the original sentence.
  - `source_words(spoken)` maps each spoken word, such as the words an
    aligner timed, to its range in the input. It matches whole words in
    order, so words the aligner dropped are skipped.
  - `Normalizer::normalize_pieces_with_spans(text, pieces)` normalizes
    each piece of the text on its own, such as one paragraph per line, and
    keeps the map back to the whole text. Map words back with it when
    the audio was made from the pieces.
- `spoken_word_count(text)` counts the spoken words. It isn't cached: cache
  it yourself, keyed by `Normalizer::fingerprint()` and the text.

Every pass, in order and with examples, is in
[docs/normalizer.md](../../docs/normalizer.md).

## Override tables

An override table maps a word or phrase to its spoken form. It's
tab-separated text, one entry per line, with `# ` comments:

```text
e.g.	for example
dB	decibels
```

- A key with no punctuation is matched as a whole word, case-insensitively.
- A key with punctuation is matched as a literal substring. It's
  case-sensitive if it contains an uppercase letter.
- The crate ships a built-in table, `BUILTIN_OVERRIDES`, of general spoken
  forms. The free functions use it.
- The crate reads and writes no files. Load your own table and pass it in:

```rust
use speakable_normalizer::{parse_overrides, Normalizer};

let mine = parse_overrides("Byrne\tBurn\n");
let n = Normalizer::builtin().with_overrides(mine);
assert_eq!(n.normalize("Byrne wrote it in 1976."), "Burn wrote it in nineteen seventy six.");
```

An entry in the layered table wins over a built-in entry with the same key.

## Acknowledgements

Some tests quote short excerpts (fair use) from the works below, because
that real text exposed a bug. Thanks to their authors.

- Douglas C. Engelbart, "Augmenting Human Intellect: A Conceptual
  Framework" (1962).
- Douglas C. Engelbart, "Authorship Provisions in AUGMENT" (1984):
  citation fragments.
- Andries van Dam, "Hypertext '87 Keynote Address" (1987).
- Virginia Woolf, *A Room of One's Own* (1929).
- Jules Verne, *Twenty Thousand Leagues Under the Sea* (English
  translation, public domain).
- New England Yearly Meeting Ministry and Counsel Committee, "Living with
  Oneself and Others" (2001): an attribution line naming William Penn
  (1694).
