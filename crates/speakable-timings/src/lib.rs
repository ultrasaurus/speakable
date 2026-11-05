//! Sentence timings: a start and end time for each sentence of a markdown
//! document, from one recording of it.
//!
//! - `Timings` is the JSON format (version 0.1).
//! - `map_words` maps timed words onto sentences, for any producer.
//! - `DescriptTranscript` reads a Descript word-level transcript export.
//! - `render_demo` writes a page that highlights each sentence as it plays.

mod demo;
mod descript;
mod format;
mod map;

pub use demo::{render_demo, title_of};
pub use descript::{DescriptSegment, DescriptTranscript};
pub use format::{
    Error, FilteredWord, Insertion, InsertionAt, MissingWord, Report, SentenceTiming, Stale, SuspectReason,
    SuspectWord, Timings, WordTiming, FORMAT_VERSION,
};
pub use map::{compare_key, map_words, words, SentenceMap, TimedWord};

/// The document's sentences, as their plain text, in order. These are the
/// sentences `speakable_markdown::to_html_with_sentence_spans` puts in
/// spans, so a sentence's index here is its `data-sentence` there.
pub fn document_sentences(markdown: &str) -> Vec<String> {
    speakable_markdown::split_for_export(markdown).0.into_iter().map(|s| s.text).collect()
}

/// Timings for `markdown` from timed words of the recording at `audio`
/// (stored as given: a path relative to where the JSON will be written).
///
/// The words compared are each sentence's plain text: silent text left
/// out, math spoken, and not normalized. That suits a transcript, which
/// writes what was heard as ordinary words.
///
/// Each sentence lists its words with their times; clear `words` to leave
/// them out of the JSON.
pub fn timings_from_words(markdown: &str, timed: &[TimedWord], audio: &str) -> Timings {
    let texts = document_sentences(markdown);
    let sentence_words: Vec<Vec<String>> = texts
        .iter()
        .map(|t| words(t).into_iter().map(String::from).collect())
        .collect();
    let mut map = map_words(&sentence_words, timed);
    // Times are written to the millisecond: transcript times carry
    // floating-point noise (37.035000000000004), and a finer time means
    // nothing to a player.
    let ms = |t: f64| (t * 1000.0).round() / 1000.0;
    for i in &mut map.report.insertions {
        (i.start, i.end) = (ms(i.start), ms(i.end));
    }
    for w in &mut map.report.suspect {
        (w.start, w.end) = (ms(w.start), ms(w.end));
    }
    Timings {
        version: FORMAT_VERSION.to_string(),
        audio: audio.to_string(),
        sentences: texts
            .into_iter()
            .zip(map.times.iter().zip(&sentence_words).zip(&map.word_times))
            .enumerate()
            .map(|(index, (text, ((t, words), word_times)))| SentenceTiming {
                index,
                text,
                start: t.map(|t| ms(t.0)),
                end: t.map(|t| ms(t.1)),
                words: words
                    .iter()
                    .zip(word_times)
                    .map(|(w, t)| WordTiming { text: w.clone(), start: t.map(|t| ms(t.0)), end: t.map(|t| ms(t.1)) })
                    .collect(),
            })
            .collect(),
        report: map.report,
    }
}

