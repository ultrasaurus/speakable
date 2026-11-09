// README.md with its links pinned to this version (build.rs).
#![doc = include_str!(concat!(env!("OUT_DIR"), "/README.md"))]
//!
//! ## Design notes
//!
//! Markdown -> plain text conversion, for TTS sentence splitting.
//!
//! Bold/italic/inline-code markers are removed and links are reduced to
//! their link text. Fenced code blocks, images, and horizontal rules are
//! dropped entirely, since they have nothing to say aloud.
//!
//! A renderer that shows the original markdown with one span per sentence
//! (for highlighting while audio plays) walks the same blocks this crate
//! does, so the sentence counts here are a contract with that renderer:
//! every block gets an entry, even one that speaks nothing. Block-level elements (paragraphs,
//! headings, list items, blockquote text) are joined with blank lines so
//! the result can be split into paragraphs the same way as plain-text input.

use std::collections::HashMap;

pub mod images;
pub mod linkify;
mod spans;

pub use spans::{to_html_with_sentence_spans, to_html_with_sentence_spans_and_math_overrides};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// One spoken sentence, with both its plain text (for synthesis indices)
/// and its raw markdown source (for inline-formatted export rendering).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSentence {
    pub text: String,
    pub markdown_text: String,
    pub paragraph_end: bool,
}

/// Split `markdown` into sentences for static-export rendering, paired with
/// per-block sentence counts so a caller walking the same block structure
/// (e.g. a client-side `marked.lexer`) knows how many sentences each block
/// contributes, without having to run the splitter itself.
///
/// Per block, the plain-text split (via `speakable_splitter::split`) determines the
/// sentence count; the raw-markdown split is used for the rendered text only
/// when its count matches — otherwise the
/// plain text is repeated as `markdown_text` for that block, since a count
/// mismatch means the raw split can't be trusted to align sentence-for-
/// sentence with the plain one.
pub fn split_for_export(markdown: &str) -> (Vec<ExportSentence>, Vec<usize>) {
    split_for_export_with_math_overrides(markdown, &HashMap::new())
}

/// Same as `split_for_export`, but with math overrides — see
/// `to_plain_text_with_math_overrides`.
pub fn split_for_export_with_math_overrides(
    markdown: &str,
    math_overrides: &HashMap<String, String>,
) -> (Vec<ExportSentence>, Vec<usize>) {
    let markdown = strip_silent(strip_frontmatter(markdown));
    let markdown = speak_math(&markdown, math_overrides);

    let mut sentences: Vec<ExportSentence> = Vec::new();
    let mut block_lengths: Vec<usize> = Vec::new();

    let mut current_plain = String::new();
    let mut block_start: Option<usize> = None;
    let mut last_end = 0usize;
    let mut in_code_block = false;
    let mut in_image = false;
    // A list item's own range starts at its marker (`- `, `1. `). In a tight
    // list (no blank lines between items) no `Paragraph` event follows to
    // move `block_start` past it, so the marker would end up in
    // `markdown_text` — and the export reader, which draws the list's own
    // bullet or number, would show it twice. The item's text is taken to
    // start at its first event after `Start(Item)` instead.
    let mut item_start_pending = false;
    // Ordered-list item numbers, spoken as `to_plain_text` speaks them: the
    // stored sentences come from `to_plain_text`, and a sentence count that
    // differs from theirs makes the export drop every block boundary. Only
    // the spoken text gets the number; `markdown_text` stays without it.
    let mut ordered_list_stack: Vec<Option<u64>> = Vec::new();

    // ENABLE_TABLES: without it, pipe-table syntax is never recognized as a
    // table at all — the whole thing falls through as one raw-pipe
    // paragraph, both mis-rendering client-side and desyncing every
    // sentence index after it (see `to_plain_text`'s doc comment; the same
    // gap existed here independently since this function has its own
    // Parser::new call).
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);

    for (event, range) in Parser::new_ext(&markdown, options).into_offset_iter() {
        let item_just_started = std::mem::take(&mut item_start_pending);
        if item_just_started && !matches!(event, Event::End(TagEnd::Item)) {
            block_start = Some(range.start);
        }
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::Image { .. }) => in_image = true,
            Event::End(TagEnd::Image) => in_image = false,
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell) => {
                block_start = Some(range.start);
            }
            Event::Start(Tag::List(start)) => {
                // A nested list inside a tight item: the parent item's own
                // text ends here, as its own block, exactly where
                // `to_plain_text` ends it. Not when the nested list is the
                // item's first content (`- - x`): there is no parent text,
                // and a block-walking renderer adds nothing for it.
                if !ordered_list_stack.is_empty() && !item_just_started {
                    if let Some(start) = block_start.take() {
                        let raw = markdown[start..last_end].trim();
                        push_export_block(&mut sentences, &mut block_lengths, &current_plain, raw);
                    }
                    current_plain.clear();
                }
                ordered_list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                ordered_list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                block_start = Some(range.start);
                item_start_pending = true;
                if let Some(Some(n)) = ordered_list_stack.last_mut() {
                    current_plain.push_str(&n.to_string());
                    current_plain.push_str(". ");
                    *n += 1;
                }
            }
            Event::Text(text) | Event::Code(text) if !in_code_block && !in_image => {
                current_plain.push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak => {
                current_plain.push(' ');
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(start) = block_start.take() {
                    // Strip leading "## " so markdown_text doesn't include "#"
                    // markers — renderToken already wraps the span in <h1>/<h2>/etc.
                    let raw = markdown[start..last_end]
                        .trim()
                        .trim_start_matches('#')
                        .trim_start();
                    push_export_block(&mut sentences, &mut block_lengths, &current_plain, raw);
                }
                current_plain.clear();
            }
            Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::TableCell) => {
                if let Some(start) = block_start.take() {
                    let raw = markdown[start..last_end].trim();
                    push_export_block(&mut sentences, &mut block_lengths, &current_plain, raw);
                }
                current_plain.clear();
            }
            _ => {}
        }
        last_end = range.end;
    }

    (sentences, block_lengths)
}

/// Stand-in for a hard break during `collapse_hard_breaks`, restored to the
/// real `<br>` by `split_markdown_sentences_for_render` once sentence
/// splitting is done (see that function's own comment for why). A Unicode
/// *noncharacter* — the standard formally reserves these and guarantees
/// they never appear in valid text interchange, so there's no risk of this
/// colliding with real authored content, unlike a Private Use Area
/// codepoint (some fonts/legacy icon sets do assign real glyphs to those).
/// Deliberately *not* `speakable_splitter::PERIOD_PLACEHOLDER` (`\u{FFFE}`) — that
/// noncharacter is already meaningful *inside* `speakable_splitter::split` itself
/// (its own abbreviation-period protection), and this text is about to be
/// passed straight into that same function, so reusing its exact marker
/// would risk exactly the kind of confusion a second, independent
/// noncharacter avoids entirely.
const HARD_BREAK_PLACEHOLDER: &str = "\u{FFFF}";

/// Collapse a markdown hard break (line ending in 2+ spaces, or a trailing
/// backslash) to `HARD_BREAK_PLACEHOLDER` so it survives sentence
/// splitting; a soft break collapses to a space. The literal `<br>` text
/// itself can't be used directly here — tried first, it confused
/// `speakable_splitter::split`'s sentence-boundary detection often enough to
/// silently break rendering: `"Roses are red.  \nViolets are blue."`
/// collapsed straight to `<br>` splits into *one* sentence
/// (`"Roses are red. <br>Violets are blue."`, no boundary detected after
/// "red."), while the same text's plain-text form (`<br>` stripped
/// entirely, real newline collapsed to a space by `to_plain_text`) splits
/// into two — the count mismatch this function's own caller depends on to
/// agree with the plain-text split, so the raw-markdown render path
/// silently gave up and fell back to plain, unformatted text, dropping
/// the hard break (and any other formatting in that block) entirely. The
/// placeholder round-trips through splitting as inert, unremarkable text
/// (verified directly — the same input splits into two sentences with it),
/// so the count agreement holds and the real `<br>` only gets restored
/// afterward, once there's no more sentence-boundary detection left to
/// confuse.
fn collapse_hard_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find('\n') {
        let before = &rest[..idx];
        if before.ends_with("  ") {
            out.push_str(before.trim_end_matches(' '));
            out.push(' ');
            out.push_str(HARD_BREAK_PLACEHOLDER);
        } else if let Some(before) = before.strip_suffix('\\') {
            out.push_str(before);
            out.push_str(HARD_BREAK_PLACEHOLDER);
        } else {
            out.push_str(before);
            out.push(' ');
        }
        rest = &rest[idx + 1..];
    }
    out.push_str(rest);
    out
}

/// A CommonMark inline span this module tracks across sentence boundaries,
/// as its exact open/close source text — e.g. `*`/`*` for emphasis,
/// `**`/`**` for strong, or `[`/`](href "title")` for a link, captured
/// verbatim from the source rather than reconstructed (so an authored
/// title's quoting/escaping is preserved exactly).
struct StraddlingSpan {
    range: std::ops::Range<usize>,
    open: String,
    close: String,
}

/// Real CommonMark emphasis/strong/link spans in `text`, found via
/// `pulldown_cmark` rather than naive delimiter-counting — e.g. a lone `*`
/// next to punctuation (`*,*`) often isn't valid emphasis at all per
/// CommonMark's flanking rules, and only a real parse knows that.
/// `Start(Emphasis)`/`Start(Strong)`/`Start(Link)` events carry the
/// *whole* span's byte range, so no bookkeeping is needed for `End`
/// events. Naturally ordered outermost-before-innermost by
/// `pulldown_cmark`'s event order, which the caller relies on.
///
/// A link's range covers the full `[label](dest "title")` source; the
/// split between `open` and `close` is found by locating the last `](` in
/// that range — links can't nest in CommonMark, so a coincidental `](`
/// inside the label text itself isn't a realistic concern.
///
/// `pulldown_cmark` has no notion of `\(...\)`/`\[...\]`/`$...$` math
/// syntax — it's a plain-text-level convention this module strips/speaks
/// itself (see the "Math spans" section below), not real CommonMark. So a
/// bare `*` that's actually math content (e.g. `\(*\)`, read aloud as
/// "asterisk") can flank real prose asterisks closely enough to look like
/// a genuine, even straddle-a-sentence-boundary, emphasis delimiter to a
/// parse that doesn't know better — pairing with an unrelated real `*...*`
/// span elsewhere in the paragraph and corrupting both once rendered
/// (confirmed: `the \(*\) operator...known as *convolution*... \(*\) for
/// multiplication` renders the second `\(*\)` as a bare `()` with its
/// asterisk stolen into a bogus `<em>` reopened from the first). Any span
/// whose open or close delimiter position falls inside a math span is
/// discarded here rather than trusted.
fn inline_straddle_spans(text: &str) -> Vec<StraddlingSpan> {
    let math_ranges: Vec<std::ops::Range<usize>> = find_math_spans(text)
        .into_iter()
        .map(|m| m.start..m.end)
        .collect();
    let delimiter_in_math = |pos: usize| math_ranges.iter().any(|r| r.contains(&pos));

    Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::Emphasis) => Some(StraddlingSpan {
                range: range.clone(),
                open: "*".to_string(),
                close: "*".to_string(),
            }),
            Event::Start(Tag::Strong) => Some(StraddlingSpan {
                range: range.clone(),
                open: "**".to_string(),
                close: "**".to_string(),
            }),
            Event::Start(Tag::Link { .. }) => {
                let split_at = text[range.clone()].rfind("](")?;
                let label_end = range.start + split_at; // "]" starts the close delimiter
                Some(StraddlingSpan {
                    open: text[range.start..range.start + 1].to_string(),
                    close: text[label_end..range.end].to_string(),
                    range,
                })
            }
            _ => None,
        })
        .filter(|span| {
            let close_start = span.range.end - span.close.len();
            !delimiter_in_math(span.range.start) && !delimiter_in_math(close_start)
        })
        .collect()
}

/// Byte range of each of `sentences` within `text`, in order. `Sentence`
/// carries no offsets of its own, so each one is relocated by sequential
/// substring search from a cursor that only moves forward — safe because
/// `speakable_splitter::split` always returns sentences as exact, in-order,
/// non-overlapping substrings of its input.
fn locate_sentence_ranges(
    text: &str,
    sentences: &[speakable_splitter::Sentence],
) -> Vec<std::ops::Range<usize>> {
    let mut cursor = 0usize;
    sentences
        .iter()
        .map(|s| {
            let start = text[cursor..]
                .find(s.text.as_str())
                .map(|offset| cursor + offset)
                .unwrap_or(cursor);
            let end = start + s.text.len();
            cursor = end;
            start..end
        })
        .collect()
}

