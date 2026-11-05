# speakable-align

Sentence and word timings for a markdown document from one audio file of
it, by forced alignment ([forced-alignment-rs]). The `speakable` CLI's
`align` command and `demo --audio` use it.

[forced-alignment-rs]: https://github.com/ultrasaurus/forced-alignment-rs

## How it works

1. Split the document into sentences (`speakable_timings::document_sentences`).
2. Normalize each sentence to its spoken words ("1945" → "nineteen forty
   five"), keeping which text word each spoken word was said for.
3. Align the whole audio against all the spoken words at once, telling
   the aligner where each sentence starts. Speech that isn't in the text
   then lands between sentences where it can, rather than stretching a
   word.
4. Map the aligned words back: a sentence runs from its first word's start
   to its last word's end, and each text word takes the span of the spoken
   words said for it. A text word with no spoken words (a symbol the
   normalizer drops) is listed without times.

The result is `speakable_timings::Timings`, including the aligner's report
(speech not in the text, suspect words, words with nothing to align).

## Use

```rust
use speakable_normalizer::Normalizer;

let timings = speakable_align::align_audio(
    &markdown,
    &Normalizer::builtin(),
    Path::new("talk.mp3"),
    "talk.mp3", // the audio's path relative to where the JSON goes
)?;
```

The first run downloads the wav2vec2 model from Hugging Face. Enable the
`metal` or `cuda` feature to run the model on a GPU.

The aligner keeps tables that grow with the square of the audio's
length: up to about 7–8 minutes aligns in under 2 GB. For longer
audio, align in pieces (see forced-alignment-rs's overview).
