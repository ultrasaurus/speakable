//! Sentence timings for a markdown document from one audio file of it, by
//! forced alignment.
//!
//! - `spoken_sentences` gives each sentence's plain text and its spoken
//!   words: normalized ("1945" → "nineteen forty five"), so they match what
//!   the aligner hears, each tied back to the text words it was said for.
//! - `align_document` aligns the audio against those words.
//! - `timings_from_alignment` turns the aligner's words and report into
//!   `speakable_timings::Timings`. It needs no model, so it's tested on its
//!   own.
//! - `align_audio` does all three for an audio file.

use anyhow::{Context, Result};
use forced_alignment::transcript::{self as fa, AlignReport, Transcript};
use forced_alignment::AlignOptions;
use std::ops::Range;
use std::path::Path;

use speakable_normalizer::{NormalizedText, Normalizer};
use speakable_timings::{
    document_sentences, words as text_words, FilteredWord, Insertion, InsertionAt, Report, SentenceTiming,
    SuspectReason, SuspectWord, Timings, WordTiming, FORMAT_VERSION,
};

/// One sentence of the document: its plain text, as `Timings` records it,
/// and the words the aligner is given for it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenSentence {
    pub text: String,
    /// The text's words, as `speakable_timings::words` splits them: the
    /// words a sentence's `words` lists.
    pub text_words: Vec<String>,
    /// The spoken words, split on whitespace as the aligner splits its text.
    pub words: Vec<String>,
    /// Per spoken word, the range of `text_words` it was said for: one word
    /// for ordinary prose, a whole number for "nineteen", and possibly none
    /// for a word the normalizer added.
    pub sources: Vec<Range<usize>>,
}

/// Timings for `markdown` from the audio file at `audio_path` (mp3 or wav),
/// with every sentence's words and the full report. `audio_ref` is the path
/// the timings store for that file: relative to where the timings JSON goes.
pub fn align_audio(markdown: &str, normalizer: &Normalizer, audio_path: &Path, audio_ref: &str) -> Result<Timings> {
    let sentences = spoken_sentences(markdown, normalizer);
    let samples = forced_alignment::audio::load_audio(audio_path, forced_alignment::SAMPLE_RATE)
        .with_context(|| audio_path.display().to_string())?;
    let (transcript, report) = align_document(&samples, &sentences)?;
    Ok(timings_from_alignment(&sentences, &transcript, &report, audio_ref))
}

/// The document's sentences, each normalized to its spoken words.
pub fn spoken_sentences(markdown: &str, normalizer: &Normalizer) -> Vec<SpokenSentence> {
    document_sentences(markdown).into_iter().map(|text| spoken_sentence(text, normalizer)).collect()
}

fn spoken_sentence(text: String, normalizer: &Normalizer) -> SpokenSentence {
    let normalized = normalizer.normalize_with_spans(&text);
    let source = source_chars(&normalized);
    // Each text word's char range in `text`.
    let text_ranges: Vec<Range<usize>> = text_words(&text).into_iter().map(|w| char_range(&text, w)).collect();
    let spoken: Vec<&str> = normalized.text.split_whitespace().collect();
    let sources = spoken
        .iter()
        .map(|w| {
            let r = char_range(&normalized.text, w);
            let from = source[r].iter().fold(None::<Range<usize>>, |acc, c| {
                Some(acc.map_or(c.clone(), |a| a.start.min(c.start)..a.end.max(c.end)))
            });
            let overlapping: Vec<usize> = match from {
                None => vec![],
                Some(from) => {
                    (0..text_ranges.len()).filter(|&i| text_ranges[i].start < from.end && from.start < text_ranges[i].end).collect()
                }
            };
            match (overlapping.first(), overlapping.last()) {
                (Some(&a), Some(&b)) => a..b + 1,
                _ => 0..0,
            }
        })
        .collect();
    SpokenSentence {
        text_words: text_ranges.iter().map(|r| text.chars().skip(r.start).take(r.len()).collect()).collect(),
        words: spoken.into_iter().map(String::from).collect(),
        sources,
        text,
    }
}

/// For each char of the normalized text, the source chars it came from.
/// A raw chunk (untouched prose) keeps the source's chars one for one, so
/// its chars map exactly; a transformed chunk ("1945" → "nineteen forty
/// five") maps every char to the chunk's whole source.
fn source_chars(normalized: &NormalizedText) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut pos = 0;
    for (chunk, raw) in normalized.chunks() {
        let len = chunk.chars().count();
        if len == 0 {
            continue;
        }
        let src = normalized.source_range(pos..pos + 1).unwrap_or(0..0);
        for k in 0..len {
            out.push(if raw { src.start + k..src.start + k + 1 } else { src.clone() });
        }
        pos += len;
    }
    out
}

