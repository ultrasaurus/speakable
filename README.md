# speakable

Rust crates for highlighting text sentence by sentence as its audio plays.

1. Turn markdown into the plain text a voice should read. (speakable-markdown)
2. Split text into sentences. (speakable-splitter)
3. Rewrite each sentence into the words that would be spoken by a reader.
  (speakable-normalizer)
4. Generate sentence timings for an audio file (speakable-align)
5. Generate HTML, where sentence spans have timestamps (speakable-timings)

The `speakable` CLI uses them together so that the sentences a voice speaks are
the same sentences a page highlights.

```
cargo install --path crates/speakable
speakable demo sample.md --audio sample.mp3 -o sample.html
```
then open sample.html in a browser

English only for now.

## Why sentences?

When seeking, starting at the beginning of a sentence is a natural place to
start audio playing. Also, highlighting text that follows the voice adds:

- **A visual anchor:** the eyes stay on the text being spoken.
- **External pacing:** the highlight moves at the voice's pace, so reading
  becomes a guided process.

This only works if the page and the audio agree on every sentence. These
crates exist to make that true.

## Crates

| Crate | What it does |
|---|---|
| [`speakable-splitter`](crates/speakable-splitter) | Plain text → sentences, with paragraph boundaries. |
| [`speakable-markdown`](crates/speakable-markdown) | Markdown → speakable plain text, split into sentences. Math is spoken through [`math-to-speech`](https://github.com/ultrasaurus/math-to-speech). |
| [`speakable-normalizer`](crates/speakable-normalizer) | One sentence → its spoken form ("1976-77" → "nineteen seventy six to seventy seven"), with a source-span map back to the original text. |
| [`speakable-timings`](crates/speakable-timings) | Sentence timings from timed words (a transcript or forced alignment): the JSON format, the word-to-sentence mapper, a Descript transcript reader, and a demo page. |
| [`speakable-align`](crates/speakable-align) | Sentence and word timings from an audio file, by forced alignment ([forced-alignment-rs](https://github.com/ultrasaurus/forced-alignment-rs)): normalizes the text, aligns, and maps the times back to the document's words. |
| [`speakable`](crates/speakable) | The `speakable` CLI: `speakable descript` writes timings from a Descript transcript, `speakable align` writes them from the audio itself, and `speakable demo` writes a page that highlights each sentence as it plays. |

## Using them

All crates share one version and one git tag. Depend on each crate by
package name from this repo, at the same tag:

- Cargo builds only the crates you name.
- Use one tag for every crate. Then the splitter inside
  `speakable-markdown` is the same splitter you call directly.

## Development

```sh
cargo nextest run
```

The workspace sets `opt-level = 3` for `speakable-normalizer` in dev
builds: unoptimized, it runs about 9x slower.

## Samples

[`samples/`](samples/) holds recordings with their text, for trying
speakable. `samples/` is not under the MIT/Apache license below: each
sample's README gives its terms.

## Acknowledgements

Some tests quote short excerpts (fair use) from published works, because
that real text exposed a bug. Each crate's README credits the works its
tests quote.

## Code License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
