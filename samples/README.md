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
  its timings from forced alignment (`audio-timings`, in
  forced-alignment-rs).
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

## License

The samples are not covered by the repository's MIT/Apache license. Each
sample's license is in its own README.
