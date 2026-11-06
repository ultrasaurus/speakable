# Samples

Recordings with their text, for learning and trying speakable: each
folder is one recording of a markdown document being read aloud.

## Files

Each sample folder holds files that share one base name (`talk`,
`section0`):

- `<name>.md`: the text, as it would be published.
- `<name>.mp3`: one recording of it being read.
- `<name>.descript.json`, in some samples: Descript's word-level
  transcript export of the recording, unedited. A sample without one gets
  its timings from forced alignment (`speakable align`).
- `README.md`: where the text and recording come from, and their license.

The text is what would be published, not a transcript of the recording,
so the audio can differ from it where the reader adds, skips or changes
words. A Descript transcript adds differences of its own where it
mishears the speaker.

## Samples

- [transparency-wins](transparency-wins/): an excerpt of "Transparency
  Wins", a talk by Sarah Allen at RubyConf Taiwan, 2015.
- [freedmens-book](freedmens-book/): the preface of *The Freedmen's
  Book* by Lydia Maria Child (1865), from a LibriVox recording.

## Alignment against Descript

Both samples have a Descript export, so `speakable align`'s timings can be
checked against `speakable descript`'s:

- `transparency-wins`: every sentence within 0.35s. The aligner's ends
  are a little earlier, since Descript pads its word ends. With
  `--words`, word starts are within 0.085s at the median and 0.3s at most
  (289 words).
- `freedmens-book`: every sentence within 0.45s, except three.
  - Sentences 0 and 3: Descript matched words in the LibriVox preamble,
    and the aligner is right.
  - Sentence 2 ("TO"): the reader says "To the Freedmen" before the
    text's "TO / THE LOYAL AND BRAVE", so both put the sentence on the
    inserted "to" (17.6s), not its own (19.7s). Both placements leave the
    extra speech between sentences, and nothing in the audio or the text
    tells them apart.
  - The preamble and outro are reported before the first sentence and
    after the last.

## License

The samples are not covered by the repository's MIT/Apache license. Each
sample's license is in its own README.
