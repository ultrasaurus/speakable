# speakable (CLI)

Command-line tools for sentence timings, built on `speakable-timings`.
Kept in its own crate so the libraries don't depend on `clap`.

## Why

To try sentence highlighting on a real recording without writing code:
turn a transcript into timings, then open a page that plays them.

## Commands

```sh
speakable descript <doc.md> <transcript.json> --audio <audio.mp3> -o <doc.timings.json> [--report] [--words]
speakable demo <doc.md> <doc.timings.json> -o <doc.html>
```

- `descript` maps a Descript word-level transcript export onto the
  document's sentences and writes the timings JSON. The JSON names the
  audio by its path relative to the JSON file. It prints how many
  sentences got times, and how many insertions, suspect words and missing
  words it found. `--report` also writes those into the JSON's `report`,
  for review; without it the JSON has only the sentence times. `--words`
  also lists each sentence's words with their times.
- `demo` writes one self-contained HTML page that highlights each
  sentence as the audio plays. It refers to the audio by relative path,
  so keep the page and the audio where they are. It refuses timings that
  don't match the document.

Try it on the sample in this repo:

```sh
cd samples/transparency-wins
cargo run -p speakable -- descript talk.md talk.descript.json --audio talk.mp3 -o talk.timings.json
cargo run -p speakable -- demo talk.md talk.timings.json -o talk.html
```

Then open `talk.html` in a browser.
