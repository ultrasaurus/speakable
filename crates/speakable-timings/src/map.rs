//! Map timed words onto a document's sentences.
//!
//! The same way for any producer of timed words (a transcript, a forced
//! aligner): a word-level diff lines up the sentences' words with the timed
//! words, and each sentence takes the times of the timed words that line up
//! with its own.

use similar::{capture_diff_slices, Algorithm, DiffOp};

use crate::format::{Insertion, InsertionAt, MissingWord, Report, SuspectReason, SuspectWord};

/// One word of speech with its time in the audio, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

impl TimedWord {
    pub fn new(text: impl Into<String>, start: f64, end: f64) -> Self {
        TimedWord { text: text.into(), start, end }
    }
}

/// What `map_words` found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SentenceMap {
    /// Per sentence: the start of its first timed word and the end of its
    /// last, or `None` when no timed word lined up with it.
    pub times: Vec<Option<(f64, f64)>>,
    /// Where the text and the timed words differ. `filtered` is always
    /// empty and `threshold` absent: those come from an aligner.
    pub report: Report,
}

/// Split text into words: on whitespace, and on em and en dashes, which
/// join words without a space ("this—that") but are never part of one.
/// Hyphens stay inside the word.
pub fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| c.is_whitespace() || c == '—' || c == '–')
        .filter(|w| !w.is_empty())
        .collect()
}

/// The form two words are compared in: lowercase, letters and digits only.
/// Unicode-aware, so "Café," and "café" are the same word. Empty for a word
/// with no letters or digits ("&", "—").
pub fn compare_key(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A text word that takes part in the diff.
struct TextWord<'a> {
    key: String,
    text: &'a str,
    sentence: usize,
    /// Position among all of `sentence_words`' words, counting the ones
    /// left out of the diff: the report's `word_index`.
    word_index: usize,
}

/// Map `timed` words onto sentences.
///
/// `sentence_words[i]` holds sentence `i`'s words in order (see `words`);
/// `timed` is every timed word in the audio, in order. Words with no
/// letters or digits are left out of the diff on both sides.
///
/// After a word-level diff of the two:
/// - A matched word gives its times to its text word.
/// - A run of text words replaced by timed words ("would have" heard as
///   "would've", a misheard name) takes the span of those timed words, and
///   each of its words is reported `Replaced`. When the run crosses a
///   sentence boundary, nothing says where in the span the boundary falls:
///   the span is shared among the run's words in proportion to their
///   count, and each of its words is reported `BoundaryUncertain` instead.
/// - Timed words that match no text word are reported as an insertion,
///   `in_sentence` when they fall between two words of one sentence and
///   `before_sentence` otherwise.
/// - Text words with no timed words get no times and are reported
///   missing. A sentence whose words all have none gets no times.
pub fn map_words(sentence_words: &[Vec<String>], timed: &[TimedWord]) -> SentenceMap {
    let mut text: Vec<TextWord> = Vec::new();
    let mut total_words = 0usize;
    for (sentence, words) in sentence_words.iter().enumerate() {
        for word in words {
            let key = compare_key(word);
            if !key.is_empty() {
                text.push(TextWord { key, text: word, sentence, word_index: total_words });
            }
            total_words += 1;
        }
    }
    let timed: Vec<&TimedWord> = timed.iter().filter(|w| !compare_key(&w.text).is_empty()).collect();
    let text_keys: Vec<&str> = text.iter().map(|w| w.key.as_str()).collect();
    let timed_keys: Vec<String> = timed.iter().map(|w| compare_key(&w.text)).collect();
    let timed_keys: Vec<&str> = timed_keys.iter().map(String::as_str).collect();

    let mut word_times: Vec<Option<(f64, f64)>> = vec![None; text.len()];
    let mut report = Report::default();

    // Where speech before text word `pos` falls among the sentences.
    let insertion_at = |pos: usize| -> (InsertionAt, usize) {
        let word_index = text.get(pos).map_or(total_words, |w| w.word_index);
        let prev = pos.checked_sub(1).map(|p| text[p].sentence);
        let next = text.get(pos).map(|w| w.sentence);
        let at = match (prev, next) {
            (Some(p), Some(n)) if p == n => InsertionAt::InSentence(p),
            (Some(p), _) => InsertionAt::BeforeSentence(p + 1),
            (None, _) => InsertionAt::BeforeSentence(0),
        };
        (at, word_index)
    };

    for op in capture_diff_slices(Algorithm::Myers, &text_keys, &timed_keys) {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    let w = timed[new_index + k];
                    word_times[old_index + k] = Some((w.start, w.end));
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for w in &text[old_index..old_index + old_len] {
                    report.missing.push(MissingWord {
                        sentence: w.sentence,
                        word_index: w.word_index,
                        word: w.text.to_string(),
                    });
                }
            }
            DiffOp::Insert { old_index, new_index, new_len } => {
                let heard = &timed[new_index..new_index + new_len];
                let (at, word_index) = insertion_at(old_index);
                report.insertions.push(Insertion {
                    at,
                    word_index,
                    start: heard[0].start,
                    end: heard[new_len - 1].end,
                    decoded_text: joined(heard),
                    score: None,
                });
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let heard = &timed[new_index..new_index + new_len];
                let (start, end) = (heard[0].start, heard[new_len - 1].end);
                let run = &text[old_index..old_index + old_len];
                let crosses = run.first().map(|w| w.sentence) != run.last().map(|w| w.sentence);
                let step = (end - start) / old_len as f64;
                for (k, w) in run.iter().enumerate() {
                    let t = if crosses {
                        let s = start + step * k as f64;
                        (s, s + step)
                    } else {
                        (start, end)
                    };
                    word_times[old_index + k] = Some(t);
                    report.suspect.push(SuspectWord {
                        sentence: w.sentence,
                        word_index: w.word_index,
                        word: w.text.to_string(),
                        reason: if crosses { SuspectReason::BoundaryUncertain } else { SuspectReason::Replaced },
                        heard: Some(joined(heard)),
                        score: None,
                        start: t.0,
                        end: t.1,
                    });
                }
            }
        }
    }

    let mut times: Vec<Option<(f64, f64)>> = vec![None; sentence_words.len()];
    for (w, t) in text.iter().zip(&word_times) {
        let Some((start, end)) = *t else { continue };
        let slot = &mut times[w.sentence];
        *slot = Some(match *slot {
            None => (start, end),
            Some((s, e)) => (s.min(start), e.max(end)),
        });
    }

    SentenceMap { times, report }
}

