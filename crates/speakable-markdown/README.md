# speakable-markdown

[![crates.io](https://img.shields.io/crates/v/speakable-markdown.svg)](https://crates.io/crates/speakable-markdown)
[![docs.rs](https://img.shields.io/docsrs/speakable-markdown)](https://docs.rs/speakable-markdown/latest/speakable_markdown/)

Turns markdown into the plain text a voice should read, and splits it into
sentences.

## Why

To highlight or select a sentence, the page needs a span per sentence, and to
generate or process the audio for timings, the spoken text needs the same
sentences in the same order. This crate makes both from one markdown source:

- **Per-block sentence counts:** a renderer walking the markdown knows which
  sentences belong to which paragraph, list item or table cell.
- **Each sentence's own markdown:** bold or a link that crosses a sentence
  boundary is closed and reopened, so each sentence renders as its own span.
- **Shown but not said:** images, code and silent text (see below) appear on
  the page but not in the spoken text. Math is shown as written and spoken
  in words.

## Supported Markdown

[CommonMark](https://commonmark.org/), parsed with
[pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark), plus:

- **Tables** (GitHub style). Each cell is its own block, and each row ends
  with a period so a voice pauses between rows.
- **Math:** `$...$` and `\(...\)` inline, `$$...$$` and `\[...\]` as a block.
- **Silent text:** `[text]<!--silent-->` is shown on the page but not
  spoken. `to_plain_text` drops it. A heading that is entirely silent
  produces no spoken block at all:

  ```markdown
  ## [Chapter 3]<!--silent-->
  ```

Other extensions are not enabled, so their syntax is spoken as written:
`~~strikethrough~~`, footnotes (`[^1]`) and task-list boxes (`[ ]`).

HTML:

- An HTML block (`<div>...</div>` on its own lines) is dropped with its
  contents.
- Inline tags (`<b>bold</b>`) are dropped, keeping their text.
- Comments are dropped.
- Text in angle brackets that isn't a real HTML tag name, like `<Ref-1>`,
  is kept and spoken. CommonMark parses it as raw HTML, so a renderer
  should show it as text too.


## What it does

- `to_plain_text(markdown)` returns the text to speak:
  - Inline markup is removed, and links become their text.
  - Code blocks, images and horizontal rules are dropped.
  - Each block becomes one paragraph line.
  - Table rows end with a period, so a voice pauses between them.
  - Silent text is dropped.
- Math (`$...$`, `$$...$$`, `\(...\)`, `\[...\]`) is spoken through
  [`math-to-speech`](https://github.com/ultrasaurus/math-to-speech). The
  `_with_math_overrides` variants take a table of spoken forms.
- `split_for_export(markdown)` returns each sentence's plain text and
  markdown source, plus how many sentences each block has.
- `split_markdown_sentences_for_render` splits one block's markdown into
  sentences. Emphasis or a link that crosses a sentence boundary is closed
  and reopened, so each piece renders on its own.
- `to_html_with_sentence_spans(markdown, times)` renders the markdown to
  HTML with each sentence in a
  `<span class="sentence" data-sentence="i" start=".." end="..">`, for a
  page that highlights sentences as audio plays. The sentences are exactly
  `split_for_export`'s. Silent text on a line of its own is shown in
  `<span class="silent">`; silent text sharing a line with spoken text
  isn't shown, and math is shown as its spoken words.
- Helpers for cleaning up markdown converted from other formats:
  `from_plain_text`, `linkify`, image helpers, table repairs.

```rust
let text = speakable_markdown::to_plain_text("# Title\n\nSome **bold** text. A [link](https://example.com).");
let sentences = speakable_splitter::split(&text);
```

## Acknowledgements

Some tests quote short excerpts (fair use) from the works below, because
that real text exposed a bug. Thanks to their authors.

- Sandra Cronk, *Gospel Order* (Pendle Hill pamphlet #271, 1991): a
  citation of George Fox's *Works*.
- Jonathan Swift, *Gulliver's Travels*: table of contents (public domain).
