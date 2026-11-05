# speakable

Rust crates that support reading a markdown document aloud.

- Split text into sentences.
- Turn markdown into the plain text a voice should read.
- Rewrite each sentence into the words that would be spoken by a reader.

Use them together so that the sentences a voice speaks are the same
sentences a page highlights.

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

## Using them

All crates share one version and one git tag. Depend on each crate by
package name from this repo, at the same tag:

```toml
[dependencies]
speakable-splitter = { git = "https://github.com/ultrasaurus/speakable", tag = "v0.1.0" }
speakable-markdown = { git = "https://github.com/ultrasaurus/speakable", tag = "v0.1.0" }
speakable-normalizer = { git = "https://github.com/ultrasaurus/speakable", tag = "v0.1.0" }
```

- Cargo builds only the crates you name.
- Use one tag for every crate. Then the splitter inside
  `speakable-markdown` is the same splitter you call directly.

## Development

```sh
cargo nextest run
```

The workspace sets `opt-level = 3` for `speakable-normalizer` in dev
builds: unoptimized, it runs about 9x slower.

## Acknowledgements

Some tests quote short excerpts (fair use) from published works, because
that real text exposed a bug. Each crate's README credits the works its
tests quote.

## Code License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
