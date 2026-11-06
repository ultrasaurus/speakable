# speakable-splitter

Splits plain text into sentences for reading aloud.

## Why

Highlighting each sentence while audio plays only works if the text and the
audio timings use the same split. A general-purpose sentence splitter breaks on things that text-to-speech text is full of:

- abbreviations ("Vol. 31, No. 7", "Dr.", "e.g.")
- footnote and citation markers (`[1]`, `*1*`, `[^3]`, `[Fox, Works, 8:42]`)
- outline labels ("1.A.", "(1)")
- ellipses and quotes at sentence ends

This crate keeps each of those with the sentence it belongs to.

## What it does

- `split(text)` returns the sentences in order. Each sentence has an `id`,
  its `text`, and `paragraph_end`.
- Each non-blank line is a paragraph. Blank lines are allowed but not
  needed.
- The sentences are exact, in-order slices of the input. No characters are
  added or lost.
- Footnote and citation markers stay with the sentence they cite (see
  "Footnote and citation markers" below).
- Very long sentences are cut so that each piece fits in one TTS request
  (see "Long sentences" below).

```rust
let sentences = speakable_splitter::split("Dr. Smith arrived. He sat.\nNew paragraph.");
assert_eq!(sentences.len(), 3);
assert!(sentences[1].paragraph_end);
```

Each step of the split, and why it's there, is in
[docs/splitter.md](../../docs/splitter.md).

## Footnote and citation markers

A marker is wrapped in `[...]`, `(...)` or `*...*`, glued to the sentence or
after a space. Inside, it can be:

- **Numbers** (IEEE/Vancouver style): `[1]`, `[3,4]`, `[5-7]`, `[1, 4–8]`.
  A range can use a hyphen or an en dash.
- **Markdown footnote references:** `[^3]`.
- **Citation fields:** two or more fields separated by `,` or `;`, at least
  one containing a digit. This covers APA (`(Smith, 2022, p. 15)`,
  `(Smith, 2020; Jones, 2023)`) and `[Fox, Works, 8:42]`. A comma inside a
  quoted title doesn't split a field.
- **Statement references:** a word, a number and an empty field, as in
  `(AUGMENT,71279,)`.

A parenthetical with no digit, like `(see appendix)`, is not a marker.

A marker joins the end of the sentence before it. A marker with no sentence
before it in its paragraph stays as its own sentence. Two helpers let a
synthesizer decide what to skip:

- `has_speakable_content(text)` is false for a sentence with no letters or
  digits (a bare `—` or `***`), which has nothing to say.
- `is_footnote_marker(text)` is true for a sentence that is only a marker,
  like `[1]`. A synthesizer that skips sentences without letters can use it
  to still speak the marker ("Footnote one." once normalized).

## Long sentences

Some "sentences" have no sentence-ending punctuation for hundreds of words,
such as a reference-list entry with a long author list. A TTS request that
long fails, so the splitter cuts them.

- **Spoken length, estimated:** a voice says more words than the text shows.
  "CJ" is said "C J", and "1976" is several words. The splitter estimates
  the spoken word count with simple rules, without running a normalizer:
  - A 2–3 letter all-caps token counts one word per letter.
  - A number counts about one word per digit.
  - A hyphen or dash splits a token into separate words.
  - A token with no letters or digits counts zero.
- **The cap:** a sentence estimated at more than `MAX_SENTENCE_WORDS` (300)
  spoken words is cut into roughly equal pieces of about 250, leaving room
  for the estimate's error.
- **Where it cuts:** after a word ending in `,` `;` `:` `—` or `–`, within
  15 words of each cut point, or at a plain word boundary if there is
  none. No word is broken.
- Each piece is a real sentence in the output.

## Acknowledgements

Some tests quote short excerpts (fair use) from the works below, because
that real text exposed a bug. Thanks to their authors.

- Douglas C. Engelbart, "Augmenting Human Intellect: A Conceptual
  Framework" (1962).
- Douglas C. Engelbart, "Authorship Provisions in AUGMENT" (1984):
  citation fragments.
- Andries van Dam, "Hypertext '87 Keynote Address" (1987).
- Sandra Cronk, *Gospel Order* (Pendle Hill pamphlet #271, 1991):
  citations of George Fox, Isaac Penington and Thomas Kelly.
- Brian McFee, *Digital Signals Theory*: a section heading.
- The Project Gutenberg License: section numbering (1.A., 1.E.1.).
