# speakable-timings

Sentence timings: a start and end time for each sentence of a markdown
document, from one recording of it.

## Why

A page that highlights each sentence while its audio plays needs to know
when each sentence is spoken. The times come from timed words (a speech
transcript, or forced alignment), and those words never match the text
exactly: a reader adds or skips words, and a transcript mishears names or
joins words differently. This crate lines the two up, writes the result in
one small JSON format, and checks it against the document later.

## What it does

- `Timings`: the JSON format (version 0.1), with `from_json`, `to_json`, and
  `check(markdown)`, which fails when the document's sentences no longer
  match the timed ones.
- `map_words(sentence_words, timed_words)` maps timed words onto
  sentences with a word-level diff (from
  [`similar`](https://github.com/mitsuhiko/similar)):
  - Words compare case-insensitively, without punctuation, Unicode-aware.
  - A matched word gives its times to its text word.
  - A run of text words replaced by other timed words ("would have" heard
    as "would've") takes the span of those timed words. Its words are
    reported `Replaced`.
  - When a replaced run crosses a sentence boundary, its span is shared
    among its words by count, and they're reported `BoundaryUncertain`.
  - Timed words that match no text are reported as an insertion.
  - Text words with no timed words are reported missing. A sentence with
    no timed words gets no times.
- `DescriptTranscript` reads a Descript word-level transcript export, and
  `timings_from_descript` maps it onto a document's plain-text sentences.
- `render_demo` writes a one-file HTML page: the document with each
  sentence in a span, an audio player, the spoken sentence highlighted,
  and click-a-sentence-to-seek.

The sentences are the ones `speakable-markdown` splits and puts in spans,
so sentence `i` in the JSON is `data-sentence="i"` on the page.

## Format

```json
{
  "version": "0.1",
  "audio": "doc.mp3",
  "sentences": [
    { "index": 0, "text": "...", "start": 0.42, "end": 3.10 }
  ]
}
```

- `version` is `"major.minor"`, with semver's rules. Before 1.0 a new
  minor version may change the format, so a reader accepts only its own.
  From 1.0 a minor version only adds optional fields, so a reader accepts
  any version with its own major.
- `audio` is relative to the JSON file's folder.
- Times are in seconds, to the millisecond. A sentence with no times has
  no `start` or `end`.
- `text` is the sentence's plain text, before any normalization, so a
  reader can detect an edited document.

### Report

Timings can also carry a `report` of where the text and the audio differ
(the CLI writes one with `--report`). It's left out when empty.

```json
{
  "version": "0.1",
  "audio": "doc.mp3",
  "sentences": [ ... ],
  "report": {
    "insertions": [
      { "before_sentence": 0, "word_index": 0, "start": 0.0, "end": 12.5,
        "decoded_text": "..." },
      { "in_sentence": 9, "word_index": 220, "start": 125.34, "end": 125.79,
        "decoded_text": "about" }
    ],
    "suspect": [
      { "sentence": 6, "word_index": 139, "word": "in", "reason": "Replaced",
        "heard": "and", "start": 80.265, "end": 80.385 }
    ],
    "missing": [ { "sentence": 2, "word_index": 52, "word": "with" } ],
    "filtered": []
  }
}
```

- `report` is for review, and a player ignores it. It has the same lists,
  field names and reason names as forced-alignment's `AlignReport`, so one
  reader handles reports from either producer.
  - `word_index` counts the producer's text words: every sentence's words
    in order.
  - `insertions`: speech that isn't in the text. Each has exactly one of
    `before_sentence` (equal to the sentence count after the last
    sentence) and `in_sentence`. `decoded_text` is the speech as heard.
  - `suspect`: words timed but worth checking. `reason` is an aligner's
    (`LowScore`, `Truncated`, `AnomalousDuration`) or the mapper's
    (`Replaced`, `BoundaryUncertain`, with the audio's words in `heard`).
  - `missing`: text words with no audio.
  - `filtered`: words an aligner dropped before aligning. Empty for a
    transcript.
  - `score` and `threshold` appear only when an aligner gives them.

The `speakable` CLI writes timings and demo pages from the command line.