/// Close/reopen any emphasis/strong/link span that straddles a sentence
/// boundary, so each of the returned strings is independently parseable
/// (e.g. by `marked.parseInline()` client-side, or `pulldown_cmark`) without breaking a span that was never meant to be cut
/// in half. Boundary *positions* are exactly `sentences`' own boundaries —
/// never moved, since those have to stay in lockstep with the identical
/// split the server runs over `plain_text`; only how the raw markdown is
/// apportioned across them changes.
///
/// A span straddles the boundary between sentence `i` and `i+1` when it
/// starts at or before sentence `i`'s end and ends at or after sentence
/// `i+1`'s start. When more than one straddles the same boundary (nested
/// spans), they're closed innermost-first and reopened outermost-first —
/// the order `pulldown_cmark`'s event stream already gives them in — to
/// preserve the original nesting on both sides of the cut.
fn balance_straddling_spans(
    text: &str,
    sentences: Vec<speakable_splitter::Sentence>,
) -> Vec<String> {
    let ranges = locate_sentence_ranges(text, &sentences);
    let spans = inline_straddle_spans(text);
    let mut chunks: Vec<String> = sentences.into_iter().map(|s| s.text).collect();

    for i in 0..chunks.len().saturating_sub(1) {
        let boundary_start = ranges[i].end;
        let boundary_end = ranges[i + 1].start;

        let straddling: Vec<&StraddlingSpan> = spans
            .iter()
            .filter(|s| s.range.start < boundary_end && s.range.end > boundary_start)
            .collect();

        if straddling.is_empty() {
            continue;
        }

        for span in straddling.iter().rev() {
            chunks[i].push_str(&span.close);
        }

        // A span only needs its opener reopened in chunk i+1 if its
        // content actually continues past the boundary. When the boundary
        // instead lands right at an already-intact closing delimiter (the
        // span's content ended in chunk i; only the delimiter itself spilled
        // into chunk i+1), that delimiter is real text already sitting
        // there — reopening would duplicate it, so strip it instead.
        let mut reopen = String::new();
        for span in &straddling {
            let content_end = span.range.end - span.close.len();
            if content_end <= boundary_start && chunks[i + 1].starts_with(span.close.as_str()) {
                chunks[i + 1] = chunks[i + 1][span.close.len()..].to_string();
            } else {
                reopen.push_str(&span.open);
            }
        }
        chunks[i + 1] = format!("{reopen}{}", chunks[i + 1]);
    }

    chunks
}

/// Split one markdown block's raw source into `expected_sentence_count`
/// sentences (`None` if the raw-markdown split doesn't actually produce
/// that many — a caller should then fall back to the plain text, as
/// `push_export_block` does), with any emphasis/strong/link span
/// that would otherwise straddle a sentence boundary closed and reopened
/// across the cut (see `balance_straddling_spans`). Public so a renderer
/// that builds per-sentence markdown itself (e.g. one running in the
/// browser through wasm) shares this exact logic with `push_export_block`
/// below.
///
/// Takes the expected count as a plain `usize` rather than re-deriving it
/// from `raw` internally, so each caller supplies whatever plain-text split
/// it already computed for this block — guaranteeing the count this
/// function checks against is the exact same one the caller's own
/// subsequent indexing relies on, rather than two independently-derived
/// plain-text conversions that could in principle disagree.
pub fn split_markdown_sentences_for_render(
    raw: &str,
    expected_sentence_count: usize,
) -> Option<Vec<String>> {
    let raw_collapsed = collapse_hard_breaks(raw);
    let raw_sentences = speakable_splitter::split(&raw_collapsed);

    if raw_sentences.len() != expected_sentence_count {
        return None;
    }

    let chunks = balance_straddling_spans(&raw_collapsed, raw_sentences);
    // Restore the real `<br>` now that sentence-boundary detection is done
    // — see `HARD_BREAK_PLACEHOLDER`'s own comment for why it can't just
    // be `<br>` from the start. Per chunk, not on the joined text: each
    // chunk is independent by this point, and a plain per-chunk `replace`
    // needs no position tracking across the cuts `balance_straddling_spans`
    // just made.
    Some(
        chunks
            .into_iter()
            .map(|c| c.replace(HARD_BREAK_PLACEHOLDER, "<br>"))
            .collect(),
    )
}

fn push_export_block(
    sentences: &mut Vec<ExportSentence>,
    block_lengths: &mut Vec<usize>,
    plain: &str,
    raw: &str,
) {
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.is_empty() {
        // Push 0 rather than omitting the entry — a block with nothing to
        // speak (a standalone image, or math `speak_math` can't phrase)
        // still needs a `block_lengths` slot so a block-walking renderer's
        // per-block lookups line up 1:1 with blocks in document
        // order. Omitting the entry here used to force such a renderer to
        // separately guess, per block, whether Rust would have counted it
        // — correct as long as every such block spoke nothing, which
        // broke once math-to-speech made some standalone formulas speak
        // real sentences while others still don't.
        block_lengths.push(0);
        return;
    }
    let plain_sentences = speakable_splitter::split(&plain);
    let raw_sentences = split_markdown_sentences_for_render(raw, plain_sentences.len());

    block_lengths.push(plain_sentences.len());
    for (i, s) in plain_sentences.into_iter().enumerate() {
        let markdown_text = match &raw_sentences {
            Some(raw_sentences) => raw_sentences[i].clone(),
            None => s.text.clone(),
        };
        sentences.push(ExportSentence {
            text: s.text,
            markdown_text,
            paragraph_end: s.paragraph_end,
        });
    }
}

/// Convert plain text (e.g. a `.txt` import with no markdown intent) into
/// markdown that renders back out as itself — the inverse of
/// `to_plain_text`.
///
/// Two things plain text needs before it's safe as markdown `content`:
/// - **Escape every CommonMark-special character.** Without this, a
///   character like `*` (e.g. a footnote marker written as `*1*`) stores
///   verbatim and the client's `marked` parser reads it as real emphasis
///   syntax — wrong rendering, and it desyncs the client's sentence count
///   from the server's. Escaping the full CommonMark-escapable set up
///   front means there's no need to special-case which characters "look
///   like" formatting; anything outside that set is a no-op for a
///   backslash anyway.
/// - **Force each paragraph onto its own blank-line-separated block.**
///   `to_plain_text`'s contract is one paragraph per line, single `\n`
///   between them — but CommonMark treats a single `\n`
///   between two lines as a soft break *within* one paragraph, not a
///   paragraph boundary. Left alone, every line of the import would
///   collapse into one giant paragraph. Joining with `\n\n` instead
///   matches how `to_plain_text` itself joins blocks, and how
///   `speakable_splitter::split`'s "each non-blank line is its own paragraph" rule
///   expects paragraphs to be delimited.
///
/// Leading/trailing whitespace on each line is trimmed for the same
/// reason: a stray 4-space indent would otherwise read as a CommonMark
/// indented code block.
pub fn from_plain_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(escape_markdown_chars)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Escape literal `<`/`>` characters in already-formatted markdown so
/// CommonMark's raw-inline-HTML rule doesn't swallow them.
///
/// HTML-to-markdown conversion (e.g. trafilatura) can leave source text
/// like `<Select>` (a literal reference to a UI control, not real markup)
/// unescaped. The client's `marked` parser reads that as a raw HTML tag —
/// e.g. an actual `<select>` element gets created, silently absorbing
/// everything up to some later `>` as its content. Escaping outside fenced
/// code, inline code spans, and leading blockquote `>` markers avoids
/// breaking the few places `<`/`>` are structurally meaningful.
///
/// Also skips escaping inside a math span (`$...$`, `$$...$$`, `\(...\)`,
/// `\[...\]`) — `<`/`>` there are usually the literal comparison operators
/// (`t < 0`), and escaping them to `\<`/`\>` produces `\<`, which isn't
/// valid LaTeX and makes `mitex::convert_math` (math-render crate) reject
/// the whole formula. A real bug hit in production: `\(t < 0\)` round-
/// tripped through ingest as `\(t \< 0\)`, which failed to render at all.
pub fn escape_literal_angle_brackets(text: &str) -> String {
    let math_ranges: Vec<(usize, usize)> = find_math_spans(text)
        .into_iter()
        .map(|s| (s.start, s.end))
        .collect();
    let in_math = |pos: usize| {
        math_ranges
            .iter()
            .any(|&(start, end)| pos >= start && pos < end)
    };

    let mut in_fence = false;
    let mut out = String::with_capacity(text.len());
    let mut offset = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            offset += 1;
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            offset += line.len();
            continue;
        }
        if in_fence {
            out.push_str(line);
            offset += line.len();
            continue;
        }
        let marker_len = leading_blockquote_marker_len(line);
        out.push_str(&line[..marker_len]);
        let mut pos = offset + marker_len;
        let mut in_code_span = false;
        for c in line[marker_len..].chars() {
            if c == '`' {
                in_code_span = !in_code_span;
                out.push(c);
            } else if !in_code_span && (c == '<' || c == '>') && !in_math(pos) {
                out.push('\\');
                out.push(c);
            } else {
                out.push(c);
            }
            pos += c.len_utf8();
        }
        offset += line.len();
    }
    out
}

/// Byte length of leading blockquote markers (`>`, optionally nested and
/// space-separated) at the start of a line — left unescaped since they're
/// structural, not literal text.
fn leading_blockquote_marker_len(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut i = 0;
    loop {
        let start = i;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'>' {
            i += 1;
            if i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
        } else {
            return start;
        }
    }
}

