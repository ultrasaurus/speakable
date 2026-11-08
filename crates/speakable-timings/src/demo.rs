//! A one-file demo page: the rendered document with each sentence in a
//! span, an audio player, the sentence being spoken highlighted, and a
//! click on a sentence seeking to it.
//!
//! The page has its CSS and script inline and loads nothing else but the
//! audio, so it can be opened straight from disk. It plays the times as
//! given: the timings' report is for review, not playback.

use crate::format::Timings;

/// The demo page for `markdown` and its `timings`. `audio_src` is the
/// audio's path relative to where the page will be written.
pub fn render_demo(markdown: &str, timings: &Timings, audio_src: &str) -> String {
    let body = speakable_markdown::to_html_with_sentence_spans(markdown, &timings.sentence_times());
    let title = title_of(markdown).unwrap_or_else(|| "speakable demo".to_string());
    // The body goes in last, so text in the document that looks like a
    // placeholder is never replaced.
    TEMPLATE
        .replace("{{css}}", CSS)
        .replace("{{js}}", JS)
        .replace("{{title}}", &escape_html(&title))
        .replace("{{audio}}", &escape_html(&encode_path(audio_src)))
        .replace("{{body}}", &body)
}

/// The page's style and player script. Kept as their own files so they
/// can be edited as CSS and JS; every page gets a copy inline, so a page
/// stays one file.
const CSS: &str = include_str!("demo.css");
const JS: &str = include_str!("demo.js");

/// The text of the document's first heading, silent or not, with inline
/// markup left out. A `#` line in YAML frontmatter is not a heading.
pub fn title_of(markdown: &str) -> Option<String> {
    let line = speakable_markdown::strip_frontmatter(markdown).lines().find(|l| l.trim_start().starts_with('#'))?;
    let text = line.trim().trim_start_matches('#').trim().trim_end_matches('#').trim();
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
    }
    out.push_str(rest);
    let out: String = out.chars().filter(|c| !matches!(c, '[' | ']' | '*' | '_' | '`')).collect();
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Percent-encode a relative path for a URL, keeping `/` between parts.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

const TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>{{title}}</title>
<style>
{{css}}</style>
</head>
<body>
<div class="player"><audio id="audio" controls preload="metadata" src="{{audio}}"></audio></div>
<main id="text">
{{body}}</main>
<script>
{{js}}</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timings_from_words;
    use crate::TimedWord;

    #[test]
    fn page_has_spans_audio_and_title() {
        let md = "# [My *talk*]<!--silent-->\n\nHello there. Second.";
        let words = vec![TimedWord::new("hello", 1.0, 1.5), TimedWord::new("there", 1.5, 2.0)];
        let timings = timings_from_words(md, &words, "my talk.mp3");
        let page = render_demo(md, &timings, "audio/my talk.mp3");
        assert!(page.contains("<title>My talk</title>"), "{page}");
        assert!(page.contains("src=\"audio/my%20talk.mp3\""), "{page}");
        assert!(page.contains("<span class=\"sentence\" data-sentence=\"0\" start=\"1\" end=\"2\">Hello there.</span>"), "{page}");
        assert!(page.contains("<span class=\"sentence\" data-sentence=\"1\">Second.</span>"), "{page}");
        assert!(!page.contains("{{"), "{page}");
    }

    #[test]
    fn title_from_first_heading() {
        assert_eq!(title_of("intro\n\n## Part `two` ##\n\n# Later"), Some("Part two".to_string()));
        assert_eq!(title_of("no heading"), None);
    }
}