/// The char range of `part`, a slice of `whole`.
fn char_range(whole: &str, part: &str) -> Range<usize> {
    let byte_start = part.as_ptr() as usize - whole.as_ptr() as usize;
    let start = whole[..byte_start].chars().count();
    start..start + part.chars().count()
}

/// Align `samples` (mono, at `forced_alignment::SAMPLE_RATE`) against every
/// sentence's words at once, telling the aligner where sentences start so
/// it places speech that isn't in the text between sentences when it can.
pub fn align_document(samples: &[f32], sentences: &[SpokenSentence]) -> Result<(Transcript, AlignReport)> {
    let text = sentences.iter().flat_map(|s| &s.words).map(String::as_str).collect::<Vec<_>>().join(" ");
    let options = AlignOptions { sentence_starts: Some(sentence_starts(sentences)) };
    forced_alignment::align_with(samples, &text, &options)
}

/// The index of each sentence's first word among all sentences' words. A
/// sentence with no words starts where the next one does.
fn sentence_starts(sentences: &[SpokenSentence]) -> Vec<usize> {
    sentences
        .iter()
        .scan(0, |next, s| {
            let start = *next;
            *next += s.words.len();
            Some(start)
        })
        .collect()
}

/// Timings from the aligner's output for `sentences`, for the audio file at
/// `audio` (stored as given: a path relative to where the JSON goes).
///
/// The aligner times every word it keeps, in order, so its words line up
/// one to one with the sentences' words minus the ones it filtered out; no
/// diff is needed, and nothing is reported missing. A sentence runs from
/// its first word's start to its last word's end.
///
/// Each sentence lists its text words; a word takes the span of the spoken
/// words said for it, and has no times when none were. Clear `words` to
/// leave them out of the JSON.
///
/// The report carries the aligner's over, with `word_index` counting every
/// sentence word (filtered ones included, unlike the aligner's own word
/// indexes) and each entry placed in its sentence.
pub fn timings_from_alignment(
    sentences: &[SpokenSentence],
    transcript: &Transcript,
    report: &AlignReport,
    audio: &str,
) -> Timings {
    // Every spoken word as (sentence, word, its index in the sentence): its
    // position here is its `word_index`, and matches the aligner's
    // `original_index`.
    let all: Vec<(usize, &str, usize)> = sentences
        .iter()
        .enumerate()
        .flat_map(|(s, sent)| sent.words.iter().enumerate().map(move |(k, w)| (s, w.as_str(), k)))
        .collect();
    let filtered: Vec<usize> = report.filtered.iter().map(|f| f.original_index).collect();
    // `kept[k]` is the `word_index` of the aligner's word `k`.
    let kept: Vec<usize> = (0..all.len()).filter(|i| !filtered.contains(i)).collect();
    let aligned: Vec<&fa::Word> = transcript.segments.iter().flat_map(|s| &s.words).collect();
    debug_assert_eq!(kept.len(), aligned.len(), "the aligner times every word it keeps");

    let ms = |t: f64| (t * 1000.0).round() / 1000.0;

    let widen = |slot: &mut Option<(f64, f64)>, start: f64, end: f64| {
        *slot = Some(match *slot {
            None => (start, end),
            Some((s, e)) => (s.min(start), e.max(end)),
        });
    };
    let mut times: Vec<Option<(f64, f64)>> = vec![None; sentences.len()];
    let mut word_times: Vec<Vec<Option<(f64, f64)>>> =
        sentences.iter().map(|s| vec![None; s.text_words.len()]).collect();
    for (&i, w) in kept.iter().zip(&aligned) {
        let (Some(start), Some(end)) = (w.start, w.end) else { continue };
        let (sentence, spoken) = (all[i].0, all[i].2);
        widen(&mut times[sentence], start, end);
        for t in sentences[sentence].sources[spoken].clone() {
            widen(&mut word_times[sentence][t], start, end);
        }
    }

    // Where speech before the aligner's word `k` falls among the sentences.
    let insertion_at = |k: usize| -> (InsertionAt, usize) {
        let word_index = kept.get(k).copied().unwrap_or(all.len());
        let prev = k.checked_sub(1).map(|p| all[kept[p]].0);
        let next = kept.get(k).map(|&i| all[i].0);
        let at = match (prev, next) {
            (Some(p), Some(n)) if p == n => InsertionAt::InSentence(p),
            (Some(p), _) => InsertionAt::BeforeSentence(p + 1),
            (None, _) => InsertionAt::BeforeSentence(0),
        };
        (at, word_index)
    };

    let out = Report {
        insertions: report
            .insertions
            .iter()
            .map(|ins| {
                let (at, word_index) = insertion_at(ins.before_word_index);
                Insertion {
                    at,
                    word_index,
                    start: ms(ins.start),
                    end: ms(ins.end),
                    decoded_text: ins.decoded_text.clone(),
                    score: Some(ins.score),
                }
            })
            .collect(),
        suspect: report
            .suspect
            .iter()
            .map(|w| {
                let i = kept[w.word_index];
                let a = aligned[w.word_index];
                SuspectWord {
                    sentence: all[i].0,
                    word_index: i,
                    word: w.word.clone(),
                    reason: match w.reason {
                        fa::SuspectReason::LowScore => SuspectReason::LowScore,
                        fa::SuspectReason::Truncated => SuspectReason::Truncated,
                        fa::SuspectReason::AnomalousDuration => SuspectReason::AnomalousDuration,
                    },
                    heard: None,
                    score: Some(w.score),
                    start: ms(a.start.unwrap_or_default()),
                    end: ms(a.end.unwrap_or_default()),
                }
            })
            .collect(),
        missing: vec![],
        filtered: report
            .filtered
            .iter()
            .map(|f| FilteredWord {
                sentence: all[f.original_index].0,
                word_index: f.original_index,
                word: f.word.clone(),
            })
            .collect(),
        threshold: Some(report.threshold),
    };

    Timings {
        version: FORMAT_VERSION.to_string(),
        audio: audio.to_string(),
        sentences: sentences
            .iter()
            .zip(times.iter().zip(&word_times))
            .enumerate()
            .map(|(index, (s, (t, wt)))| SentenceTiming {
                index,
                text: s.text.clone(),
                start: t.map(|t| ms(t.0)),
                end: t.map(|t| ms(t.1)),
                words: s
                    .text_words
                    .iter()
                    .zip(wt)
                    .map(|(w, t)| WordTiming { text: w.clone(), start: t.map(|t| ms(t.0)), end: t.map(|t| ms(t.1)) })
                    .collect(),
            })
            .collect(),
        report: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forced_alignment::transcript::Segment;

    /// A sentence whose spoken words are said for its text words one to
    /// one, as far as both go.
    fn sentence(text: &str, words: &str) -> SpokenSentence {
        let text_words: Vec<String> = text_words(text).into_iter().map(String::from).collect();
        let words: Vec<String> = words.split_whitespace().map(String::from).collect();
        let sources = (0..words.len()).map(|i| if i < text_words.len() { i..i + 1 } else { 0..0 }).collect();
        SpokenSentence { text: text.into(), text_words, words, sources }
    }

    /// The aligner's words, each 1 second long, back to back from 0.
    fn transcript(words: &[&str]) -> Transcript {
        let words = words
            .iter()
            .enumerate()
            .map(|(i, w)| fa::Word {
                word: w.to_string(),
                start: Some(i as f64),
                end: Some(i as f64 + 1.0),
                score: Some(0.9),
                speaker: None,
            })
            .collect();
        Transcript {
            segments: vec![Segment { start: 0.0, end: 0.0, text: String::new(), words, speaker: None }],
            word_segments: None,
            language: "en".into(),
        }
    }

    fn report() -> AlignReport {
        AlignReport { filtered: vec![], suspect: vec![], insertions: vec![], threshold: 0.3 }
    }

    #[test]
    fn sentence_starts_count_words() {
        let s = [sentence("One two.", "one two."), sentence("—", ""), sentence("Three.", "three.")];
        assert_eq!(sentence_starts(&s), vec![0, 2, 2]);
    }

    #[test]
    fn spoken_sentences_are_normalized() {
        let s = spoken_sentences("It was 1945. Then more.", &Normalizer::from_map(Default::default()));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "It was 1945.");
        assert_eq!(s[0].words, vec!["It", "was", "nineteen", "forty", "five."]);
        assert_eq!(s[0].text_words, vec!["It", "was", "1945."]);
        assert_eq!(s[0].sources, vec![0..1, 1..2, 2..3, 2..3, 2..3]);
    }

    #[test]
    fn a_sentence_lists_every_text_word_timed_or_not() {
        // "1945" is said as three words and takes their span; "&" is said as
        // nothing here, so it's listed without times.
        let mut s = sentence("In 1945 & after.", "in nineteen forty five after.");
        s.sources = vec![0..1, 1..2, 1..2, 1..2, 3..4];
        let t = timings_from_alignment(
            &[s],
            &transcript(&["in", "nineteen", "forty", "five", "after."]),
            &report(),
            "a.mp3",
        );
        let words: Vec<(&str, Option<f64>, Option<f64>)> =
            t.sentences[0].words.iter().map(|w| (w.text.as_str(), w.start, w.end)).collect();
        assert_eq!(
            words,
            vec![("In", Some(0.0), Some(1.0)), ("1945", Some(1.0), Some(4.0)), ("&", None, None), ("after.", Some(4.0), Some(5.0))]
        );
    }

    #[test]
    fn prose_maps_word_for_word_through_the_normalizer() {
        let s = spoken_sentences("Often these days, we make things.", &Normalizer::from_map(Default::default()));
        assert_eq!(s[0].words, s[0].text_words);
        assert_eq!(s[0].sources, (0..6).map(|i| i..i + 1).collect::<Vec<_>>());
    }

    #[test]
    fn sentences_take_their_words_span() {
        let s = [sentence("One two.", "one two."), sentence("Three.", "three.")];
        let t = timings_from_alignment(&s, &transcript(&["one", "two.", "three."]), &report(), "a.mp3");
        let times: Vec<_> = t.sentences.iter().map(|s| s.times()).collect();
        assert_eq!(times, vec![Some((0.0, 2.0)), Some((2.0, 3.0))]);
        assert_eq!(t.sentences[0].text, "One two.");
        assert_eq!(t.report.threshold, Some(0.3));
        assert!(t.report.missing.is_empty());
    }

    #[test]
    fn filtered_words_shift_indexes_and_can_leave_a_sentence_untimed() {
        // "—" has nothing to align: the aligner drops it, so its words are
        // one fewer than the sentences', and the sentence of only "—" has
        // no times.
        let s = [sentence("One.", "one."), sentence("—", "—"), sentence("Two three.", "two three.")];
        let mut r = report();
        r.filtered = vec![fa::FilteredWord { word: "—".into(), original_index: 1 }];
        // The aligner's word 2 ("three.") is suspect, and speech before its
        // word 1 ("two") falls between sentences.
        r.suspect = vec![fa::SuspectWord {
            word_index: 2,
            word: "three.".into(),
            score: 0.1,
            reason: fa::SuspectReason::LowScore,
        }];
        r.insertions = vec![fa::Insertion {
            before_word_index: 1,
            start: 1.0,
            end: 1.5,
            decoded_text: "um".into(),
            score: 0.4,
        }];
        let t = timings_from_alignment(&s, &transcript(&["one.", "two", "three."]), &r, "a.mp3");
        let times: Vec<_> = t.sentences.iter().map(|s| s.times()).collect();
        assert_eq!(times, vec![Some((0.0, 1.0)), None, Some((1.0, 3.0))]);
        assert_eq!(t.report.filtered, vec![FilteredWord { sentence: 1, word_index: 1, word: "—".into() }]);
        assert_eq!(
            t.report.suspect,
            vec![SuspectWord {
                sentence: 2,
                word_index: 3,
                word: "three.".into(),
                reason: SuspectReason::LowScore,
                heard: None,
                score: Some(0.1),
                start: 2.0,
                end: 3.0,
            }]
        );
        assert_eq!(
            t.report.insertions,
            vec![Insertion {
                at: InsertionAt::BeforeSentence(1),
                word_index: 2,
                start: 1.0,
                end: 1.5,
                decoded_text: "um".into(),
                score: Some(0.4),
            }]
        );
    }

    #[test]
    fn insertions_inside_before_and_after() {
        let s = [sentence("One two.", "one two."), sentence("Three.", "three.")];
        let ins = |before_word_index: usize| fa::Insertion {
            before_word_index,
            start: 0.0,
            end: 0.5,
            decoded_text: "x".into(),
            score: 0.5,
        };
        let mut r = report();
        r.insertions = vec![ins(0), ins(1), ins(3)];
        let t = timings_from_alignment(&s, &transcript(&["one", "two.", "three."]), &r, "a.mp3");
        let at: Vec<(InsertionAt, usize)> = t.report.insertions.iter().map(|i| (i.at, i.word_index)).collect();
        assert_eq!(
            at,
            vec![
                (InsertionAt::BeforeSentence(0), 0),
                (InsertionAt::InSentence(0), 1),
                (InsertionAt::BeforeSentence(2), 3),
            ]
        );
    }
}
