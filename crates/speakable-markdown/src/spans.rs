//! Markdown -> HTML with one `<span>` per spoken sentence, for a page that
//! highlights each sentence while its audio plays.
//!
//! The sentences are exactly `split_for_export`'s: this module takes each
//! block's sentences from it and only decides where in the HTML they go.
//! It walks the same pulldown-cmark events `split_for_export` walks, finds
//! the same leaf blocks (paragraphs, headings, list-item text, table cells),
//! and replaces each block's source text with its sentences, each wrapped in
//! a span. The edited markdown is then rendered as usual, so lists, quotes,
//! tables and inline formatting come out as they would without spans.
//!
//! Silent text (`[text]<!--silent-->`) on a line of its own, such as a
//! title, is shown outside any sentence span, in `<span class="silent">`.
//! Silent text sharing a line with spoken text is not shown: the spoken
//! sentences of that block replace the whole block, and they don't contain
//! it. Math is shown as its spoken words, the text the sentences carry.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::{
    escape_markdown_chars, find_silent_comment, remove_silent_spans, speak_math,
    split_for_export_with_math_overrides, strip_frontmatter, ExportSentence,
};

/// Render `markdown` to HTML with each spoken sentence in its own span:
///
/// ```html
/// <span class="sentence" data-sentence="0" start="0.27" end="10.05">…</span>
/// ```
///
/// - `data-sentence` is the sentence's index in `split_for_export` order.
/// - `times[i]` gives sentence `i` its `start` and `end` attributes, in
///   seconds. A sentence with `None`, or past the end of `times`, gets a
///   span with no times.
/// - A leading YAML frontmatter block is left out (see `strip_frontmatter`).
pub fn to_html_with_sentence_spans(markdown: &str, times: &[Option<(f64, f64)>]) -> String {
    to_html_with_sentence_spans_and_math_overrides(markdown, times, &HashMap::new())
}

/// Same as `to_html_with_sentence_spans`, with math overrides as in
/// `to_plain_text_with_math_overrides`.
pub fn to_html_with_sentence_spans_and_math_overrides(
    markdown: &str,
    times: &[Option<(f64, f64)>],
    math_overrides: &HashMap<String, String>,
) -> String {
    let markdown = strip_frontmatter(markdown);
    let (sentences, block_lengths) = split_for_export_with_math_overrides(markdown, math_overrides);
    let blocks = group_by_block(&sentences, &block_lengths);

    let (display, silent_lines) = display_markdown(markdown);
    let display = speak_math(&display, math_overrides);
    let leaf_ranges = leaf_block_ranges(&display);

    let spoken: Vec<&Range<usize>> = leaf_ranges
        .iter()
        .filter(|r| !within_any(r, &silent_lines))
        .collect();

    // The display markdown keeps silent-only lines that `split_for_export`
    // drops. Where keeping them changed the block structure (a silent line
    // glued to a paragraph), the leaf blocks no longer line up one to one
    // with `split_for_export`'s, and placing sentences by position would put
    // them in the wrong blocks. The sentences are still right, so show them
    // as one paragraph per block, without the document's structure.
    if spoken.len() != blocks.len() {
        return render_flat(&blocks, times);
    }

    let mut edited = String::with_capacity(display.len() * 2);
    let mut cursor = 0usize;
    let mut sentence_index = 0usize;
    for (range, block) in spoken.into_iter().zip(&blocks) {
        if block.is_empty() {
            // Nothing spoken (a standalone image): leave the source as is.
            continue;
        }
        edited.push_str(&display[cursor..range.start]);
        for (i, sentence) in block.iter().enumerate() {
            if i > 0 {
                edited.push(' ');
            }
            push_span_open(
                &mut edited,
                sentence_index,
                times.get(sentence_index).copied().flatten(),
            );
            edited.push_str(&sentence_markdown(sentence));
            edited.push_str("</span>");
            sentence_index += 1;
        }
        cursor = range.end;
    }
    edited.push_str(&display[cursor..]);

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, Parser::new_ext(&edited, parser_options()));
    html
}

fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options
}

/// `sentences` cut into one slice per block, by `block_lengths`.
fn group_by_block<'a>(
    sentences: &'a [ExportSentence],
    block_lengths: &[usize],
) -> Vec<&'a [ExportSentence]> {
    let mut out = Vec::with_capacity(block_lengths.len());
    let mut start = 0usize;
    for &len in block_lengths {
        out.push(&sentences[start..start + len]);
        start += len;
    }
    out
}

/// A sentence's markdown, safe to render inside its span. When the
/// raw-markdown split couldn't be used for its block, `split_for_export`
/// repeats the plain text as `markdown_text`; escaped, it renders as exactly
/// that text instead of being read as markup.
fn sentence_markdown(sentence: &ExportSentence) -> String {
    if sentence.markdown_text == sentence.text {
        escape_markdown_chars(&sentence.text)
    } else {
        sentence.markdown_text.clone()
    }
}

fn push_span_open(out: &mut String, index: usize, time: Option<(f64, f64)>) {
    let _ = write!(out, "<span class=\"sentence\" data-sentence=\"{index}\"");
    if let Some((start, end)) = time {
        let _ = write!(out, " start=\"{start}\" end=\"{end}\"");
    }
    out.push('>');
}

/// One `<p>` per block, for when sentences can't be placed in the
/// document's own structure (see the caller).
fn render_flat(blocks: &[&[ExportSentence]], times: &[Option<(f64, f64)>]) -> String {
    let mut html = String::new();
    let mut sentence_index = 0usize;
    for block in blocks.iter().filter(|b| !b.is_empty()) {
        html.push_str("<p>");
        for (i, sentence) in block.iter().enumerate() {
            if i > 0 {
                html.push(' ');
            }
            push_span_open(
                &mut html,
                sentence_index,
                times.get(sentence_index).copied().flatten(),
            );
            let mut inner = String::new();
            pulldown_cmark::html::push_html(&mut inner, Parser::new(&sentence_markdown(sentence)));
            let inner = inner.trim();
            let inner = inner
                .strip_prefix("<p>")
                .and_then(|s| s.strip_suffix("</p>"))
                .unwrap_or(inner);
            html.push_str(inner);
            html.push_str("</span>");
            sentence_index += 1;
        }
        html.push_str("</p>\n");
    }
    html
}

/// `markdown` as `strip_silent` leaves it, except that a line made empty by
/// stripping is kept, with its silent text shown in `<span class="silent">`.
/// Returns the text and the byte ranges of those kept lines.
///
/// Built line by line exactly as `strip_silent` builds its output, so every
/// other line is byte-for-byte the same and parses into the same blocks.
fn display_markdown(markdown: &str) -> (String, Vec<Range<usize>>) {
    let mut lines: Vec<String> = Vec::new();
    let mut silent_lines: Vec<Range<usize>> = Vec::new();
    let mut offset = 0usize;
    for line in markdown.lines() {
        let removed = remove_silent_spans(line);
        let trimmed = removed.trim();
        let emptied_by_strip =
            removed != line && (trimmed.is_empty() || trimmed.chars().all(|c| c == '#'));
        let kept = if emptied_by_strip {
            let shown = silent_spans_as_html(line);
            silent_lines.push(offset..offset + shown.len());
            shown
        } else {
            removed
        };
        offset += kept.len() + 1;
        lines.push(kept);
    }
    (lines.join("\n"), silent_lines)
}

