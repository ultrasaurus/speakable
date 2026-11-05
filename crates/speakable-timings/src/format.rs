//! The timings JSON, format version 0.1.

use serde::{Deserialize, Serialize};

/// The format version this crate writes: `"major.minor"`, with semver's
/// compatibility rules. Before 1.0 any minor version may change the format
/// incompatibly, so a reader accepts only its own. From 1.0 a minor version
/// only adds optional fields, which a reader ignores, so a reader accepts any
/// version with its own major.
pub const FORMAT_VERSION: &str = "0.1";

/// Sentence timings for one markdown document and one recording of it.
///
/// ```json
/// {
///   "version": "0.1",
///   "audio": "doc.mp3",
///   "sentences": [ { "index": 0, "text": "...", "start": 0.42, "end": 3.10 } ],
///   "report": { "insertions": [], "suspect": [], "missing": [], "filtered": [] }
/// }
/// ```
///
/// `report` is optional, and is left out when it has nothing in it:
///
/// ```json
/// { "version": "0.1", "audio": "doc.mp3", "sentences": [] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// The format version, as `FORMAT_VERSION`.
    pub version: String,
    /// The recording, as a path relative to the JSON file's folder.
    pub audio: String,
    /// Every sentence of the document, in order, timed or not.
    pub sentences: Vec<SentenceTiming>,
    /// Where the text and the audio differ, for review. A player ignores
    /// it and plays the sentence times as given. Left out of the JSON
    /// when empty.
    #[serde(default, skip_serializing_if = "Report::is_empty")]
    pub report: Report,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceTiming {
    pub index: usize,
    /// The sentence's plain text, before any normalization. A reader
    /// re-splits the markdown and compares, so an edit to the markdown
    /// after timing is caught instead of giving wrong times.
    pub text: String,
    /// Seconds from the start of the audio. Both are absent for a sentence
    /// with no timed words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}

impl SentenceTiming {
    /// `(start, end)` when the sentence has both.
    pub fn times(&self) -> Option<(f64, f64)> {
        Some((self.start?, self.end?))
    }
}

/// Where the text and the audio differ.
///
/// The lists, field names and reason names follow forced-alignment's
/// `AlignReport`, so one reader handles reports from either producer. Each
/// entry's `word_index` counts the producer's text words: every sentence's
/// words, in order, as the producer split them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Report {
    /// Speech that isn't in the text.
    #[serde(default)]
    pub insertions: Vec<Insertion>,
    /// Words timed but worth checking.
    #[serde(default)]
    pub suspect: Vec<SuspectWord>,
    /// Text words with no audio, such as words the reader skipped.
    #[serde(default)]
    pub missing: Vec<MissingWord>,
    /// Words the aligner dropped before aligning, having no alignable
    /// characters. Always empty for a transcript.
    #[serde(default)]
    pub filtered: Vec<FilteredWord>,
    /// The aligner's score threshold for suspect words. Only present when
    /// an aligner produced the timings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

impl Report {
    /// True when there's nothing to review and no threshold.
    pub fn is_empty(&self) -> bool {
        self.insertions.is_empty()
            && self.suspect.is_empty()
            && self.missing.is_empty()
            && self.filtered.is_empty()
            && self.threshold.is_none()
    }
}

/// Speech that matches no text word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawInsertion")]
pub struct Insertion {
    #[serde(flatten)]
    pub at: InsertionAt,
    /// The text word the speech comes before: the number of text words for
    /// speech after the last one.
    pub word_index: usize,
    pub start: f64,
    pub end: f64,
    /// The speech as heard: a transcript's words, or an aligner's
    /// low-confidence decode.
    pub decoded_text: String,
    /// The aligner's confidence in `decoded_text`, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// An insertion as written in JSON, before checking that it has exactly
/// one location field. A flattened enum alone would accept both.
#[derive(Deserialize)]
struct RawInsertion {
    before_sentence: Option<usize>,
    in_sentence: Option<usize>,
    word_index: usize,
    start: f64,
    end: f64,
    decoded_text: String,
    #[serde(default)]
    score: Option<f64>,
}

impl TryFrom<RawInsertion> for Insertion {
    type Error = &'static str;

    fn try_from(raw: RawInsertion) -> Result<Self, Self::Error> {
        let at = match (raw.before_sentence, raw.in_sentence) {
            (Some(s), None) => InsertionAt::BeforeSentence(s),
            (None, Some(s)) => InsertionAt::InSentence(s),
            _ => return Err("an insertion needs exactly one of before_sentence and in_sentence"),
        };
        Ok(Insertion {
            at,
            word_index: raw.word_index,
            start: raw.start,
            end: raw.end,
            decoded_text: raw.decoded_text,
            score: raw.score,
        })
    }
}

/// Where among the sentences an insertion falls. In JSON this is exactly
/// one of the fields `before_sentence` and `in_sentence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsertionAt {
    /// Between sentences, before this one. Equal to the sentence count for
    /// speech after the last sentence.
    #[serde(rename = "before_sentence")]
    BeforeSentence(usize),
    /// Inside this sentence, between two of its words.
    #[serde(rename = "in_sentence")]
    InSentence(usize),
}

/// A text word that was timed, but whose time is worth checking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuspectWord {
    pub sentence: usize,
    pub word_index: usize,
    /// The word as it appears in the text.
    pub word: String,
    pub reason: SuspectReason,
    /// The audio's words in place of this one, for `Replaced` and
    /// `BoundaryUncertain`: every word heard in place of the run of text
    /// words this word belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<String>,
    /// The aligner's confidence in the word, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// The time the word was given.
    pub start: f64,
    pub end: f64,
}

