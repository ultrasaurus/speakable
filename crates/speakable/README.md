# speakable (CLI)

Command-line tools for sentence timings, built on `speakable-timings` and
`speakable-align`. Kept in its own crate so the libraries don't depend on
`clap` and other crates they don't need.

## Why

To try sentence highlighting on a real recording without writing code:
get timings from a transcript or from the audio itself, then open a
page that plays them.

## Commands

```sh
speakable demo <doc> --audio <audio.mp3> -o <doc.html>
speakable demo <doc> <doc.timings.json> -o <doc.html>
speakable align <audio.mp3> <doc> -o <doc.timings.json> [--report] [--words]
speakable descript <doc.md> <transcript.json> --audio <audio.mp3> -o <doc.timings.json> [--report] [--words]
```

- `descript` maps a Descript word-level transcript export onto the
  document's sentences and writes the timings JSON.
- `align` aligns the audio file against the document's text (forced
  alignment) and writes the timings JSON.
  - `<doc>` is markdown (`.md`, `.markdown`) or plain text (anything
    else). Plain text is read one paragraph per line, so a hard-wrapped
    file splits sentences at its line ends.
  - Each sentence is normalized to its spoken form ("1945" → "nineteen
    forty five") before aligning, and the times are mapped back to the
    text's words. `<doc>.overrides.txt` beside the document, when present,
    adds per-document normalizer overrides.
  - The first run downloads the wav2vec2 model (~360MB) from Hugging Face.
  - It aligns the whole audio file at once. Memory grows with the square of
    its length: up to about 35 minutes fits in under 2 GB, and 60 minutes
    needs about 7 GB.
- For both:
  - The JSON names the audio by its path relative to the JSON file.
  - They print how many sentences got times, and how many insertions,
    suspect, missing and filtered words the report has.
  - `--report` also writes those into the JSON's `report`, for review;
    without it the JSON has only the sentence times.
  - `--words` also lists each sentence's words with their times. A word
    with nothing said for it is listed without times.
- `demo` writes one self-contained HTML page that highlights each
  sentence as the audio plays.
  - With a timings file, it refuses timings that don't match the document.
  - With `--audio`, it aligns the audio first, as `align` does.
  - It refers to the audio by relative path, so keep the page and the
    audio where they are.

## Features

- `align` (default): the `align` command and `demo --audio`. It brings in
  the model runtime; build with `--no-default-features` to leave it out.
- `metal`, `cuda`: run the model on a GPU. Set
  `FORCED_ALIGNMENT_DEVICE=cpu|metal|cuda` to choose at run time.

## Try it

On the sample in this repo:

```sh
cd samples/transparency-wins
cargo run --release -p speakable -- demo talk.md --audio talk.mp3 -o talk.html
```

Then open `talk.html` in a browser. 

Or from the Descript transcript:

```sh
cargo run -p speakable -- descript talk.md talk.descript.json --audio talk.mp3 -o talk.timings.json
cargo run -p speakable -- demo talk.md talk.timings.json -o talk.html
```