/// Replace each `[text]<!--silent-->` in `line` with
/// `<span class="silent">text</span>`, and drop a bare `<!--silent-->`.
fn silent_spans_as_html(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some((start, end)) = find_silent_comment(rest) {
        let before = &rest[..start];
        if before.ends_with(']') {
            if let Some(open) = before.rfind('[') {
                out.push_str(&before[..open]);
                out.push_str("<span class=\"silent\">");
                out.push_str(&before[open + 1..before.len() - 1]);
                out.push_str("</span>");
                rest = &rest[end..];
                continue;
            }
        }
        out.push_str(before);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn within_any(range: &Range<usize>, outer: &[Range<usize>]) -> bool {
    outer
        .iter()
        .any(|o| o.start <= range.start && range.end <= o.end)
}

/// The source range of each leaf block's text, in document order: one entry
/// per entry `split_for_export` gives `block_lengths`, found by the same
/// event rules. Each range is what `split_for_export` takes as the block's
/// raw markdown: trimmed, and for a heading without its `#` markers.
fn leaf_block_ranges(markdown: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut last_end = 0usize;
    let mut item_start_pending = false;
    let mut list_depth = 0usize;

    let push = |ranges: &mut Vec<Range<usize>>, start: usize, end: usize, heading: bool| {
        ranges.push(trimmed_range(markdown, start, end, heading));
    };

    for (event, range) in Parser::new_ext(markdown, parser_options()).into_offset_iter() {
        let item_just_started = std::mem::take(&mut item_start_pending);
        if item_just_started && !matches!(event, Event::End(TagEnd::Item)) {
            block_start = Some(range.start);
        }
        match event {
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell) => {
                block_start = Some(range.start);
            }
            Event::Start(Tag::List(_)) => {
                if list_depth > 0 && !item_just_started {
                    if let Some(start) = block_start.take() {
                        push(&mut ranges, start, last_end, false);
                    }
                }
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                block_start = Some(range.start);
                item_start_pending = true;
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(start) = block_start.take() {
                    push(&mut ranges, start, last_end, true);
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::TableCell) => {
                if let Some(start) = block_start.take() {
                    push(&mut ranges, start, last_end, false);
                }
            }
            _ => {}
        }
        last_end = range.end;
    }
    ranges
}

/// `markdown[start..end]` trimmed, and for a heading also without leading
/// `#` markers, as a range of `markdown`.
fn trimmed_range(markdown: &str, start: usize, end: usize, heading: bool) -> Range<usize> {
    let end = end.max(start);
    let raw = &markdown[start..end];
    let mut s = raw.trim_start();
    if heading {
        s = s.trim_start_matches('#').trim_start();
    }
    let s = s.trim_end();
    // `s` is a subslice of `raw`, so its offset inside `raw` is the
    // difference of their start pointers.
    let begin = s.as_ptr() as usize - raw.as_ptr() as usize;
    (start + begin)..(start + begin + s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split_for_export;

    /// The text inside each sentence span, tags removed, in order.
    fn span_texts(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(i) = rest.find("<span class=\"sentence\"") {
            rest = &rest[i..];
            let open_end = rest.find('>').unwrap() + 1;
            // Sentence spans don't nest, and inner spans are only silent ones,
            // which never appear inside a sentence.
            let close = rest.find("</span>").unwrap();
            out.push(strip_tags(&rest[open_end..close]));
            rest = &rest[close..];
        }
        out
    }

    fn strip_tags(s: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in s.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&#39;", "'")
    }

    fn plain_sentences(md: &str) -> Vec<String> {
        split_for_export(md).0.into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn frontmatter_is_left_out_with_the_same_indices() {
        let body = "# Talk\n\nOne here. Two here.\n\n- Three.\n";
        let with = format!("---\ntitle: Talk\nreader: Jane Doe\n---\n{body}");
        let times = [
            Some((0.0, 1.0)),
            Some((1.0, 2.0)),
            Some((2.0, 3.0)),
            Some((3.0, 4.0)),
        ];
        let html = to_html_with_sentence_spans(&with, &times);
        assert_eq!(html, to_html_with_sentence_spans(body, &times));
        assert!(!html.contains("reader") && !html.contains("<hr"));
        assert_eq!(span_texts(&html), plain_sentences(body));
    }

    #[test]
    fn one_span_per_sentence_with_times() {
        let md = "First one. Second one.\n\nThird.";
        let html = to_html_with_sentence_spans(md, &[Some((0.5, 1.25)), None, Some((2.0, 3.0))]);
        assert_eq!(
            html,
            "<p><span class=\"sentence\" data-sentence=\"0\" start=\"0.5\" end=\"1.25\">First one.</span> \
             <span class=\"sentence\" data-sentence=\"1\">Second one.</span></p>\n\
             <p><span class=\"sentence\" data-sentence=\"2\" start=\"2\" end=\"3\">Third.</span></p>\n"
        );
    }

    #[test]
    fn keeps_structure_and_inline_formatting() {
        let md = "# Title here\n\n- One **bold. Still** bold.\n- Two.\n\n> Quoted text. More.\n\n| a | b |\n|---|---|\n| x | y |\n";
        let html = to_html_with_sentence_spans(md, &[]);
        assert!(
            html.contains(
                "<h1><span class=\"sentence\" data-sentence=\"0\">Title here</span></h1>"
            ),
            "{html}"
        );
        assert!(html.contains("<li><span class=\"sentence\" data-sentence=\"1\">One <strong>bold.</strong></span> <span class=\"sentence\" data-sentence=\"2\"><strong>Still</strong> bold.</span></li>"), "{html}");
        assert!(html.contains("<blockquote>"), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert_eq!(span_texts(&html), plain_sentences(md));
    }

    #[test]
    fn silent_lines_are_shown_outside_sentences() {
        let md =
            "# [Transparency wins]<!--silent-->\n\n[an aside]<!--silent-->\n\nSpoken text here.";
        let html = to_html_with_sentence_spans(md, &[]);
        assert!(
            html.contains("<h1><span class=\"silent\">Transparency wins</span></h1>"),
            "{html}"
        );
        assert!(
            html.contains("<p><span class=\"silent\">an aside</span></p>"),
            "{html}"
        );
        assert_eq!(span_texts(&html), vec!["Spoken text here.".to_string()]);
    }

    #[test]
    fn plain_fallback_text_is_escaped() {
        // A hard break followed by text the raw split may cut differently;
        // whatever path is taken, the spans hold exactly the sentences.
        let md = "Use *stars* and 2 * 3 = 6. Next.";
        let html = to_html_with_sentence_spans(md, &[]);
        assert_eq!(span_texts(&html), plain_sentences(md));
    }

    #[test]
    fn spans_hold_exactly_the_split_sentences() {
        let cases = [
            "1. First item. Has two.\n2. Second item.\n   - Nested one.\n   - Nested two.\n",
            "- Loose item one.\n\n- Loose item two. Again.\n",
            "- Parent text. More.\n  - Child.\n",
            "Before.\n\n![an image](a.png)\n\n```\ncode. not spoken.\n```\n\nAfter.",
            "A [link that. spans two](https://example.com) sentences. Done.",
            "Roses are red.  \nViolets are blue.",
            "Dr. Smith arrived [1]. He sat (Smith, 2020, p. 4). Then left.",
            "## Heading one ##\n\nText *with emphasis. Across* a boundary.",
            "Café crème is good. Ça va?",
        ];
        for md in cases {
            let html = to_html_with_sentence_spans(md, &[]);
            let shown = span_texts(&html);
            let spoken = plain_sentences(md);
            assert_eq!(shown.len(), spoken.len(), "markdown: {md}\nhtml: {html}");
            // An ordered item's number is spoken ("1. First item.") but drawn
            // by the list, not shown in the span.
            for (shown, spoken) in shown.iter().zip(&spoken) {
                assert!(
                    spoken.ends_with(shown.as_str()),
                    "{shown:?} vs {spoken:?}\nmarkdown: {md}\nhtml: {html}"
                );
            }
        }
    }

    #[test]
    fn spans_match_the_split_for_the_sample() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/transparency-wins/talk.md"
        );
        let Ok(md) = std::fs::read_to_string(path) else {
            return;
        };
        let html = to_html_with_sentence_spans(&md, &[]);
        let spoken: Vec<String> = speakable_splitter::split(&crate::to_plain_text(&md))
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(span_texts(&html), spoken);
        assert!(html.contains("<span class=\"silent\">Transparency wins</span>"));
    }
}