/// Why a word is suspect. The first three are forced-alignment's
/// `SuspectReason`s; the last two come from mapping words to sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuspectReason {
    /// The aligner's score is below its threshold.
    LowScore,
    /// Part of a run at the end of the audio crushed into too little time.
    Truncated,
    /// Far longer than the word's pace suggests.
    AnomalousDuration,
    /// The audio has different words here, given in `heard`. The word was
    /// given the time of the words heard in place of its run.
    Replaced,
    /// A sentence boundary falls inside a replaced run, so where the
    /// run's time divides between the sentences is a guess.
    BoundaryUncertain,
}

/// A text word with no audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissingWord {
    pub sentence: usize,
    pub word_index: usize,
    pub word: String,
}

/// A text word the aligner dropped before aligning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilteredWord {
    pub sentence: usize,
    pub word_index: usize,
    pub word: String,
}

/// Why timings can't be used with a document.
#[derive(Debug)]
pub enum Error {
    Json(serde_json::Error),
    /// The file's format version, which this crate can't read: a different
    /// major (or, before 1.0, minor) version, or not `"major.minor"`.
    UnsupportedVersion(String),
    /// The document's sentences differ from the ones the timings were made
    /// for: the markdown changed after timing.
    Stale(Stale),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stale {
    pub document_sentences: usize,
    pub timed_sentences: usize,
    /// The first index where the texts differ, if any within both.
    pub first_difference: Option<usize>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Json(e) => write!(f, "invalid timings JSON: {e}"),
            Error::UnsupportedVersion(v) => {
                write!(f, "timings version {v} is not supported (expected {FORMAT_VERSION})")
            }
            Error::Stale(s) => {
                write!(
                    f,
                    "timings are stale: the document has {} sentences, the timings {}",
                    s.document_sentences, s.timed_sentences
                )?;
                if let Some(i) = s.first_difference {
                    write!(f, "; sentence {i} differs")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

impl Timings {
    /// Read timings JSON. The version is checked first, so a file in an
    /// unsupported version fails as `UnsupportedVersion` even when the rest
    /// of it wouldn't parse.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct VersionOnly {
            version: String,
        }
        let VersionOnly { version } = serde_json::from_str(json)?;
        if !is_supported(&version) {
            return Err(Error::UnsupportedVersion(version));
        }
        Ok(serde_json::from_str(json)?)
    }

    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("timings always serialize");
        json.push('\n');
        json
    }

    /// Check these timings against `markdown`'s sentences, as
    /// `document_sentences` splits them.
    pub fn check(&self, markdown: &str) -> Result<(), Error> {
        let sentences = crate::document_sentences(markdown);
        let first_difference = sentences
            .iter()
            .zip(&self.sentences)
            .position(|(doc, timed)| *doc != timed.text);
        if sentences.len() == self.sentences.len() && first_difference.is_none() {
            return Ok(());
        }
        Err(Error::Stale(Stale {
            document_sentences: sentences.len(),
            timed_sentences: self.sentences.len(),
            first_difference,
        }))
    }

    /// Each sentence's `(start, end)`, by index, for rendering spans.
    pub fn sentence_times(&self) -> Vec<Option<(f64, f64)>> {
        self.sentences.iter().map(SentenceTiming::times).collect()
    }
}

/// `"major.minor"` as two numbers.
fn parse_version(v: &str) -> Option<(u32, u32)> {
    let (major, minor) = v.split_once('.')?;
    let number = |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten();
    Some((number(major)?, number(minor)?))
}

/// Whether this crate reads files in format version `v` (see
/// `FORMAT_VERSION`).
fn is_supported(v: &str) -> bool {
    let ours = parse_version(FORMAT_VERSION).expect("FORMAT_VERSION is major.minor");
    match parse_version(v) {
        Some((0, minor)) => ours == (0, minor),
        Some((major, _)) => major == ours.0,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_follow_semver() {
        assert!(is_supported(FORMAT_VERSION));
        assert!(is_supported("0.1"));
        // Before 1.0, another minor version may be incompatible.
        assert!(!is_supported("0.2"));
        assert!(!is_supported("0.0"));
        assert!(!is_supported("1.0"));
        // Only "major.minor".
        for bad in ["", "0", "1", "0.1.0", "v0.1", "0.x", "0.", ".1", "+0.1", "0.-1"] {
            assert!(!is_supported(bad), "{bad:?}");
        }
    }

    #[test]
    fn unsupported_version_is_reported_before_the_rest_is_parsed() {
        let json = r#"{"version": "2.0", "sentences": "a shape this crate can't read"}"#;
        assert!(matches!(Timings::from_json(json), Err(Error::UnsupportedVersion(v)) if v == "2.0"));
        // A number isn't a version string.
        assert!(matches!(Timings::from_json(r#"{"version": 1}"#), Err(Error::Json(_))));
    }
}