fn joined(words: &[&TimedWord]) -> String {
    words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentences(texts: &[&str]) -> Vec<Vec<String>> {
        texts.iter().map(|t| words(t).into_iter().map(String::from).collect()).collect()
    }

    /// One timed word per entry, each 1 second long, back to back from 0.
    fn timed(text: &str) -> Vec<TimedWord> {
        words(text)
            .into_iter()
            .enumerate()
            .map(|(i, w)| TimedWord::new(w, i as f64, i as f64 + 1.0))
            .collect()
    }

    fn insertion(at: InsertionAt, word_index: usize, start: f64, end: f64, text: &str) -> Insertion {
        Insertion { at, word_index, start, end, decoded_text: text.into(), score: None }
    }

    #[test]
    fn all_words_match() {
        let s = sentences(&["Often these days, we make things.", "We post them!"]);
        let m = map_words(&s, &timed("often these days we make things. We post them"));
        assert_eq!(m.times, vec![Some((0.0, 6.0)), Some((6.0, 9.0))]);
        assert_eq!(m.report, Report::default());
    }

    #[test]
    fn compares_without_case_or_punctuation_and_keeps_accented_letters() {
        assert_eq!(compare_key("Café,"), "café");
        assert_eq!(compare_key("\"It's\""), "its");
        assert_eq!(compare_key("—"), "");
        assert_eq!(words("this—that and – more"), vec!["this", "that", "and", "more"]);
    }

    #[test]
    fn replaced_run_takes_its_span() {
        // "Fanny Burney" heard as "Fannie Bernie": the two text words take
        // the span of the two timed words.
        let s = sentences(&["Fanny Burney wrote.", "Then rested."]);
        let m = map_words(&s, &timed("Fannie Bernie wrote then rested"));
        assert_eq!(m.times, vec![Some((0.0, 3.0)), Some((3.0, 5.0))]);
        let replaced = |word_index: usize, word: &str| SuspectWord {
            sentence: 0,
            word_index,
            word: word.into(),
            reason: SuspectReason::Replaced,
            heard: Some("Fannie Bernie".into()),
            score: None,
            start: 0.0,
            end: 2.0,
        };
        assert_eq!(m.report.suspect, vec![replaced(0, "Fanny"), replaced(1, "Burney")]);
    }

    #[test]
    fn words_joined_or_split() {
        // "would have" heard as one word; "mantelpiece" heard as two.
        let s = sentences(&["I would have gone.", "By the mantelpiece."]);
        let m = map_words(&s, &timed("I would've gone by the mantle piece"));
        assert_eq!(m.times, vec![Some((0.0, 3.0)), Some((3.0, 7.0))]);
        let heard: Vec<(&str, Option<&str>)> =
            m.report.suspect.iter().map(|w| (w.word.as_str(), w.heard.as_deref())).collect();
        assert_eq!(
            heard,
            vec![("would", Some("would've")), ("have", Some("would've")), ("mantelpiece.", Some("mantle piece"))]
        );
        assert!(m.report.suspect.iter().all(|w| w.reason == SuspectReason::Replaced));
        assert!(m.report.insertions.is_empty());
        assert!(m.report.missing.is_empty());
    }

    #[test]
    fn insertions_between_and_inside_sentences() {
        let s = sentences(&["First one.", "Second one."]);
        // A preamble before everything, a filler between sentences, and one
        // between two words of a sentence.
        let m = map_words(&s, &timed("hello everyone first one um so second uh one"));
        assert_eq!(
            m.report.insertions,
            vec![
                insertion(InsertionAt::BeforeSentence(0), 0, 0.0, 2.0, "hello everyone"),
                insertion(InsertionAt::BeforeSentence(1), 2, 4.0, 6.0, "um so"),
                insertion(InsertionAt::InSentence(1), 3, 7.0, 8.0, "uh"),
            ]
        );
        assert_eq!(m.times, vec![Some((2.0, 4.0)), Some((6.0, 9.0))]);
        assert!(m.report.suspect.is_empty());
    }

    #[test]
    fn insertion_after_the_last_sentence() {
        let s = sentences(&["Only one."]);
        let m = map_words(&s, &timed("only one thank you"));
        assert_eq!(
            m.report.insertions,
            vec![insertion(InsertionAt::BeforeSentence(1), 2, 2.0, 4.0, "thank you")]
        );
    }

    #[test]
    fn replaced_run_across_a_boundary_is_boundary_uncertain() {
        // "end. Start" heard as three different words: the boundary's time
        // is unknown, so the run's span is shared by word count.
        let s = sentences(&["Words at the end.", "Start of next."]);
        let m = map_words(&s, &timed("words at the and stuart art of next"));
        assert_eq!(m.times, vec![Some((0.0, 4.5)), Some((4.5, 8.0))]);
        assert_eq!(
            m.report.suspect,
            vec![
                SuspectWord {
                    sentence: 0,
                    word_index: 3,
                    word: "end.".into(),
                    reason: SuspectReason::BoundaryUncertain,
                    heard: Some("and stuart art".into()),
                    score: None,
                    start: 3.0,
                    end: 4.5,
                },
                SuspectWord {
                    sentence: 1,
                    word_index: 4,
                    word: "Start".into(),
                    reason: SuspectReason::BoundaryUncertain,
                    heard: Some("and stuart art".into()),
                    score: None,
                    start: 4.5,
                    end: 6.0,
                },
            ]
        );
    }

    #[test]
    fn sentence_with_no_timed_words_gets_no_times_and_its_words_are_missing() {
        let s = sentences(&["Spoken here.", "Skipped.", "Spoken again."]);
        let m = map_words(&s, &timed("spoken here spoken again"));
        assert_eq!(m.times, vec![Some((0.0, 2.0)), None, Some((2.0, 4.0))]);
        assert_eq!(
            m.report.missing,
            vec![MissingWord { sentence: 1, word_index: 2, word: "Skipped.".into() }]
        );
        assert!(m.report.suspect.is_empty());
        assert!(m.report.insertions.is_empty());
    }

    #[test]
    fn word_index_counts_words_left_out_of_the_diff() {
        // "&" has no letters or digits, so it takes no part in the diff,
        // but it is still word 1 of the producer's list.
        let s = sentences(&["One & two."]);
        let m = map_words(&s, &timed("one"));
        assert_eq!(m.report.missing, vec![MissingWord { sentence: 0, word_index: 2, word: "two.".into() }]);
    }

    #[test]
    fn sentence_with_no_words_at_all() {
        let s = sentences(&["One.", "—", "Two."]);
        let m = map_words(&s, &timed("one two"));
        assert_eq!(m.times, vec![Some((0.0, 1.0)), None, Some((1.0, 2.0))]);
    }

    #[test]
    fn no_timed_words() {
        let s = sentences(&["One.", "Two."]);
        let m = map_words(&s, &[]);
        assert_eq!(m.times, vec![None, None]);
        assert_eq!(m.report.missing.len(), 2);
    }
}