/// Trim leading/trailing whitespace from each line of already-formatted
/// markdown, skipping lines inside fenced (```) code blocks.
///
/// HTML-to-markdown conversion (e.g. trafilatura) can leave stray leading
/// spaces on a line — enough to hit CommonMark's 4-space indented-code-block
/// rule — from source markup like nested lists or blockquotes that carried
/// no real indentation intent. Trimming outside fences fixes that without
/// disturbing indentation that's meaningful inside a fenced code block.
///
/// A list-item line straight after another list-item line keeps its
/// indentation: that's a nested list item, which the importer writes
/// indented past its parent's marker (trafilatura itself writes nested
/// lists flat, so its stray spaces never land there).
pub fn trim_line_indentation(text: &str) -> String {
    let mut in_fence = false;
    let mut prev_is_item = false;
    text.lines()
        .map(|line| {
            let trimmed = line.trim();
            let is_item = !in_fence && leading_marker_len(trimmed) > 0;
            let out = if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                trimmed
            } else if in_fence {
                line
            } else if is_item && prev_is_item {
                line.trim_end()
            } else {
                trimmed
            };
            prev_is_item = is_item;
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a table header row off the end of the text line it was glued to.
///
/// Trafilatura emits a `<table>` that follows bare text inside a `<div>`
/// (no `<p>` around the text) on the same line as that text:
/// `...changes in rCBF.| Analysis | Location | ...`. The glued line doesn't
/// start with `|`, so the table's separator row below it isn't preceded by a
/// header row and the whole table renders as one literal paragraph.
///
/// A line is split only when the row after it is a separator row and the
/// text from the line's first `|` has the same pipe count as that separator,
/// so ordinary prose containing a `|` is never touched. The text before the
/// `|` becomes its own paragraph. Skips lines inside fenced (```) code blocks.
pub fn split_glued_table_header(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    for (i, &line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && !line.starts_with('|') {
            if let (Some(split_at), Some(&next)) = (line.find('|'), lines.get(i + 1)) {
                let header = &line[split_at..];
                let pipes = |s: &str| s.bytes().filter(|&b| b == b'|').count();
                if is_table_separator_row(next) && pipes(header) == pipes(next) {
                    let prefix = line[..split_at].trim_end();
                    if !prefix.is_empty() {
                        out.push(prefix.to_string());
                        out.push(String::new());
                    }
                    out.push(header.to_string());
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Pad a pipe-table row's trailing `|` count up to match its table's
/// header/separator, skipping lines inside fenced (```) code blocks.
///
/// Trafilatura's markdown table writer drops the trailing empty-cell `|`s
/// on the very last row of every table it emits (confirmed on a real page
/// and in isolated tests — rows 1..N-1 all match the header's pipe count,
/// only row N comes up short, e.g. header `| A | B | |` but last row
/// `| 2 | y |` — missing the final empty cell's `|`). That short row's
/// cell count mismatches the header, which is enough for a strict GFM
/// table parser (both our own `to_plain_text` and the client's `marked`)
/// to reject the whole table as unparseable — so it falls back to one
/// giant literal paragraph (raw pipes/dashes and all) instead of a real
/// table, which is what actually gets spoken by TTS and throws off
/// sentence-splitting for everything after it.
///
/// The separator row (`|---|---|---|`, always present directly under a
/// real table's header) is what's actually padded-to: once seen, its pipe
/// count becomes the target for every following `|`-prefixed row until a
/// blank or non-`|` line ends the table.
pub fn close_unclosed_table_rows(text: &str) -> String {
    let mut in_fence = false;
    let mut target_pipes: Option<usize> = None;
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                return line.to_string();
            }
            if in_fence {
                return line.to_string();
            }
            if !line.starts_with('|') {
                target_pipes = None;
                return line.to_string();
            }

            let pipe_count = line.bytes().filter(|&b| b == b'|').count();
            if is_table_separator_row(line) {
                target_pipes = Some(pipe_count);
                return line.to_string();
            }

            match target_pipes {
                Some(target) if pipe_count < target => {
                    let mut padded = line.to_string();
                    padded.extend(std::iter::repeat_n('|', target - pipe_count));
                    padded
                }
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop sections an extraction left empty: their content was buttons or
/// dialogs the extractor dropped, so only the heading survived (PubMed's
/// "Publication types", "MeSH terms" and "LinkOut" footer). Without this
/// the empty headings run together into one spoken sentence.
/// - An ATX heading followed by a heading of the same or a higher level
///   (fewer `#`s), or by the end of the text, is empty. One followed by a
///   deeper heading (`## Part 1` then `### Chapter`) is not.
/// - A list whose every item is only a heading (`- ### Full Text Sources`)
///   is empty, unless a line indented under it continues an item.
/// - Repeated until nothing changes, so a section holding only empty
///   sections goes too. Fenced code is left alone.
pub fn drop_empty_sections(text: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Line {
        Blank,
        Heading(usize),
        HeadingItem,
        Other,
    }
    fn classify(line: &str) -> Line {
        if line.trim().is_empty() {
            return Line::Blank;
        }
        if let Some(level) = atx_level(line) {
            return Line::Heading(level);
        }
        let item = line
            .strip_prefix(['-', '*', '+'])
            .and_then(|rest| rest.strip_prefix(' '));
        match item.map(str::trim_start).and_then(atx_level) {
            Some(_) => Line::HeadingItem,
            None => Line::Other,
        }
    }
    fn atx_level(line: &str) -> Option<usize> {
        let level = line.chars().take_while(|&c| c == '#').count();
        let rest = &line[level..];
        ((1..=6).contains(&level) && (rest.is_empty() || rest.starts_with([' ', '\t'])))
            .then_some(level)
    }

    let mut lines: Vec<&str> = text.lines().collect();
    let mut dropped_any = false;
    loop {
        let mut in_fence = false;
        let kinds: Vec<Line> = lines
            .iter()
            .map(|line| {
                if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                    in_fence = !in_fence;
                    return Line::Other;
                }
                if in_fence {
                    Line::Other
                } else {
                    classify(line)
                }
            })
            .collect();
        let next_non_blank = |from: usize| (from..kinds.len()).find(|&j| kinds[j] != Line::Blank);

        let mut drop = vec![false; lines.len()];
        let mut i = 0;
        while i < lines.len() {
            match kinds[i] {
                Line::Heading(level) => {
                    let empty = match next_non_blank(i + 1) {
                        None => true,
                        Some(j) => matches!(kinds[j], Line::Heading(next) if next <= level),
                    };
                    if empty {
                        drop[i] = true;
                        // Its blank line goes with it.
                        if kinds.get(i + 1) == Some(&Line::Blank) {
                            drop[i + 1] = true;
                        }
                    }
                    i += 1;
                }
                Line::HeadingItem => {
                    let mut end = i;
                    while end < lines.len() && matches!(kinds[end], Line::HeadingItem | Line::Blank)
                    {
                        end += 1;
                    }
                    // Indented under an item, or more items: the list has more than
                    // headings.
                    let continued = end < lines.len()
                        && (lines[end].starts_with([' ', '\t'])
                            || lines[end]
                                .strip_prefix(['-', '*', '+'])
                                .is_some_and(|r| r.starts_with(' ')));
                    let starts_list = i == 0
                        || kinds[i - 1] == Line::Blank
                        || matches!(kinds[i - 1], Line::Heading(_));
                    if !continued && starts_list {
                        drop[i..end].fill(true);
                    }
                    i = end;
                }
                _ => i += 1,
            }
        }
        if !drop.contains(&true) {
            break;
        }
        dropped_any = true;
        lines = lines
            .into_iter()
            .zip(drop)
            .filter(|(_, d)| !d)
            .map(|(l, _)| l)
            .collect();
    }

    if !dropped_any {
        return text.to_string();
    }
    // A section dropped at the end leaves the blank line before it.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Insert a GFM separator row (`|---|---|`) after the first row of a
/// pipe-table block that's missing one.
///
/// Trafilatura's markdown table writer only emits a separator row when the
/// source HTML table has real `<th>` header cells — a plain
/// `<table><tbody><tr><td>...` table (no `<th>` anywhere, e.g. an
/// old-book-scan table of contents) comes out as bare `| A | B |` rows with
/// no separator at all. That's not valid GFM: with no separator row and no
/// blank lines between rows, both `to_plain_text` and the client's `marked`
/// read the whole block as one literal paragraph — pipes and all — instead
/// of a table.
pub fn insert_missing_table_separator(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    for (i, &line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }

        let prev_is_table = i > 0 && lines[i - 1].starts_with('|');
        out.push(line.to_string());

        if line.starts_with('|') && !prev_is_table {
            if let Some(&next) = lines.get(i + 1) {
                if next.starts_with('|') && !is_table_separator_row(next) {
                    let cols = line
                        .bytes()
                        .filter(|&b| b == b'|')
                        .count()
                        .saturating_sub(1)
                        .max(1);
                    out.push(format!("|{}", "---|".repeat(cols)));
                }
            }
        }
    }
    out.join("\n")
}

/// True if `line` is a markdown table separator row (`|---|:--:|---|`) —
/// only `|`, `-`, `:`, and whitespace, with at least one `-`.
fn is_table_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed.contains('-')
        && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Escape a leading list-marker (`-`/`+`/`*`, or `1.`) on a line of
/// already-formatted markdown when it isn't actually part of a list —
/// i.e. neither the nearest non-blank line before it nor after it also
/// starts with a marker. Skips lines inside fenced (```) code blocks.
///
/// Trafilatura's markdown renderer writes a block's text verbatim,
/// including a source hyphen used as a citation dash (`- Author Name`,
/// mid-paragraph in the source HTML) — if that hyphen lands at the very
/// start of trafilatura's output line, CommonMark reads it as a bullet.
/// Confirmed on a real page: `<span>- Tai Amri Spann-Ryan from </span>`
/// renders as a one-item bulleted list instead of the literal dash it is.
///
/// Real HTML lists are never mistaken for this: trafilatura only ever
/// emits `-`-marker lines for genuine `<li>` content, and always as 2+
/// adjacent marker lines (a single `<li>` collapses to a plain
/// unmarked paragraph, confirmed empirically) — so an isolated
/// marker line with no marker neighbor is never a real list.
pub fn escape_isolated_list_markers(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    let mut fenced = vec![false; lines.len()];
    let mut marker = vec![false; lines.len()];
    {
        let mut in_fence = false;
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                fenced[i] = true;
                continue;
            }
            fenced[i] = in_fence;
            if !in_fence {
                marker[i] = leading_marker_len(line) > 0;
            }
        }
    }

    // A tight-list item's own text can soft-wrap onto continuation lines
    // that don't themselves start with a marker (e.g. a multi-line
    // reference-list `<li>` whose citation text wraps across several
    // lines) — so "real list" can't just mean "the immediate neighbor line
    // has a marker". Instead, group into blocks of contiguous non-blank
    // lines (blank lines always break a block) and count markers per
    // block: 2+ marker lines anywhere in the same block is a real list
    // (its items just happen to wrap), even if they're not adjacent.
    let mut block_id = vec![usize::MAX; lines.len()];
    let mut block_marker_count: Vec<usize> = Vec::new();
    {
        let mut current: Option<usize> = None;
        for (i, line) in lines.iter().enumerate() {
            if fenced[i] || line.trim().is_empty() {
                current = None;
                continue;
            }
            let id = *current.get_or_insert_with(|| {
                block_marker_count.push(0);
                block_marker_count.len() - 1
            });
            block_id[i] = id;
            if marker[i] {
                block_marker_count[id] += 1;
            }
        }
    }

    let nearest_marker = |mut range: Box<dyn Iterator<Item = usize>>| {
        range
            .find(|&j| !lines[j].trim().is_empty())
            .map(|j| marker[j])
            .unwrap_or(false)
    };

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if fenced[i] || !marker[i] {
                return line.to_string();
            }
            if block_marker_count[block_id[i]] >= 2 {
                return line.to_string();
            }
            let prev_marker = nearest_marker(Box::new((0..i).rev()));
            let next_marker = nearest_marker(Box::new((i + 1)..lines.len()));
            if prev_marker || next_marker {
                return line.to_string();
            }
            let marker_len = leading_marker_len(line);
            // A lone `1.` right under a heading is a one-note Footnotes
            // list, which the reader links `[1]` to; a stray citation dash
            // or mid-paragraph number never starts directly under one.
            if line.as_bytes()[0].is_ascii_digit()
                && (0..i)
                    .rev()
                    .find(|&j| !lines[j].trim().is_empty())
                    .is_some_and(|j| !fenced[j] && lines[j].starts_with('#'))
            {
                return line.to_string();
            }
            let (marker_text, rest) = line.split_at(marker_len);
            let last_char_len = marker_text.chars().last().map_or(0, char::len_utf8);
            format!(
                "{}\\{}{}",
                &marker_text[..marker_len - last_char_len],
                &marker_text[marker_len - last_char_len..],
                rest
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// True if `c` is one of CommonMark's backslash-escapable ASCII punctuation
/// characters. Shared with callers that need a different escaping strategy
/// per character than a uniform backslash (e.g. `ingest::augment`, which
/// escapes `<`/`>` as HTML entities instead) but still want "what counts as
/// escapable" defined in exactly one place.
pub fn is_commonmark_escapable(c: char) -> bool {
    matches!(
        c,
        '!' | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    )
}

/// Byte length of a leading CommonMark list-marker at the start of a
/// (trimmed, single-line) string — a bullet marker (`-`/`+`/`*` followed by
/// a space or end of line) or an ordered-list marker (digits followed by
/// `.` then a space or end of line). Zero if the line doesn't start with
/// one. Used to scope escaping of `-`/`.`/`+` to just this position — the
/// only place they're actually CommonMark-significant; mid-line they're
/// never ambiguous. The marker itself is always plain ASCII, so a byte
/// count here is also a valid char count for comparison against a char
/// index, regardless of any multi-byte Unicode later in the line.
fn leading_marker_len(line: &str) -> usize {
    let bytes = line.as_bytes();
    let followed_by_space_or_eol = |i: usize| bytes.get(i).is_none_or(|b| *b == b' ');

    if let Some(&first) = bytes.first() {
        if matches!(first, b'-' | b'+' | b'*') && followed_by_space_or_eol(1) {
            return 1;
        }
    }

    let digit_len = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digit_len > 0
        && bytes.get(digit_len) == Some(&b'.')
        && followed_by_space_or_eol(digit_len + 1)
    {
        return digit_len + 1;
    }

    0
}

/// Escape CommonMark backslash-escapable ASCII punctuation in a single line
/// (no paragraph/newline handling — see `from_plain_text`).
///
/// `-`/`.`/`+` are only escaped inside a leading list-marker (see
/// `leading_marker_len`) — elsewhere in the line they're never
/// CommonMark-significant, so escaping them there just clutters the stored
/// markdown with backslashes a reader never needed.
///
/// `,` is never escaped — it has no CommonMark meaning in any position, so
/// there's nothing to protect against.
///
/// `(`/`)` are only special together, as `](url)` immediately after a
/// `]` (link/image destination) — a bare `(...)` elsewhere in the line
/// (the common case for a plain-prose parenthetical) is never markup. So
/// `)` is never escaped, and `(` is escaped only when the immediately
/// preceding character is `]`. This matters beyond clutter: a document
/// built from `from_plain_text` has no real math in it, but unconditional
/// escaping here used to turn plain prose like `(We could have, but...)`
/// into `\(We could have\, but...\)` — indistinguishable from a genuine
/// `\(...\)` LaTeX inline-math span to `find_math_spans`'s plain
/// first-open/first-close scan, misrouting that sentence through math
/// handling instead of speaking it as plain text.
///
/// Every other escapable character (`*`, `_`, `` ` ``, `[`, `]`, `#`,
/// etc.) keeps escaping everywhere it appears, since those genuinely can
/// trigger emphasis/code/link/heading syntax from any position in the
/// line.
fn escape_markdown_chars(text: &str) -> String {
    let marker_len = leading_marker_len(text);
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    for (i, c) in text.chars().enumerate() {
        let escape = if (i >= marker_len && matches!(c, '-' | '.' | '+')) || matches!(c, ',' | ')')
        {
            false
        } else if c == '(' {
            prev == Some(']')
        } else {
            is_commonmark_escapable(c)
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Full list of HTML5 element names, one per line, alphabetized.
const HTML_TAGS: &str = include_str!("html_tags.txt");

/// True if `html` is an opening or closing tag for a known HTML element
/// (e.g. `<sup>`, `</b>`), as opposed to a non-tag `<...>` marker like
/// `<Ref-1>` that authors expect to be read literally.
fn is_known_html_tag(html: &str) -> bool {
    let inner = html
        .trim_start_matches('<')
        .trim_end_matches("/>")
        .trim_end_matches('>');
    let name = inner.strip_prefix('/').unwrap_or(inner);
    let name = name.split_whitespace().next().unwrap_or("");
    let name = name.to_ascii_lowercase();
    HTML_TAGS.lines().any(|tag| tag == name)
}

/// Convert markdown to plain text suitable for sentence splitting and TTS.
pub fn to_plain_text(markdown: &str) -> String {
    to_plain_text_with_math_overrides(markdown, &HashMap::new())
}

/// Same as `to_plain_text`, but `math_overrides` is passed through to every
/// math span's `math_to_speech::speak_with_overrides` call — see its doc
/// comment for the token/replacement shape. Empty map behaves exactly like
/// `to_plain_text`.
pub fn to_plain_text_with_math_overrides(
    markdown: &str,
    math_overrides: &HashMap<String, String>,
) -> String {
    let markdown = strip_silent(strip_frontmatter(markdown));
    let markdown = speak_math(&markdown, math_overrides);

    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_code_block = false;
    let mut in_image = false;

    // `blocks.len()` at the most recent `TableHead`/`TableRow` start — lets
    // the row-end handler tell whether this row actually produced a block
    // (an all-empty row wouldn't) before touching `blocks.last_mut()`, so it
    // never mistakenly punctuates a block left over from *before* the row
    // (unrelated prose, or a previous row).
    let mut table_row_start_len: usize = 0;

    // Without ENABLE_TABLES, pipe-table syntax is never recognized as a
    // table at all — it falls through to a single plain-paragraph Text
    // event carrying the raw `| cell | cell |` markup verbatim, which is
    // what actually got spoken by TTS (the `TagEnd::TableCell` arm below
    // otherwise never fires).
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);

    // Ordered-list item numbers are structural in pulldown-cmark
    // (`Tag::List(Some(start))`), never emitted as `Event::Text` — without
    // this stack the number is silently dropped from the spoken text, which
    // is wrong even for a plain top-level ordered list, not just the
    // nested-list edge case (a bullet item whose content starts with
    // `N. text`, which CommonMark parses as a nested ordered list). One
    // `Option<u64>` per list nesting level; `None` for an unordered list.
    let mut ordered_list_stack: Vec<Option<u64>> = Vec::new();
    // True from `Start(Item)` until the item's next event.
    let mut item_just_started = false;

    for event in Parser::new_ext(&markdown, options) {
        let at_item_start = std::mem::take(&mut item_just_started);
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::Image { .. }) => in_image = true,
            Event::End(TagEnd::Image) => in_image = false,
            Event::Start(Tag::List(start)) => {
                // A nested list inside a tight item (no blank line before
                // it) has no `End(Paragraph)` to end the parent item's
                // text, so without this the parent's last sentence and the
                // first nested item's text are glued with no space between
                // them ("…do.Each edge…"). Not when the nested list is the
                // item's first content (`- - x`, `1. - x`): there is no
                // parent text, and an ordered parent's number stays with
                // the first nested item.
                if !ordered_list_stack.is_empty() && !at_item_start {
                    push_block(&mut blocks, &current);
                    current.clear();
                }
                ordered_list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                ordered_list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                item_just_started = true;
                if let Some(Some(n)) = ordered_list_stack.last_mut() {
                    current.push_str(&n.to_string());
                    current.push_str(". ");
                    *n += 1;
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                table_row_start_len = blocks.len();
            }
            // A row boundary is a hard stop for TTS purposes — without
            // this, a caller that merges a paragraph not ending in a
            // finished sentence into the next (to group short headings
            // with their text) sees consecutive
            // cell lines with no terminal punctuation and glues an
            // entire table (headers, unrelated rows, even the
            // surrounding prose) into one incoherent run-on "sentence"
            // sent to the model as if it were continuous prose. A
            // period on the row's last cell is invisible in the
            // rendered table (that comes from `content`/markdown,
            // untouched here) but gives TTS a real per-row pause and
            // stops the merge at the row, not the whole table.
            Event::End(TagEnd::TableHead | TagEnd::TableRow)
                if blocks.len() > table_row_start_len =>
            {
                if let Some(last) = blocks.last_mut() {
                    if !matches!(last.chars().last(), Some('.' | '?' | '!')) {
                        last.push('.');
                    }
                }
            }
            Event::Text(text) | Event::Code(text) if !in_code_block && !in_image => {
                current.push_str(&text);
            }
            // pulldown_cmark treats any <...> as InlineHtml, including
            // non-tag markers like <Ref-1> that authors expect to be
            // read literally. Only drop text that names a real HTML
            // element; anything else falls through unchanged. An
            // inline comment (one event, even when it wraps across
            // lines) is never spoken: `split_for_export` drops it too,
            // and the rendered document hides it.
            Event::InlineHtml(html)
                if !in_image && !html.starts_with("<!--") && !is_known_html_tag(&html) =>
            {
                current.push_str(&html);
            }
            Event::SoftBreak | Event::HardBreak => {
                current.push(' ');
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableCell,
            ) => {
                push_block(&mut blocks, &current);
                current.clear();
            }
            _ => {}
        }
    }

    push_block(&mut blocks, &current);

    blocks.join("\n")
}

/// `markdown` without a leading YAML frontmatter block, which is metadata
/// (`title:`, `reader:`), not text to speak or show.
///
/// The block is recognized as pulldown-cmark recognizes a YAML metadata
/// block: the document's first line is `---` (after an optional byte-order
/// mark), the line after it is not blank, and the block ends at the next
/// line that is `---` or `...`. Without a closing line, nothing is
/// stripped. The not-blank rule keeps a document that opens with a
/// thematic break (`---` then a blank line) as it is.
///
/// Every function here that splits or renders a document strips it first,
/// so a document and the same document with its frontmatter removed have
/// the same sentences. Plain text through `from_plain_text` never has one:
/// its lines are joined with blank lines.
pub fn strip_frontmatter(markdown: &str) -> &str {
    let body = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let mut lines = body.split_inclusive('\n');
    let is_line = |line: &str, marker: &str| line.trim_end() == marker;
    let Some(first) = lines.next() else {
        return markdown;
    };
    if !first.ends_with('\n') || !is_line(first, "---") {
        return markdown;
    }
    let mut offset = first.len();
    for (i, line) in lines.enumerate() {
        if i == 0 && line.trim().is_empty() {
            return markdown;
        }
        offset += line.len();
        if is_line(line, "---") || is_line(line, "...") {
            return &body[offset..];
        }
    }
    markdown
}

/// Remove "silent" spans — bracketed text immediately followed by a
/// `<!--silent-->` comment, e.g. `[Doug Engelbart]<!--silent-->`. These are
/// displayed in the rendered document but excluded from speech synthesis.
///
/// Operates line by line: silent spans are stripped from each line, and a
/// line that became empty (or only heading `#` markers) *because* of the
/// stripping is dropped entirely — so a fully-silent heading produces no
/// spoken block. Originally-blank lines are preserved so paragraph
/// boundaries survive.
fn strip_silent(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let removed = remove_silent_spans(line);
        let trimmed = removed.trim();
        let emptied_by_strip =
            removed != line && (trimmed.is_empty() || trimmed.chars().all(|c| c == '#'));
        if emptied_by_strip {
            continue;
        }
        out.push(removed);
    }
    out.join("\n")
}

/// Remove every `[...]<!--silent-->` span from a single line, keeping any
/// other text. A `<!--silent-->` comment not preceded by a bracket span has
/// only the comment removed.
fn remove_silent_spans(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some((start, end)) = find_silent_comment(rest) {
        let before = &rest[..start];
        if before.ends_with(']') {
            if let Some(open) = before.rfind('[') {
                out.push_str(&before[..open]);
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

/// Find the next `<!--silent-->` comment (whitespace around `silent`
/// tolerated), returning its byte range `(start, end)` within `s`.
fn find_silent_comment(s: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(rel) = s[from..].find("<!--") {
        let start = from + rel;
        let inner_start = start + 4;
        match s[inner_start..].find("-->") {
            Some(erel) => {
                let inner_end = inner_start + erel;
                let end = inner_end + 3;
                if s[inner_start..inner_end].trim() == "silent" {
                    return Some((start, end));
                }
                from = end;
            }
            None => return None,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Math spans — `$...$`, `$$...$$`, `\(...\)`, `\[...\]`. Rendered visually
// by the caller (for example as a server-rendered image), but for
// TTS the LaTeX is phrased as spoken English via `math-to-speech` instead.
// Not a pulldown_cmark node type (no CommonMark math syntax), so this runs
// as a text-level pass before parsing, mirroring `strip_silent`'s call
// sites (`to_plain_text`, `split_for_export` both call it first).
// ---------------------------------------------------------------------------

/// Spoken in place of a math span whose LaTeX `math-to-speech` can't
/// phrase (unsupported construct, parse error) — e.g. a matrix or an
/// aligned-equation block. Deliberately a real, spoken placeholder rather
/// than silence: a paragraph whose *entire* content is one such span would
/// otherwise become blank text, and pulldown_cmark doesn't emit a
/// paragraph event for genuinely blank content — silently vanishing it
/// from `split_for_export`'s block count and desyncing every block after
/// it from what a renderer (which parses the original, un-stripped
/// markdown) still sees as one paragraph. A short spoken phrase keeps the
/// paragraph real on both sides, and tells the listener something was
/// skipped instead of leaving an unexplained gap.
const UNSPEAKABLE_MATH_PLACEHOLDER: &str = "mathematical formula";

/// Replace every math span found in `markdown` with its spoken-English
/// phrase, left to right — e.g. `$x^2$` becomes `x squared`. A span whose
/// LaTeX `math-to-speech` can't phrase becomes `UNSPEAKABLE_MATH_PLACEHOLDER`
/// instead (see its doc comment for why that can't just be silence).
pub fn speak_math(markdown: &str, math_overrides: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(span) = find_next_math_span(rest) {
        out.push_str(&rest[..span.start]);
        let phrase = math_to_speech::speak_with_overrides(&span.tex, math_overrides)
            .unwrap_or_else(|_| UNSPEAKABLE_MATH_PLACEHOLDER.to_string());
        // No space after an opening emphasis marker (`*\(y\) is ...*`):
        // `* y` can't open emphasis, so both markers would survive into the
        // plain text as literal asterisks for TTS to read.
        if !out.is_empty()
            && !out.ends_with(char::is_whitespace)
            && !ends_with_emphasis_opener(&out)
        {
            out.push(' ');
        }
        out.push_str(&phrase);
        // Only guard against actually gluing onto a following word (e.g.
        // `before$x$after` -> `before x after`) — inserting a space before
        // trailing punctuation instead produces an orphaned, space-
        // separated `.`/`,`/etc. (`x .` instead of `x.`), which multiple
        // independent TTS backends (misaki/Kokoro, VibeVoice and F5 —
        // confirmed against both, no shared pronunciation code between
        // them) render as the spoken word "point" instead of a silent
        // full stop. An ordinal suffix stays glued (`\(n\)th` -> `nth`, not
        // `n th`): a stranded "th" is spoken as its own odd word, while
        // "nth" reads as one word and "2nd"/"1th" become "second"/"first"
        // in the normalizer's ordinal pass.
        let after = &rest[span.end..];
        if after.chars().next().is_some_and(|c| c.is_alphanumeric())
            && !starts_with_ordinal_suffix(after)
        {
            out.push(' ');
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whether `s` ends with a run of `*`/`_`/`~` that opens emphasis or
/// strikethrough — i.e. the run is at the start of `s` or follows a
/// non-alphanumeric character (`as *`, `(**`). A run right after a letter or
/// digit (`x*`, `snake_`) isn't treated as an opener.
fn ends_with_emphasis_opener(s: &str) -> bool {
    let before = s.trim_end_matches(['*', '_', '~']);
    before.len() < s.len()
        && !before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric())
}

/// Whether `s` begins with `st`/`nd`/`rd`/`th` (any case) as a whole word —
/// `th` or `th sample` qualifies, `that` or `stuff` doesn't.
fn starts_with_ordinal_suffix(s: &str) -> bool {
    let Some(suffix) = s.get(..2) else {
        return false;
    };
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "st" | "nd" | "rd" | "th"
    ) && !s[2..].chars().next().is_some_and(|c| c.is_alphanumeric())
}

/// One math span found in source markdown — its byte range plus the
/// captured LaTeX and display/inline mode. `speak_math` needs the LaTeX to
/// phrase it for TTS; a caller rendering formulas to images also needs it,
/// via `find_math_spans` below.
struct MathSpanMatch {
    start: usize,
    end: usize,
    tex: String,
    display: bool,
}

/// The earliest math span (of any of the four delimiter forms) in `s`,
/// re-scanned from scratch each call — callers scan repeatedly on the
/// shrinking remainder, so each of the four scanners only ever needs to
/// find its own next occurrence independently, not coordinate with the
/// others beyond "whichever starts first wins."
fn find_next_math_span(s: &str) -> Option<MathSpanMatch> {
    let mut best: Option<MathSpanMatch> = None;
    let mut consider =
        |span: Option<(usize, usize)>, open_len: usize, close_len: usize, display: bool| {
            let Some((start, end)) = span else { return };
            if best.as_ref().map(|b| start < b.start).unwrap_or(true) {
                let tex = s[start + open_len..end - close_len].trim().to_string();
                best = Some(MathSpanMatch {
                    start,
                    end,
                    tex,
                    display,
                });
            }
        };
    consider(find_bracket_span(s, "\\[", "\\]"), 2, 2, true);
    consider(find_bracket_span(s, "\\(", "\\)"), 2, 2, false);
    consider(find_dollar_block_span(s), 2, 2, true);
    consider(find_dollar_inline_span(s), 1, 1, false);
    best
}

/// A math span with its byte range resolved to `markdown`'s own
/// coordinates (as opposed to `MathSpanMatch`'s range, which is relative
/// to whatever shrinking remainder `find_next_math_span` was last called
/// on) — what `find_math_spans` returns.
pub struct MathSpan {
    pub start: usize,
    pub end: usize,
    pub tex: String,
    pub display: bool,
}

/// Find every math span (`$...$`, `$$...$$`, `\(...\)`, `\[...\]`) in
/// `markdown`, in document order, with each one's LaTeX source and
/// display/inline mode. For a caller that renders each formula to an image
/// (e.g. an SVG at export time) and splices in a reference to it: that
/// needs the LaTeX pulled back out, rather than phrased for speech like
/// `speak_math` does for TTS. `linkify` also uses it, to leave math alone.
pub fn find_math_spans(markdown: &str) -> Vec<MathSpan> {
    let mut spans = Vec::new();
    let mut base = 0;
    let mut rest = markdown;
    while let Some(m) = find_next_math_span(rest) {
        spans.push(MathSpan {
            start: base + m.start,
            end: base + m.end,
            tex: m.tex,
            display: m.display,
        });
        base += m.end;
        rest = &rest[m.end..];
    }
    spans
}

/// `\[...\]` / `\(...\)` — unambiguous delimiters, no currency-style
/// collision risk, so a plain first-open/first-close scan is enough.
fn find_bracket_span(s: &str, open: &str, close: &str) -> Option<(usize, usize)> {
    let start = s.find(open)?;
    let after_open = start + open.len();
    let rel_end = s[after_open..].find(close)?;
    Some((start, after_open + rel_end + close.len()))
}

/// `$$...$$`, possibly spanning multiple lines (GitHub allows both a
/// single-line and a `$$` / content / `$$` multi-line form — both are just
/// "first `$$`, then the next `$$`" once the surrounding line structure
/// doesn't matter to a plain-text strip).
fn find_dollar_block_span(s: &str) -> Option<(usize, usize)> {
    let start = s.find("$$")?;
    let after_open = start + 2;
    let rel_end = s[after_open..].find("$$")?;
    Some((start, after_open + rel_end + 2))
}

/// `$...$` — GitHub's actual rule: the opening `$` must be immediately
/// followed by non-whitespace, the closing `$` immediately preceded by
/// non-whitespace, with no blank line in between. Deliberately inherits
/// GitHub's own known ambiguity (`$5 and $10` can misparse as inline math)
/// rather than trying to out-guess it — matching, not fixing, upstream
/// behavior. A `$` that's actually part of a `$$` block marker (checked via
/// its neighbors) is never treated as an inline delimiter.
fn find_dollar_inline_span(s: &str) -> Option<(usize, usize)> {
    fn is_part_of_double(s: &str, pos: usize) -> bool {
        s[pos..].starts_with("$$") || (pos > 0 && s[..pos].ends_with('$'))
    }

    let mut i = 0;
    while let Some(rel) = s[i..].find('$') {
        let start = i + rel;
        if is_part_of_double(s, start) {
            i = start + 1;
            continue;
        }
        let opens_on_nonwhitespace = s[start + 1..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace());
        if !opens_on_nonwhitespace {
            i = start + 1;
            continue;
        }

        let mut j = start + 1;
        loop {
            let rel2 = s[j..].find('$')?;
            let close = j + rel2;
            if is_part_of_double(s, close) {
                j = close + 1;
                continue;
            }
            let closes_on_nonwhitespace = s[..close]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace());
            let spans_blank_line = s[start + 1..close].contains("\n\n");
            if closes_on_nonwhitespace && !spans_blank_line {
                return Some((start, close + 1));
            }
            j = close + 1;
        }
    }
    None
}

/// Trim and collapse internal whitespace runs (left by dropped inline
/// elements like images) before pushing a finished block.
fn push_block(blocks: &mut Vec<String>, text: &str) {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        blocks.push(collapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_a_leading_yaml_block() {
        assert_eq!(
            strip_frontmatter("---\nreader: Jane Doe\n---\n# Hi\n\nText."),
            "# Hi\n\nText."
        );
        assert_eq!(
            strip_frontmatter("\u{feff}---\r\ntitle: A\r\n...\r\nText."),
            "Text."
        );
        assert_eq!(strip_frontmatter("---\n---\nText."), "Text.");
        assert_eq!(strip_frontmatter("---\nreader: Jane Doe\n---"), "");
    }

    #[test]
    fn strip_frontmatter_leaves_other_documents_alone() {
        for md in [
            "Text.\n\n---\nreader: X\n---\n",       // not at the top
            "---\n\nPara one.\n\n---\n\nPara two.", // a thematic break: blank line after
            "---\nreader: Jane Doe\n\nNo closing line.",
            "---",
            " ---\nreader: X\n---\nText.", // indented opener
            "",
        ] {
            assert_eq!(strip_frontmatter(md), md, "{md:?}");
        }
    }

    #[test]
    fn frontmatter_is_not_spoken() {
        let body = "# Talk\n\nOne here. Two here.\n\n- Three.\n";
        let with = format!("---\ntitle: Talk\nreader: Jane Doe\n---\n{body}");
        assert_eq!(to_plain_text(&with), to_plain_text(body));
        assert!(!to_plain_text(&with).contains("Jane"));
        assert_eq!(split_for_export(&with), split_for_export(body));
    }

    #[test]
    fn plain_text_with_a_dash_line_keeps_it() {
        let markdown = from_plain_text("---\nreader: Jane Doe\n---\nText.");
        assert_eq!(strip_frontmatter(&markdown), markdown);
        assert!(to_plain_text(&markdown).contains("Jane Doe"));
    }

    #[test]
    fn from_plain_text_round_trips_a_single_paragraph() {
        let plain = "This is a footnote marker.*1* Next sentence.";
        let markdown = from_plain_text(plain);
        assert_eq!(to_plain_text(&markdown), plain);
    }

    #[test]
    fn from_plain_text_separates_paragraphs_with_a_blank_line() {
        let plain = "Para one.\nPara two.";
        // Trailing periods aren't a leading list-marker position, so they're
        // left unescaped (see escape_markdown_chars_only_escapes_leading_markers).
        assert_eq!(from_plain_text(plain), "Para one.\n\nPara two.");
        assert_eq!(
            to_plain_text(&from_plain_text(plain)),
            "Para one.\nPara two."
        );
    }

    #[test]
    fn from_plain_text_trims_and_drops_blank_lines() {
        let plain = "  Para one.  \n\n\n  Para two.  ";
        assert_eq!(from_plain_text(plain), "Para one.\n\nPara two.");
    }

    // ── trim_line_indentation ────────────────────────────────────────────

    #[test]
    fn trim_line_indentation_strips_stray_leading_spaces() {
        let raw = "    **Bio**:\nSome text.";
        assert_eq!(trim_line_indentation(raw), "**Bio**:\nSome text.");
    }

    #[test]
    fn trim_line_indentation_preserves_indentation_inside_fenced_code() {
        let raw = "```\n    def f():\n        return 1\n```";
        assert_eq!(
            trim_line_indentation(raw),
            "```\n    def f():\n        return 1\n```"
        );
    }

    #[test]
    fn trim_line_indentation_trims_outside_fence_and_preserves_inside() {
        let raw = "  Intro line.\n```\n    indented code\n```\n  Outro line.";
        assert_eq!(
            trim_line_indentation(raw),
            "Intro line.\n```\n    indented code\n```\nOutro line."
        );
    }

    #[test]
    fn trim_line_indentation_keeps_nested_list_item_indentation() {
        let raw = "1. Parent.\n   1. Child.\n      - Grandchild.\n2. Next.";
        assert_eq!(trim_line_indentation(raw), raw);
    }

    #[test]
    fn trim_line_indentation_strips_indented_item_after_non_item() {
        let raw = "Intro.\n    - Item.\n\n    - Loose item.";
        assert_eq!(
            trim_line_indentation(raw),
            "Intro.\n- Item.\n\n- Loose item."
        );
    }

    // ── close_unclosed_table_rows ───────────────────────────────────────

    #[test]
    fn drop_empty_sections_removes_pubmed_footer() {
        let md = "## Similar articles\n\n- One.\n- Two.\n\n## Publication types\n\n## MeSH terms\n\n\
                  ## LinkOut - more resources\n\n- ### Full Text Sources\n- ### Other Literature Sources\n\
                  - ### Medical\n- ### Miscellaneous\n";
        assert_eq!(
            drop_empty_sections(md),
            "## Similar articles\n\n- One.\n- Two.\n"
        );
    }

    #[test]
    fn drop_empty_sections_keeps_sections_with_content_or_subsections() {
        let md = "# Title\n\n## Part 1\n\n### Chapter\n\nText.\n\n## Part 2\n\nMore.\n";
        assert_eq!(drop_empty_sections(md), md);
        let continued = "## Links\n\n- ### Sources\n  A real item.\n";
        assert_eq!(drop_empty_sections(continued), continued);
        let mixed = "## Links\n\n- ### Sources\n- Plain item.\n";
        assert_eq!(drop_empty_sections(mixed), mixed);
    }

    #[test]
    fn drop_empty_sections_drops_a_heading_before_a_same_level_one() {
        assert_eq!(
            drop_empty_sections("## A\n\n## B\n\nText.\n"),
            "## B\n\nText.\n"
        );
        assert_eq!(drop_empty_sections("Text.\n\n## End\n"), "Text.\n");
    }

    #[test]
    fn drop_empty_sections_leaves_fenced_code_alone() {
        let md = "Text.\n\n```\n## not a heading\n\n\n## nor this\n```\n";
        assert_eq!(drop_empty_sections(md), md);
    }

    #[test]
    fn close_unclosed_table_rows_appends_missing_trailing_pipe() {
        let raw = "| A | B | |\n|---|---|---|\n| 1 | x | |\n| 2 | y | ";
        assert_eq!(
            close_unclosed_table_rows(raw),
            "| A | B | |\n|---|---|---|\n| 1 | x | |\n| 2 | y | |"
        );
    }

    #[test]
    fn close_unclosed_table_rows_leaves_already_closed_rows_alone() {
        let raw = "| A | B |\n|---|---|\n| 1 | x |\n| 2 | y |";
        assert_eq!(close_unclosed_table_rows(raw), raw);
    }

    #[test]
    fn close_unclosed_table_rows_preserves_fenced_code() {
        let raw = "```\n| not a table row\n```";
        assert_eq!(close_unclosed_table_rows(raw), raw);
    }

    // ── split_glued_table_header ────────────────────────────────────────

    #[test]
    fn split_glued_table_header_separates_header_from_paragraph() {
        let raw = "changes in rCBF.| Analysis | Location | |\n|---|---|---|\n| x | y | |";
        assert_eq!(
            split_glued_table_header(raw),
            "changes in rCBF.\n\n| Analysis | Location | |\n|---|---|---|\n| x | y | |"
        );
    }

    #[test]
    fn split_glued_table_header_keeps_stray_prefix_as_own_paragraph() {
        let raw =
            "j| Age of onset | Men | Women | ||||\n|---|---|---|---|---|---|---|\n| Naive | |";
        assert_eq!(
            split_glued_table_header(raw),
            "j\n\n| Age of onset | Men | Women | ||||\n|---|---|---|---|---|---|---|\n| Naive | |"
        );
    }

    #[test]
    fn split_glued_table_header_leaves_prose_with_pipe_alone() {
        let raw = "use a | b here\n\n| A | B |\n|---|---|";
        assert_eq!(split_glued_table_header(raw), raw);
    }

    #[test]
    fn split_glued_table_header_leaves_valid_tables_alone() {
        let raw = "intro\n\n| A | B |\n|---|---|\n| 1 | x |";
        assert_eq!(split_glued_table_header(raw), raw);
    }

    #[test]
    fn split_glued_table_header_preserves_fenced_code() {
        let raw = "```\nfoo | a | b |\n|---|---|---|\n```";
        assert_eq!(split_glued_table_header(raw), raw);
    }

    // ── insert_missing_table_separator ──────────────────────────────────

    #[test]
    fn insert_missing_table_separator_adds_separator_after_header() {
        let raw = "| PAGE | |\n| A VOYAGE TO LILLIPUT | |\n| CHAPTER I | |";
        assert_eq!(
            insert_missing_table_separator(raw),
            "| PAGE | |\n|---|---|\n| A VOYAGE TO LILLIPUT | |\n| CHAPTER I | |"
        );
    }

    #[test]
    fn insert_missing_table_separator_leaves_valid_tables_alone() {
        let raw = "| A | B |\n|---|---|\n| 1 | x |\n| 2 | y |";
        assert_eq!(insert_missing_table_separator(raw), raw);
    }

    #[test]
    fn insert_missing_table_separator_ignores_single_row() {
        let raw = "Intro.\n\n| solo row |\n\nMore text.";
        assert_eq!(insert_missing_table_separator(raw), raw);
    }

    #[test]
    fn insert_missing_table_separator_preserves_fenced_code() {
        let raw = "```\n| not a table\n| still not\n```";
        assert_eq!(insert_missing_table_separator(raw), raw);
    }

    // ── escape_isolated_list_markers ────────────────────────────────────

    #[test]
    fn escape_isolated_list_markers_escapes_lone_leading_dash() {
        let raw = "Some quote (2006)\n\n- Tai Amri Spann-Ryan from \u{201c}The Need\u{201d}\n\nMore text.";
        assert_eq!(
            escape_isolated_list_markers(raw),
            "Some quote (2006)\n\n\\- Tai Amri Spann-Ryan from \u{201c}The Need\u{201d}\n\nMore text."
        );
    }

    #[test]
    fn escape_isolated_list_markers_leaves_real_multi_item_list_alone() {
        let raw = "Intro.\n\n- First item\n- Second item\n- Third item";
        assert_eq!(escape_isolated_list_markers(raw), raw);
    }

    // A tight list item's citation text can soft-wrap onto a continuation
    // line with no marker of its own (e.g. a reference-list `<li>` whose
    // journal/volume/DOI line comes right after, no blank line) — the
    // marker lines aren't adjacent to each other, but they're still all in
    // the same blank-line-delimited block, so this must stay a real list.
    #[test]
    fn escape_isolated_list_markers_leaves_wrapped_list_items_alone() {
        let raw = "- Apoux F. (2004). Title one.\n*J. Acoust.*116 1671-1680 [DOI]\n- Badri R. (2011). Title two.\n*J. Acoust.*129 852-863 [DOI]";
        assert_eq!(escape_isolated_list_markers(raw), raw);
    }

    #[test]
    fn escape_isolated_list_markers_leaves_loose_real_list_alone() {
        let raw = "Intro.\n\n- First item\n\n- Second item";
        assert_eq!(escape_isolated_list_markers(raw), raw);
    }

    #[test]
    fn escape_isolated_list_markers_escapes_isolated_ordered_marker() {
        let raw = "Para one.\n\n1. Not actually a list, just a sentence.\n\nPara two.";
        assert_eq!(
            escape_isolated_list_markers(raw),
            "Para one.\n\n1\\. Not actually a list, just a sentence.\n\nPara two."
        );
    }

    #[test]
    fn escape_isolated_list_markers_keeps_lone_numbered_item_under_heading() {
        let raw = "## Footnotes\n\n1. The only note.\n\nPara.";
        assert_eq!(escape_isolated_list_markers(raw), raw);
    }

    #[test]
    fn escape_isolated_list_markers_preserves_fenced_code() {
        let raw = "```\n- not a list, code\n```";
        assert_eq!(escape_isolated_list_markers(raw), raw);
    }

    // ── escape_literal_angle_brackets ───────────────────────────────────

    #[test]
    fn escape_literal_angle_brackets_escapes_bare_tag_like_text() {
        let raw = "presses the <Select> button.";
        assert_eq!(
            escape_literal_angle_brackets(raw),
            "presses the \\<Select\\> button."
        );
    }

    #[test]
    fn escape_literal_angle_brackets_preserves_fenced_code() {
        let raw = "```\nVec<T>\n```";
        assert_eq!(escape_literal_angle_brackets(raw), "```\nVec<T>\n```");
    }

    #[test]
    fn escape_literal_angle_brackets_preserves_inline_code_span() {
        let raw = "Use `Vec<T>` here, but <Select> is literal.";
        assert_eq!(
            escape_literal_angle_brackets(raw),
            "Use `Vec<T>` here, but \\<Select\\> is literal."
        );
    }

    #[test]
    fn escape_literal_angle_brackets_preserves_blockquote_marker() {
        let raw = "> a quote with <Select> inside";
        assert_eq!(
            escape_literal_angle_brackets(raw),
            "> a quote with \\<Select\\> inside"
        );
    }

    // A real bug: `<`/`>` inside math are comparison operators, not
    // literal tag-like text — escaping them broke mitex's LaTeX parsing
    // (`\<` isn't a valid command) and the formula failed to render.
    #[test]
    fn escape_literal_angle_brackets_leaves_comparison_operators_in_math_alone() {
        let raw = r"We require \(t < 0\) and \(t > 1\).";
        assert_eq!(escape_literal_angle_brackets(raw), raw);
    }

    #[test]
    fn escape_literal_angle_brackets_leaves_dollar_math_alone_too() {
        let raw = r"We require $t < 0$ here.";
        assert_eq!(escape_literal_angle_brackets(raw), raw);
    }

    #[test]
    fn escape_literal_angle_brackets_still_escapes_outside_math() {
        let raw = r"Use \(x\) not <Select> for this.";
        assert_eq!(
            escape_literal_angle_brackets(raw),
            r"Use \(x\) not \<Select\> for this."
        );
    }

    // ── split_markdown_sentences_for_render ─────────────────────────────

    #[test]
    fn split_markdown_sentences_no_straddling_is_unchanged() {
        let raw = "A *no straddle* sentence. Another plain one.";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(
            chunks,
            vec!["A *no straddle* sentence.", "Another plain one."]
        );
    }

    // Real reported bug: a poem (or anything else) with a hard break
    // between two real sentences — collapsing the break straight to a
    // literal "<br>" (tried first) confused speakable_splitter::split's own
    // sentence-boundary detection into merging what to_plain_text (which
    // strips <br> and collapses the break to a plain space) correctly
    // sees as two sentences — the count mismatch this function's caller
    // depends on to agree with the plain-text split, silently falling
    // back to unformatted plain text and dropping the hard break (and any
    // other formatting in the block) entirely. See
    // HARD_BREAK_PLACEHOLDER's own comment for the fix.
    #[test]
    fn split_markdown_sentences_hard_break_between_two_sentences_does_not_merge_them() {
        let raw = "Roses are red.  \nViolets are blue.";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(
            plain_count, 2,
            "sanity check: to_plain_text sees this as two sentences"
        );
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        // The marker (and so the restored <br>) lands at the start of the
        // second chunk, not the end of the first — the sentence boundary
        // itself falls right before the separating space, which gets
        // trimmed off as the next sentence's own leading whitespace.
        // Harmless either way: a renderer that joins adjacent sentence
        // spans with their own space text node in the DOM shows the same.
        assert_eq!(chunks, vec!["Roses are red.", "<br>Violets are blue."]);
    }

    #[test]
    fn split_markdown_sentences_backslash_hard_break_between_two_sentences_does_not_merge_them() {
        let raw = "Roses are red.\\\nViolets are blue.";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(chunks, vec!["Roses are red.", "<br>Violets are blue."]);
    }

    #[test]
    fn split_markdown_sentences_closes_and_reopens_a_straddling_span() {
        let raw = "*Hello world. Goodbye world.*";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(chunks, vec!["*Hello world.*", "*Goodbye world.*"]);
    }

    #[test]
    fn split_markdown_sentences_does_not_duplicate_an_intact_closing_delimiter() {
        // Reported bug: the splitter puts the boundary right after "1982."
        // and before the closing "*", so the delimiter lands intact as the
        // first character of the next sentence rather than being torn in
        // half. The old logic still "reopened" a synthetic "*" in front of
        // it, producing a stray "**" instead of a single "*".
        let raw = "[*Giffen, Holly*. \u{201c}Coordination of Meaning in Shared \
                   Make-Believe\u{201d} *in* Symbolic Play: The Development of Social \
                   Understanding, *ed. Inge Bretherton. Orlando, FL: Academic Press, \
                   1982.*] What she was in fact observing was something else.";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        for chunk in &chunks {
            assert!(
                !chunk.contains("**"),
                "unexpected doubled delimiter in {chunk:?}"
            );
        }
        assert_eq!(
            chunks,
            vec![
                "[*Giffen, Holly*.",
                "\u{201c}Coordination of Meaning in Shared Make-Believe\u{201d} *in* \
                 Symbolic Play: The Development of Social Understanding, *ed.*",
                "*Inge Bretherton.*",
                "*Orlando, FL: Academic Press, 1982.*",
                "] What she was in fact observing was something else.",
            ]
        );
    }

    // Real reported bug: `\(*\)` (math, spoken "asterisk") sitting near a
    // genuine `*convolution*` emphasis span in the same paragraph. Before
    // the fix, `pulldown_cmark` (which has no notion of `\(...\)` math
    // syntax) mistook the first `\(*\)`'s bare `*` for a real emphasis
    // delimiter, paired it across sentence boundaries with the real
    // `*convolution*` span, and corrupted both: the second `\(*\)` lost
    // its asterisk to a bogus reopened `<em>`, rendering as a bare `()`.
    #[test]
    fn split_markdown_sentences_ignores_bare_asterisk_inside_math_span() {
        let raw = r"the \(*\) operator will be used later, known as *convolution*. \
                     Using \(\cdot\) instead of \(*\) for multiplication avoids confusion.";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        // Every chunk still contains exactly the math/emphasis markers
        // that were actually in the source at that position — no asterisk
        // stolen into a synthetic close/reopen.
        assert!(
            chunks[0].contains(r"\(*\)"),
            "first \\(*\\) intact: {:?}",
            chunks[0]
        );
        assert!(
            chunks[0].contains("*convolution*"),
            "emphasis intact: {:?}",
            chunks[0]
        );
        assert!(
            chunks[1].contains(r"\(*\)"),
            "second \\(*\\) intact: {:?}",
            chunks[1]
        );
        assert!(
            !chunks[1].starts_with('*'),
            "no bogus reopened emphasis: {:?}",
            chunks[1]
        );
    }

    #[test]
    fn split_markdown_sentences_reopens_across_two_sentences() {
        // The real reported bug: a citation whose italics run through a
        // sentence-splitter boundary. "Vols." is now a protected
        // abbreviation (a renderer running this crate through wasm shares
        // the same splitter, so there's no client/server drift risk in
        // keeping "Vols. 7 & 8" merged with what follows), so the only
        // remaining boundary is after "7:326."
        let raw = "*[ George Fox,* A Collection, Letters, and Testimonies, \
                   *published as part of *The Works of George Fox*,* \
                   *Vols. 7 & 8, 7:326. Hereafter citations will be listed by Fox, \
                   *Works*, volume #, and page.**]*";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(chunks.len(), 2);
        // Each chunk parses as self-contained, correctly-nested emphasis —
        // no stray unpaired asterisks — confirmed by rendering each one
        // independently exactly as marked.parseInline() would client-side.
        for chunk in &chunks {
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, Parser::new(chunk));
            assert!(
                html.contains("<em>"),
                "expected {chunk:?} to render as emphasis, got {html:?}"
            );
        }
        assert!(chunks[0].ends_with("*Vols. 7 & 8, 7:326.*"));
        assert!(chunks[1].starts_with("*Hereafter"));
    }

    #[test]
    fn split_markdown_sentences_nested_spans_close_innermost_reopen_outermost() {
        let raw = "*Alpha starts here **Bravo continues onward. \
                   Charlie finishes eventually** Delta wraps things up.*";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(
            chunks,
            vec![
                "*Alpha starts here **Bravo continues onward.***",
                "***Charlie finishes eventually** Delta wraps things up.*",
            ]
        );

        // Confirm both chunks independently parse to the correctly nested
        // structure, not just that the delimiter counts happen to match.
        let mut html0 = String::new();
        pulldown_cmark::html::push_html(&mut html0, Parser::new(&chunks[0]));
        assert_eq!(
            html0,
            "<p><em>Alpha starts here <strong>Bravo continues onward.</strong></em></p>\n"
        );

        let mut html1 = String::new();
        pulldown_cmark::html::push_html(&mut html1, Parser::new(&chunks[1]));
        assert_eq!(
            html1,
            "<p><em><strong>Charlie finishes eventually</strong> Delta wraps things up.</em></p>\n"
        );
    }

    #[test]
    fn split_markdown_sentences_returns_none_on_count_mismatch() {
        let raw = "Some text here.";
        assert_eq!(split_markdown_sentences_for_render(raw, 5), None);
    }

    #[test]
    fn split_markdown_sentences_closes_and_reopens_a_straddling_link() {
        let raw = "[Hello world. Goodbye world.](https://example.com \"Example\")";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(
            chunks,
            vec![
                "[Hello world.](https://example.com \"Example\")",
                "[Goodbye world.](https://example.com \"Example\")",
            ]
        );
        for chunk in &chunks {
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, Parser::new(chunk));
            assert!(
                html.contains("<a href=\"https://example.com\""),
                "expected {chunk:?} to render as a link, got {html:?}"
            );
        }
    }

    #[test]
    fn split_markdown_sentences_closes_and_reopens_a_link_spanning_three_sentences() {
        let raw = "[One. Two. Three.](https://example.com \"Example\")";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 3);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(
            chunks,
            vec![
                "[One.](https://example.com \"Example\")",
                "[Two.](https://example.com \"Example\")",
                "[Three.](https://example.com \"Example\")",
            ]
        );
    }

    #[test]
    fn split_markdown_sentences_link_and_emphasis_straddle_the_same_boundary() {
        let raw = "*[Hello world. Goodbye world.](https://example.com \"Example\")*";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 2);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(
            chunks,
            vec![
                "*[Hello world.](https://example.com \"Example\")*",
                "*[Goodbye world.](https://example.com \"Example\")*",
            ]
        );
        for chunk in &chunks {
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, Parser::new(chunk));
            assert!(
                html.contains("<em><a href=\"https://example.com\""),
                "expected {chunk:?} to render as an italicized link, got {html:?}"
            );
        }
    }

    #[test]
    fn split_markdown_sentences_single_sentence_link_is_unchanged() {
        let raw = "[Hello world.](https://example.com \"Example\")";
        let plain_count = speakable_splitter::split(&to_plain_text(raw)).len();
        assert_eq!(plain_count, 1);
        let chunks = split_markdown_sentences_for_render(raw, plain_count).unwrap();
        assert_eq!(chunks, vec![raw]);
    }

    #[test]
    fn push_export_block_balances_a_straddling_span() {
        // End-to-end through the actual export entry point, not just the
        // helper directly — this is what the export reader relies on.
        let markdown = "*[ George Fox,* A Collection, Letters, and Testimonies, \
                        *published as part of *The Works of George Fox*,* \
                        *Vols. 7 & 8, 7:326. Hereafter citations will be listed by Fox, \
                        *Works*, volume #, and page.**]*";
        let (sentences, block_lengths) = split_for_export(markdown);
        assert_eq!(block_lengths, vec![2]);
        assert_eq!(sentences.len(), 2);
        for s in &sentences {
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, Parser::new(&s.markdown_text));
            assert!(
                html.contains("<em>"),
                "expected {:?} to render as emphasis, got {html:?}",
                s.markdown_text
            );
        }
    }

    #[test]
    fn escape_markdown_chars_escapes_most_commonmark_special_chars_anywhere() {
        // -, ., + are excluded (mid-line, not a leading marker) — see the
        // dedicated leading-marker tests below. `(` here follows `f`, not
        // `]`, so it (and `)`) stay unescaped too — see the dedicated
        // paren/comma tests below.
        let plain = "a*b_c`d[e]f(g)h#i+j-k.l!m";
        assert_eq!(
            escape_markdown_chars(plain),
            "a\\*b\\_c\\`d\\[e\\]f(g)h\\#i+j-k.l\\!m"
        );
    }

    #[test]
    fn escape_markdown_chars_leaves_mid_line_hyphen_dot_plus_unescaped() {
        assert_eq!(escape_markdown_chars("word - word"), "word - word");
        assert_eq!(escape_markdown_chars("word. word"), "word. word");
        assert_eq!(escape_markdown_chars("word + word"), "word + word");
    }

    #[test]
    fn escape_markdown_chars_leaves_plain_parens_and_commas_unescaped() {
        assert_eq!(
            escape_markdown_chars("(We could have, but it would have been tedious.)"),
            "(We could have, but it would have been tedious.)"
        );
    }

    #[test]
    fn escape_markdown_chars_escapes_open_paren_only_right_after_close_bracket() {
        // `](` would otherwise form a link/image destination.
        assert_eq!(
            escape_markdown_chars("see [ref](url) here"),
            "see \\[ref\\]\\(url) here"
        );
        // A space (or anything else) between `]` and `(` isn't link syntax,
        // so `(` stays unescaped.
        assert_eq!(
            escape_markdown_chars("see [ref] (url) here"),
            "see \\[ref\\] (url) here"
        );
    }

    #[test]
    fn escape_markdown_chars_escapes_leading_bullet_marker() {
        assert_eq!(escape_markdown_chars("- item"), "\\- item");
        assert_eq!(escape_markdown_chars("+ item"), "\\+ item");
        assert_eq!(escape_markdown_chars("* item"), "\\* item");
        // Not a marker without a following space — hyphenated word, left alone.
        assert_eq!(escape_markdown_chars("-item"), "-item");
    }

    #[test]
    fn escape_markdown_chars_escapes_leading_ordered_marker() {
        assert_eq!(
            escape_markdown_chars("1. Introduction"),
            "1\\. Introduction"
        );
        assert_eq!(escape_markdown_chars("42. Answer"), "42\\. Answer");
        // Not a marker without a following space — a bare decimal number.
        assert_eq!(escape_markdown_chars("3.14 is pi"), "3.14 is pi");
    }

    #[test]
    fn strips_bold_italic_inline_code() {
        let md = "This is **bold**, *italic*, and `code`.";
        assert_eq!(to_plain_text(md), "This is bold, italic, and code.");
    }

    #[test]
    fn reduces_links_to_text() {
        let md = "See [this reference](https://example.com) for details.";
        assert_eq!(to_plain_text(md), "See this reference for details.");
    }

    #[test]
    fn to_plain_text_keeps_ordered_list_numbers() {
        let md = "1. First item\n2. Second item\n3. Third item\n";
        assert_eq!(
            to_plain_text(md),
            "1. First item\n2. Second item\n3. Third item"
        );
    }

    // A bullet item whose content itself starts with "N. " (a common
    // reference-list authoring style) parses as a bullet list containing a
    // nested ordered list starting at N, not as literal text — the number
    // is structural in pulldown-cmark and must be re-emitted explicitly.
    #[test]
    fn to_plain_text_keeps_numbers_from_bulleted_reference_list() {
        let md = "- 1. Sakai F, Igarashi H. Prevalence of migraine in Japan.\n- 2. Noseda R, Kainz V. A neural mechanism.\n";
        assert_eq!(
            to_plain_text(md),
            "1. Sakai F, Igarashi H. Prevalence of migraine in Japan.\n2. Noseda R, Kainz V. A neural mechanism."
        );
    }

    #[test]
    fn to_plain_text_leaves_ordinary_quoted_link_text_alone() {
        let md = "He said [\"hello\"](https://example.com).";
        assert_eq!(to_plain_text(md), "He said \"hello\".");
    }

    #[test]
    fn joins_paragraphs_with_blank_line() {
        let md = "First paragraph.\n\nSecond paragraph.";
        assert_eq!(to_plain_text(md), "First paragraph.\nSecond paragraph.");
    }

    #[test]
    fn includes_headings_and_list_items() {
        let md = "# Title\n\n- one\n- two";
        assert_eq!(to_plain_text(md), "Title\none\ntwo");
    }

    #[test]
    fn skips_fenced_code_blocks() {
        let md = "Before.\n\n```\nlet x = 1;\n```\n\nAfter.";
        assert_eq!(to_plain_text(md), "Before.\nAfter.");
    }

    #[test]
    fn skips_images() {
        let md = "Look: ![alt text](pic.png) done.";
        assert_eq!(to_plain_text(md), "Look: done.");
    }

    // A standalone image produces an empty plain-text block, so
    // `split_for_export` pushes a `0` (not omitted) `block_lengths` entry
    // for it — this is load-bearing for rendering, not just TTS: a
    // renderer relies on the block contributing zero sentences to know it
    // should render the image directly rather than calling into
    // sentence-span weaving.
    #[test]
    fn split_for_export_skips_standalone_image_block() {
        let md = "![a cat](https://example.com/pic.jpg)";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(sentences, vec![]);
        assert_eq!(block_lengths, vec![0]);
    }

    fn export_markdown_texts(md: &str) -> Vec<String> {
        split_for_export(md)
            .0
            .into_iter()
            .map(|s| s.markdown_text)
            .collect()
    }

    // The export reader draws a list's own bullet/number, so an item's
    // `markdown_text` must not repeat its marker.
    #[test]
    fn split_for_export_tight_list_items_exclude_marker() {
        assert_eq!(
            export_markdown_texts("1. Note one.\n2. Note two. Second sentence.\n\n- Bullet.\n* Star.\n+ Plus.\n3) Paren."),
            vec!["Note one.", "Note two.", "Second sentence.", "Bullet.", "Star.", "Plus.", "Paren."],
        );
    }

    #[test]
    fn split_for_export_loose_list_items_exclude_marker() {
        assert_eq!(
            export_markdown_texts("1. Note one.\n\n2. Note two."),
            vec!["Note one.", "Note two."]
        );
    }

    #[test]
    fn split_for_export_list_item_keeps_leading_emphasis_and_links() {
        assert_eq!(
            export_markdown_texts("- *Emphasized* start.\n- [A link](https://example.com) start."),
            vec![
                "*Emphasized* start.",
                "[A link](https://example.com) start."
            ],
        );
    }

    #[test]
    fn split_for_export_nested_list_items_exclude_marker() {
        for text in export_markdown_texts("- Outer.\n  - Inner.") {
            assert!(!text.starts_with('-'), "{text:?}");
        }
    }

    // The spoken text keeps an ordered item's number, as `to_plain_text`
    // does, while `markdown_text` leaves it out.
    #[test]
    fn split_for_export_list_item_spoken_text_keeps_number() {
        let (sentences, block_lengths) = split_for_export("1. Note one.\n2. Note two.");
        let texts: Vec<_> = sentences.into_iter().map(|s| s.text).collect();
        assert_eq!(texts, vec!["1. Note one.", "2. Note two."]);
        assert_eq!(block_lengths, vec![1, 1]);
    }

    // The stored sentences come from `to_plain_text`; `split_for_export`
    // must produce the same spoken sentences or the export falls back to a
    // single block for the whole document.
    #[test]
    fn split_for_export_matches_to_plain_text_for_ordered_lists() {
        for md in [
            "Intro:\n\n1. First item.\n2. Second item. Two sentences.\n",
            "Intro:\n\n- 1.\n- First item;\n- 2.\n- Second item;\n",
            "- 1. Sakai F, Igarashi H. Prevalence.\n- 2. Noseda R. A mechanism.\n",
            "## Footnotes\n\n1. Most of this is derived from Yost (2000).\n",
        ] {
            let exported: Vec<String> =
                split_for_export(md).0.into_iter().map(|s| s.text).collect();
            let stored: Vec<String> = speakable_splitter::split(&to_plain_text(md))
                .into_iter()
                .map(|s| s.text)
                .collect();
            assert_eq!(exported, stored, "{md:?}");
        }
    }

    const NESTED_LIST_MDS: [&str; 6] = [
        "- **Cut:** a fixed cut where Kokoro holds energy.\n  - Each edge eases in over half an octave.\n- Next item.\n",
        "- Parent one.\n  - Child one.\n  - Child two.\n    - Grandchild.\n- Parent two.\n",
        "- Loose parent.\n\n  - Loose child.\n\n- Loose next.\n",
        "1. Ordered parent.\n   - Child.\n2. Second parent.\n",
        "- - Nested first.\n- Plain.\n",
        "1. - Nested first under a number.\n2. Plain.\n",
    ];

    #[test]
    fn split_for_export_matches_to_plain_text_for_nested_lists() {
        for md in NESTED_LIST_MDS {
            let exported: Vec<String> =
                split_for_export(md).0.into_iter().map(|s| s.text).collect();
            let stored: Vec<String> = speakable_splitter::split(&to_plain_text(md))
                .into_iter()
                .map(|s| s.text)
                .collect();
            assert_eq!(exported, stored, "{md:?}");
        }
    }

    #[test]
    fn nested_list_parent_text_is_its_own_block() {
        assert_eq!(
            to_plain_text(NESTED_LIST_MDS[0]),
            "Cut: a fixed cut where Kokoro holds energy.\nEach edge eases in over half an octave.\nNext item."
        );
        assert_eq!(
            to_plain_text(NESTED_LIST_MDS[1]),
            "Parent one.\nChild one.\nChild two.\nGrandchild.\nParent two."
        );
        assert_eq!(
            to_plain_text(NESTED_LIST_MDS[3]),
            "1. Ordered parent.\nChild.\n2. Second parent."
        );
        assert_eq!(
            to_plain_text(NESTED_LIST_MDS[5]),
            "1. Nested first under a number.\n2. Plain."
        );
    }

    #[test]
    fn split_for_export_nested_list_parent_markdown_excludes_children() {
        let (sentences, block_lengths) = split_for_export(NESTED_LIST_MDS[0]);
        assert_eq!(block_lengths, vec![1, 1, 1]);
        assert_eq!(
            sentences[0].markdown_text,
            "**Cut:** a fixed cut where Kokoro holds energy."
        );
        assert_eq!(
            sentences[1].markdown_text,
            "Each edge eases in over half an octave."
        );
    }

    #[test]
    fn speaks_dollar_inline_math() {
        let md = "The signal is $x(t)$ over time.";
        assert_eq!(to_plain_text(md), "The signal is x of t over time.");
    }

    #[test]
    fn speaks_paren_inline_math() {
        let md = r"The signal is \(x(t)\) over time.";
        assert_eq!(to_plain_text(md), "The signal is x of t over time.");
    }

    #[test]
    fn math_integers_get_thousands_separators() {
        // The separators make the normalizer read the number as a quantity
        // ("twenty thousand") rather than digit by digit or as a year.
        assert_eq!(to_plain_text(r"\(30\) to \(20000\) Hz"), "30 to 20,000 Hz");
    }

    #[test]
    fn math_keeps_ordinal_suffix_glued() {
        assert_eq!(
            to_plain_text(r"the \(n\)th output sample"),
            "the nth output sample"
        );
        assert_eq!(to_plain_text(r"the \(k\)TH entry"), "the kTH entry");
        assert_eq!(to_plain_text(r"the \(2\)nd term"), "the 2nd term");
        assert_eq!(to_plain_text(r"the \(n\)th."), "the nth.");
        assert_eq!(to_plain_text(r"the \(k\)-th entry"), "the k-th entry");
    }

    #[test]
    fn math_still_spaced_from_other_following_text() {
        assert_eq!(to_plain_text("before $x$after"), "before x after");
        assert_eq!(to_plain_text("is $x$that"), "is x that");
        assert_eq!(to_plain_text("is $x$s"), "is x s");
        assert_eq!(to_plain_text(r"is \(x\)."), "is x.");
        assert_eq!(to_plain_text(r"$a$ \(\pm\)1.0"), "a plus or minus 1.0");
    }

    #[test]
    fn math_at_start_of_emphasis_keeps_emphasis_parsing() {
        assert_eq!(
            to_plain_text(r"we read as *\(y\) is \(x\)  convolved with \(h\)*."),
            "we read as y is x convolved with h."
        );
        assert_eq!(to_plain_text(r"so **\(x\)** wins"), "so x wins");
        assert_eq!(to_plain_text(r"so _\(x\) here_ wins"), "so x here wins");
        assert_eq!(to_plain_text(r"(*\(x\)*)"), "(x)");
        // A marker glued to a preceding word isn't an opener: spacing unchanged.
        assert_eq!(to_plain_text(r"a*\(x\)"), "a* x");
    }

    #[test]
    fn to_plain_text_applies_math_overrides() {
        let md = "$h * x$";
        let overrides = HashMap::from([("*".to_string(), "convolved with".to_string())]);
        assert_eq!(
            to_plain_text_with_math_overrides(md, &overrides),
            "h convolved with x"
        );
        // Unaffected without overrides.
        assert_eq!(to_plain_text(md), "h asterisk x");
    }

    // `x(t)` isn't valid LaTeX math-to-speech can parse (function-call
    // syntax, no `\` commands involved) — the strip-on-failure fallback
    // above covers that case. A formula math-to-speech *does* support gets
    // spoken instead of dropped.
    #[test]
    fn speaks_supported_inline_math() {
        let md = "The energy is $E = mc^2$ exactly.";
        assert_eq!(
            to_plain_text(md),
            "The energy is E equals mc squared exactly."
        );
    }

    // A formula immediately followed by punctuation must glue directly to
    // it ("t.", not "t .") — an orphaned, space-separated period reads as
    // the spoken word "point" to multiple independent TTS backends
    // (confirmed against misaki/Kokoro and VibeVoice/F5, which
    // share no pronunciation code), not a silent full stop.
    #[test]
    fn math_glues_directly_to_trailing_punctuation() {
        assert_eq!(
            to_plain_text(r"We deal only with continuous signals \(t\)."),
            "We deal only with continuous signals t."
        );
        assert_eq!(
            to_plain_text(r"We read this as the nth sample of signal \(x\)."),
            "We read this as the nth sample of signal x."
        );
        // Gluing prevention for a following *word* still applies.
        assert_eq!(to_plain_text(r"before\(x\)after"), "before x after");
    }

    #[test]
    fn speaks_placeholder_for_dollar_block_math() {
        let md = "Before.\n\n$$\\begin{matrix}1&2\\end{matrix}$$\n\nAfter.";
        assert_eq!(to_plain_text(md), "Before.\nmathematical formula\nAfter.");
    }

    #[test]
    fn speaks_placeholder_for_bracket_block_math() {
        let md = "Before.\n\n\\[\\begin{matrix}1&2\\end{matrix}\\]\n\nAfter.";
        assert_eq!(to_plain_text(md), "Before.\nmathematical formula\nAfter.");
    }

    // A `$` with no second `$` anywhere in the text must not eat the rest
    // of the document — find_dollar_inline_span returns None rather than
    // treating everything after it as an unterminated span. This is also
    // why a lone currency amount like "$5" is safe on its own: GitHub's
    // rule (open immediately followed by non-whitespace, close immediately
    // *preceded* by non-whitespace) means a second, unrelated "$10" later
    // in the same text does *not* pair with it either — "and $10" has a
    // space right before its `$`, which fails the closing condition, so
    // this common case is naturally not ambiguous in practice.
    #[test]
    fn lone_dollar_sign_with_no_closing_dollar_is_left_alone() {
        let md = "This costs $5 and that's final.";
        assert_eq!(to_plain_text(md), "This costs $5 and that's final.");
    }

    #[test]
    fn two_unrelated_currency_amounts_do_not_pair_up() {
        // "$10" is preceded by whitespace, which fails the closing-$ rule
        // — so this does *not* misparse as one large inline math span,
        // even though both amounts use `$`.
        let md = "This costs $5 and $10 more.";
        assert_eq!(to_plain_text(md), "This costs $5 and $10 more.");
    }

    // Where GitHub's ambiguity *does* bite: an inline span's closing `$`
    // only needs to be immediately preceded by non-whitespace, not by
    // anything that actually looks like math — so two dollar amounts with
    // no space directly before the second `$` do pair up and get treated
    // as one math span. Inherited deliberately, not solved — matches
    // GitHub's own behavior. Unlike the old strip-only behavior, this no
    // longer loses the words: "5 items" has no LaTeX commands in it, so
    // `math-to-speech` parses it as plain text and speaks it verbatim.
    #[test]
    fn adjacent_dollar_and_word_boundary_can_misparse_as_math() {
        let md = "Total: $5 items$ remain in stock.";
        assert_eq!(to_plain_text(md), "Total: 5 items remain in stock.");
    }

    // A pure-block-math paragraph whose LaTeX `math-to-speech` can't phrase
    // (unsupported construct) speaks `UNSPEAKABLE_MATH_PLACEHOLDER` instead
    // of becoming blank — see that constant's doc comment for why silence
    // isn't safe here: an actually-empty paragraph gets no pulldown_cmark
    // event at all, which used to desync `split_for_export`'s block count
    // from what a renderer (parsing the original markdown) still sees as
    // one paragraph.
    #[test]
    fn split_for_export_standalone_unsupported_math_block_speaks_placeholder() {
        let md = "$$\\begin{matrix}1&2\\end{matrix}$$";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0].text, "mathematical formula");
        assert_eq!(block_lengths, vec![1]);
    }

    #[test]
    fn split_for_export_keeps_sentence_around_inline_math() {
        let md = "The signal is $x(t)$ over time.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0].text, "The signal is x of t over time.");
    }

    #[test]
    fn find_math_spans_finds_all_four_delimiter_forms_in_order_with_correct_display_mode() {
        let md = "Inline $x(t)$ then \\(y(t)\\) then a block:\n\n$$a(t)$$\n\n\\[b(t)\\]";
        let spans = find_math_spans(md);
        assert_eq!(spans.len(), 4);
        assert_eq!(spans[0].tex, "x(t)");
        assert!(!spans[0].display);
        assert_eq!(spans[1].tex, "y(t)");
        assert!(!spans[1].display);
        assert_eq!(spans[2].tex, "a(t)");
        assert!(spans[2].display);
        assert_eq!(spans[3].tex, "b(t)");
        assert!(spans[3].display);
    }

    #[test]
    fn find_math_spans_ranges_index_into_the_original_string() {
        let md = "Before $x$ after.";
        let spans = find_math_spans(md);
        assert_eq!(spans.len(), 1);
        assert_eq!(&md[spans[0].start..spans[0].end], "$x$");
    }

    #[test]
    fn find_math_spans_empty_for_plain_text() {
        assert!(find_math_spans("Just plain text, no math here.").is_empty());
    }

    // Without ENABLE_TABLES, `|`-table syntax was never recognized as a
    // table at all — it fell through as one raw-pipe/dash paragraph, which
    // is what actually got spoken. Also confirms an empty cell (common —
    // trafilatura tables can carry empty spacer columns) contributes
    // nothing, not an empty line. Each row's last cell gets a trailing
    // period (see `splits_table_row_boundary_stops_paragraph_merge` for why)
    // — invisible in the rendered table (comes from `content`, untouched
    // here), but present in `plain_text`/the canonical sentence text.
    #[test]
    fn splits_table_cells_as_separate_lines() {
        let md = "| A | |\n|---|---|\n| 1 | x |";
        assert_eq!(to_plain_text(md), "A.\n1\nx.");
    }

    // The real-world bug this guards against: a table with no terminal
    // punctuation on any cell used to get glued by a downstream heading
    // merge (a paragraph that doesn't end a sentence joins the
    // next) into one incoherent run-on paragraph spanning every row —
    // confirmed on a real page, model hallucinated background music partway
    // through. A period on each row's last cell gives the merge a real
    // stopping point at each row boundary.
    #[test]
    fn table_row_boundary_gets_terminal_punctuation() {
        let md = "| Name | Symbol |\n|---|---|\n\
                  | sound power per unit area | I |\n\
                  | pressure | p |";
        assert_eq!(
            to_plain_text(md),
            "Name\nSymbol.\nsound power per unit area\nI.\npressure\np."
        );
    }

    #[test]
    fn drops_silent_heading() {
        let md = "## [Doug Engelbart]<!--silent-->\n\nHe invented the mouse.";
        assert_eq!(to_plain_text(md), "He invented the mouse.");
    }

    #[test]
    fn drops_standalone_silent_paragraph() {
        let md = "First.\n\n[An aside]<!--silent-->\n\nSecond.";
        assert_eq!(to_plain_text(md), "First.\nSecond.");
    }

    #[test]
    fn preserves_paragraph_boundaries_around_silent() {
        // The blank lines separating paragraphs must survive stripping.
        let md = "# [Intro]<!--silent-->\n\nFirst para.\n\nSecond para.";
        assert_eq!(to_plain_text(md), "First para.\nSecond para.");
    }

    #[test]
    fn tolerates_whitespace_in_marker() {
        let md = "## [Ted Nelson]<!-- silent -->\n\nHe coined hypertext.";
        assert_eq!(to_plain_text(md), "He coined hypertext.");
    }

    // ── split_for_export ──────────────────────────────────────────────────

    #[test]
    fn export_single_sentence_block_keeps_formatting() {
        let md = "This is **bold** text.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "This is bold text.");
        assert_eq!(sentences[0].markdown_text, "This is **bold** text.");
    }

    #[test]
    fn export_multiple_sentences_in_one_block() {
        let md = "First sentence. Second **sentence**.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![2]);
        assert_eq!(sentences[0].text, "First sentence.");
        assert_eq!(sentences[0].markdown_text, "First sentence.");
        assert_eq!(sentences[1].text, "Second sentence.");
        assert_eq!(sentences[1].markdown_text, "Second **sentence**.");
    }

    #[test]
    fn export_heading_and_list_items_are_separate_blocks() {
        let md = "# Title\n\n- one\n- two";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1, 1, 1]);
        assert_eq!(
            sentences
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Title", "one", "two"]
        );
    }

    #[test]
    fn export_heading_markdown_text_has_no_hash_prefix() {
        let md = "## Section Two\n\nSome text.";
        let (sentences, _) = split_for_export(md);
        assert_eq!(sentences[0].text, "Section Two");
        assert_eq!(
            sentences[0].markdown_text, "Section Two",
            "markdown_text should not include '##' — renderToken wraps the span in <h2>"
        );
    }

    #[test]
    fn export_paragraph_with_link_preserves_markdown() {
        let md = "See [this reference](https://example.com) for details.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "See this reference for details.");
        assert_eq!(
            sentences[0].markdown_text,
            "See [this reference](https://example.com) for details."
        );
    }

    #[test]
    fn export_drops_silent_heading_block() {
        let md = "## [Doug Engelbart]<!--silent-->\n\nHe invented the mouse.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "He invented the mouse.");
    }

    // Without ENABLE_TABLES, `|`-table syntax was never recognized as a
    // table at all — it fell through as one raw-pipe paragraph. Also
    // checks that an empty cell (common — trafilatura tables can carry
    // empty spacer columns) still gets a `block_lengths` entry (`0`, not
    // omitted) — a table renderer that walks every cell relies on each
    // having an entry to stay in sync with its running sentence index.
    #[test]
    fn export_table_produces_one_block_per_cell_including_empty() {
        let md = "| A | |\n|---|---|\n| 1 | x |";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1, 0, 1, 1]);
        assert_eq!(
            sentences
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "1", "x"]
        );
    }

    // Literal "<...>"-shaped text (e.g. citation-link syntax like "<Ref-1>"
    // from source documents predating HTML/markdown) is valid CommonMark raw
    // inline HTML syntax, so a naive "strip markup" pass can silently drop
    // it.
    #[test]
    fn preserves_literal_angle_bracket_text() {
        let md = "A short history may be found in <Ref-1>, along with a summary.";
        assert_eq!(
            to_plain_text(md),
            "A short history may be found in <Ref-1>, along with a summary."
        );
    }

    #[test]
    fn preserves_literal_angle_bracket_text_adjacent_to_real_link() {
        let md = "See <[Ref-1](https://example.com/Ref-1)> for details.";
        assert_eq!(to_plain_text(md), "See <Ref-1> for details.");
    }

    #[test]
    fn drops_inline_html_comment_mid_sentence() {
        let md =
            "In <!-- page header: 1933 he spent the summer --> while his family lived in Maine.";
        assert_eq!(to_plain_text(md), "In while his family lived in Maine.");
    }

    #[test]
    fn drops_inline_html_comment_wrapped_across_lines() {
        let md =
            "In <!-- page header: 1933 he\nspent the summer --> while his family lived in Maine.";
        assert_eq!(to_plain_text(md), "In while his family lived in Maine.");
    }

    #[test]
    fn split_for_export_agrees_with_to_plain_text_on_inline_comment() {
        let md = "First sentence <!-- a note. With a period. --> here. Second sentence.";
        let (sentences, _) = split_for_export(md);
        let plain = to_plain_text(md);
        assert_eq!(
            sentences
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            speakable_splitter::split(&plain)
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(plain, "First sentence here. Second sentence.");
    }
}