/// Timings for `markdown` from a Descript transcript of the recording at
/// `audio`.
pub fn timings_from_descript(markdown: &str, transcript: &DescriptTranscript, audio: &str) -> Timings {
    timings_from_words(markdown, &transcript.timed_words(), audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../samples/transparency-wins");

    fn read_sample() -> Option<(String, DescriptTranscript)> {
        let md = std::fs::read_to_string(format!("{SAMPLE}/talk.md")).ok()?;
        let json = std::fs::read_to_string(format!("{SAMPLE}/talk.descript.json")).ok()?;
        Some((md, DescriptTranscript::from_json(&json).expect("the sample transcript parses")))
    }

    #[test]
    fn document_sentences_are_the_spoken_split() {
        let md = "# [Title]<!--silent-->\n\nOne here. Two here.\n\n- Item.\n\n1. Numbered *item. Two* parts.";
        let spoken: Vec<String> = speakable_splitter::split(&speakable_markdown::to_plain_text(md))
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(document_sentences(md), spoken);
    }

    #[test]
    fn json_round_trips_and_omits_missing_times() {
        let t = Timings {
            version: FORMAT_VERSION.into(),
            audio: "doc.mp3".into(),
            sentences: vec![
                SentenceTiming {
                    index: 0,
                    text: "One.".into(),
                    start: Some(0.42),
                    end: Some(3.1),
                    words: vec![WordTiming { text: "One.".into(), start: Some(0.42), end: Some(3.1) }],
                },
                SentenceTiming {
                    index: 1,
                    text: "Two & three.".into(),
                    start: None,
                    end: None,
                    words: vec![
                        WordTiming { text: "Two".into(), start: None, end: None },
                        WordTiming { text: "&".into(), start: None, end: None },
                        WordTiming { text: "three.".into(), start: None, end: None },
                    ],
                },
            ],
            report: Report {
                insertions: vec![
                    Insertion {
                        at: InsertionAt::BeforeSentence(0),
                        word_index: 0,
                        start: 0.0,
                        end: 0.4,
                        decoded_text: "hello".into(),
                        score: None,
                    },
                    Insertion {
                        at: InsertionAt::InSentence(1),
                        word_index: 3,
                        start: 5.0,
                        end: 5.2,
                        decoded_text: "uh".into(),
                        score: Some(0.4),
                    },
                ],
                suspect: vec![SuspectWord {
                    sentence: 0,
                    word_index: 0,
                    word: "One.".into(),
                    reason: SuspectReason::Replaced,
                    heard: Some("Won".into()),
                    score: None,
                    start: 0.42,
                    end: 3.1,
                }],
                missing: vec![MissingWord { sentence: 1, word_index: 1, word: "Two.".into() }],
                filtered: vec![],
                threshold: None,
            },
        };
        let json = t.to_json();
        assert!(!json.contains("null"), "{json}");
        assert!(!json.contains("threshold"), "{json}");
        // Each insertion has exactly one of the two location fields.
        assert!(json.contains("\"before_sentence\": 0"), "{json}");
        assert!(json.contains("\"in_sentence\": 1"), "{json}");
        assert!(json.contains("\"reason\": \"Replaced\""), "{json}");
        // A word with no times is listed, with only its text.
        assert!(json.contains("{\n          \"text\": \"&\"\n        }"), "{json}");
        assert_eq!(Timings::from_json(&json).unwrap(), t);
        assert!(matches!(
            Timings::from_json(&json.replace("\"version\": \"0.1\"", "\"version\": \"0.2\"")),
            Err(Error::UnsupportedVersion(v)) if v == "0.2"
        ));
    }

    #[test]
    fn an_empty_report_is_left_out() {
        let mut t = timings_from_words("One here.", &[TimedWord::new("one", 0.0, 0.2), TimedWord::new("here", 0.2, 0.5)], "a.mp3");
        assert!(t.report.is_empty());
        assert_eq!(
            t.sentences[0].words,
            vec![
                WordTiming { text: "One".into(), start: Some(0.0), end: Some(0.2) },
                WordTiming { text: "here.".into(), start: Some(0.2), end: Some(0.5) },
            ]
        );
        t.sentences[0].words.clear();
        let json = t.to_json();
        assert!(!json.contains("report"), "{json}");
        assert_eq!(Timings::from_json(&json).unwrap(), t);
    }

    #[test]
    fn an_insertion_needs_exactly_one_location() {
        let with = |fields: &str| {
            format!(
                r#"{{"version": "0.1", "audio": "a.mp3", "sentences": [],
                    "report": {{"insertions": [{{{fields} "word_index": 0, "start": 0, "end": 1, "decoded_text": "x"}}]}}}}"#
            )
        };
        assert!(Timings::from_json(&with(r#""before_sentence": 0,"#)).is_ok());
        assert!(Timings::from_json(&with(r#""in_sentence": 0,"#)).is_ok());
        assert!(Timings::from_json(&with("")).is_err());
        assert!(Timings::from_json(&with(r#""before_sentence": 0, "in_sentence": 0,"#)).is_err());
    }

    #[test]
    fn check_catches_an_edited_document() {
        let md = "One here. Two here.";
        let t = timings_from_words(md, &[], "a.mp3");
        assert!(t.check(md).is_ok());
        assert!(matches!(t.check("One here. Two there."), Err(Error::Stale(Stale { first_difference: Some(1), .. }))));
        assert!(matches!(t.check("One here."), Err(Error::Stale(_))));
    }

    #[test]
    fn reads_the_sample_descript_transcript() {
        let Some((md, transcript)) = read_sample() else { return };
        let words = transcript.timed_words();
        assert_eq!(words.len(), 291);
        assert_eq!(words[0], TimedWord::new("Often", 0.27, 0.63));

        let t = timings_from_descript(&md, &transcript, "talk.mp3");
        assert!(t.check(&md).is_ok());
        assert_eq!(t.sentences.len(), document_sentences(&md).len());
        // Every sentence of the excerpt is spoken in the recording.
        assert!(t.sentences.iter().all(|s| s.times().is_some()), "{}", t.to_json());
        // The first sentence starts with the first word.
        assert_eq!(t.sentences[0].start, Some(0.27));
        // Sentences are in order and don't overlap.
        for pair in t.sentences.windows(2) {
            let (a, b) = (pair[0].times().unwrap(), pair[1].times().unwrap());
            assert!(a.0 < a.1 && a.1 <= b.0, "{pair:?}");
        }

        // The places where the recording and the text differ.
        let r = &t.report;
        let missing: Vec<(usize, &str)> = r.missing.iter().map(|w| (w.sentence, w.word.as_str())).collect();
        assert_eq!(missing, vec![(2, "with")]);
        let inserted: Vec<(InsertionAt, &str)> = r.insertions.iter().map(|i| (i.at, i.decoded_text.as_str())).collect();
        assert_eq!(inserted, vec![(InsertionAt::InSentence(2), "C"), (InsertionAt::InSentence(9), "about")]);
        let replaced: Vec<(usize, &str, SuspectReason, Option<&str>)> =
            r.suspect.iter().map(|w| (w.sentence, w.word.as_str(), w.reason, w.heard.as_deref())).collect();
        assert_eq!(
            replaced,
            vec![
                (6, "in", SuspectReason::Replaced, Some("and")),
                (7, "have", SuspectReason::Replaced, Some("who've")),
            ]
        );
        assert!(r.filtered.is_empty() && r.threshold.is_none());
    }
}
