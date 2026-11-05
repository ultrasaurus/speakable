//! Read a Descript word-level transcript export.
//!
//! The export is `{"version", "segments": [{"startTime", "endTime", "body"}]}`
//! with one word per segment and punctuation attached to the word ("one,").
//! Other fields, such as `speaker`, are ignored.
//!
//! It is speech recognition output, not corrected against the text, so it
//! differs from the text where the recognizer misheard, spelled a word
//! differently, or joined or split words differently. The mapper's diff
//! absorbs those differences.

use serde::Deserialize;

use crate::map::{words, TimedWord};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DescriptTranscript {
    #[serde(default)]
    pub version: Option<String>,
    pub segments: Vec<DescriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescriptSegment {
    pub start_time: f64,
    pub end_time: f64,
    #[serde(default)]
    pub body: String,
}

impl DescriptTranscript {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The transcript's words in order, with their times. A segment whose
    /// body holds more than one word shares its time equally among them.
    pub fn timed_words(&self) -> Vec<TimedWord> {
        let mut out = Vec::with_capacity(self.segments.len());
        for seg in &self.segments {
            let parts = words(&seg.body);
            if parts.is_empty() {
                continue;
            }
            let step = (seg.end_time - seg.start_time) / parts.len() as f64;
            for (i, part) in parts.iter().enumerate() {
                let start = seg.start_time + step * i as f64;
                let end = if i + 1 == parts.len() { seg.end_time } else { start + step };
                out.push(TimedWord::new(*part, start, end));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_segments_and_ignores_unknown_fields() {
        let json = r#"{"version":"1.0.0","segments":[
            {"speaker":"Speaker 4","startTime":0.27,"endTime":0.63,"body":"Often"},
            {"speaker":"Speaker 4","startTime":0.63,"endTime":0.9,"body":"these"},
            {"startTime":1.0,"endTime":2.0,"body":"two words"},
            {"startTime":2.0,"endTime":2.5,"body":""}
        ]}"#;
        let t = DescriptTranscript::from_json(json).unwrap();
        assert_eq!(t.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            t.timed_words(),
            vec![
                TimedWord::new("Often", 0.27, 0.63),
                TimedWord::new("these", 0.63, 0.9),
                TimedWord::new("two", 1.0, 1.5),
                TimedWord::new("words", 1.5, 2.0),
            ]
        );
    }
}
