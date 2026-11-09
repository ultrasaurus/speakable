use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// Placeholder that temporarily replaces periods in known abbreviations so
/// they don't trigger Unicode sentence boundaries. U+FFFE is a non-character
/// guaranteed never to appear in valid text.
const PERIOD_PLACEHOLDER: &str = "\u{FFFE}";
/// Like `PERIOD_PLACEHOLDER`, for a `?` or `!` inside a URL
/// (`index.php?title=…`). Also non-characters, and not `\u{FFFF}`, which
/// `markdown` passes through `split` as its hard-break marker.
const QUESTION_PLACEHOLDER: &str = "\u{FDD1}";
const EXCLAMATION_PLACEHOLDER: &str = "\u{FDD2}";

/// Abbreviations whose trailing period must not be treated as a sentence end.
const ABBREVS: &[&str] = &[
    // Titles
    "Mr", "Mrs", "Ms", "Miss", "Dr", "Prof", "Rev", "Sr", "Jr", "Mlle", "Mme",
    // Geographic
    "St", "Ave", "Blvd", "Rd", "Mt", "Dept", // Latin
    "vs", "etc", "e.g", "i.e", "et al", "viz",
    // Months (sometimes abbreviated in prose)
    "Jan", "Feb", "Mar", "Apr", "Jun", "Jul", "Aug", "Sep", "Sept", "Oct", "Nov", "Dec",
    // Corporate
    "Corp", "Inc", "Ltd", "Est", // Citations (e.g. "Vol. 31, No. 7)")
    "No", "Vol", "Fig", "Ref", "Vols", "p", "pp", "Ch", "Sec",
];

/// Abbreviations protected only inside one fixed phrase, for an abbreviation
/// that can't go in `ABBREVS` because it also ends real sentences: "Ed." is
/// the name Ed ("and Ed. Then …") as often as it means editor, but
/// "Ed. Note" is always one unit. Splitting it also breaks the
/// `ed. note` → "Editor's Note" pronunciation override, which then fires
/// across the sentence boundary when several sentences are normalized
/// together as one TTS segment.
const ABBREV_PHRASES: &[&str] = &["Ed. Note", "Ed. note"];

/// Bible book abbreviations, each with its trailing period. Their period is
/// hidden only before a chapter:verse number (`Gal. 6:1`): some, like
/// "Is." and "Am.", are also words that end real sentences. A full book
/// name ("Job", "Mark") is never listed, since it takes no period. A copy
/// of the normalizer's book table, which this crate can't depend on; a
/// test in the `speakable` CLI crate checks the two match.
#[rustfmt::skip]
const BIBLE_BOOK_ABBREVS: &[&str] = &[
    "Gen.", "Ge.", "Gn.", "Ex.", "Exod.", "Exo.", "Lev.", "Le.", "Lv.", "Num.", "Nu.", "Nm.",
    "Nb.", "Deut.", "De.", "Dt.", "Josh.", "Jos.", "Jsh.", "Judg.", "Jdg.", "Jg.", "Jdgs.",
    "Rth.", "Ru.", "1 Sam.", "1 Sm.", "1 Sa.", "1Sam.", "2 Sam.", "2 Sm.", "2 Sa.",
    "2Sam.", "1 Kgs.", "1 Ki.", "1K.", "1Kgs.", "2 Kgs.", "2 Ki.", "2K.", "2Kgs.", "1 Chron.",
    "1 Chr.", "1 Ch.", "1Chron.", "2 Chron.", "2 Chr.", "2 Ch.", "2Chron.", "Ezr.",
    "Ez.", "Neh.", "Ne.", "Esth.", "Est.", "Es.", "Jb.", "Prov.", "Pro.", "Prv.", "Pr.",
    "Eccles.", "Eccl.", "Ecc.", "Ec.", "SS.", "Song of Sol.", "Isa.",
    "Is.", "Jer.", "Je.", "Jr.", "Lam.", "La.", "Ezek.", "Eze.", "Ezk.", "Dan.", "Da.", "Dn.",
    "Hos.", "Ho.", "Jl.", "Am.", "Obad.", "Ob.", "Jnh.", "Jon.",
    "Mic.", "Mc.", "Nah.", "Na.", "Hab.", "Hb.", "Zeph.", "Zep.", "Zp.", "Hag.", "Hg.", "Zech.",
    "Zec.", "Zc.", "Mal.", "Ml.", "Matt.", "Mt.", "Mrk.", "Mk.", "Mr.", "Lk.",
    "Jn.", "Jhn.", "Ac.", "Rom.", "Ro.", "Rm.", "1 Cor.", "1 Co.", "1Cor.",
    "2 Cor.", "2 Co.", "2Cor.", "Gal.", "Ga.", "Eph.", "Ephes.", "Phil.", "Php.", "Col.", "Colo.",
    "1 Thess.", "1 Th.", "1Thess.", "2 Thess.", "2 Th.", "2Thess.", "1 Tim.", "1 Ti.", "1Tim.",
    "2 Tim.", "2 Ti.", "2Tim.", "Tit.", "Ti.", "Philem.", "Phm.", "Pm.", "Heb.",
    "Hebr.", "Jas.", "Jm.", "1 Pet.", "1 Pt.", "1Pet.", "2 Pet.", "2 Pt.", "2Pet.",
    "1 Jn.", "1Jn.", "2 Jn.", "2Jn.", "3 Jn.", "3Jn.",
    "Jud.", "Rev.", "Re.", "Rv.", "Ps.", "Psa.",
];

/// The Bible book abbreviations whose period isn't a sentence end before a
/// chapter:verse number, each with its trailing period.
pub fn bible_book_abbrevs() -> &'static [&'static str] {
    BIBLE_BOOK_ABBREVS
}

/// A sentence with its position in the document structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sentence {
    /// Stable sentence identifier. `split()` assigns fresh sequential ids
    /// (`0..N`) on every call — meaningless beyond that one call's
    /// output. Ids that survive edits
    /// come from diffing against a persisted previous list, which happens
    /// in the caller's persistence layer, not here.
    pub id: u64,
    pub text: String,
    /// True if this is the last sentence in its paragraph.
    /// Callers can use this to insert a longer pause after paragraph breaks.
    pub paragraph_end: bool,
}

/// True if `text`, once trimmed, is *entirely* a footnote/citation marker
/// (see `leading_marker_len`) — nothing else.
///
/// `reattach_marker_prefixes` merges a marker onto its preceding sentence
/// whenever one exists in the same paragraph, so a sentence that's nothing
/// but a marker should be rare — only when there's no preceding sentence to
/// merge into (e.g. a paragraph that's just `"[1]"` alone, or a marker as
/// the very first content in a document). Callers that would otherwise
/// treat a no-alphabetic-content sentence as silence-worthy filler (e.g. a
/// synthesizer that skips sentences with no letters) can use this to recognize that rare
/// case and still speak it — as "Footnote N." once normalized — rather
/// than silently dropping it.
pub fn is_footnote_marker(text: &str) -> bool {
    let trimmed = text.trim();
    leading_marker_len(trimmed) == Some(trimmed.len())
}

/// True if `text` has at least one alphanumeric character — false for a
/// sentence that's nothing but punctuation/symbols (a bare `-`/`—`, a
/// table's significance-marker cell like `*`/`**`/`***`). A bare number
/// (a table cell like `1` or `49`) *does* count as speakable — normalizes
/// to "one"/"forty nine" — so this checks `is_alphanumeric`, not just
/// `is_alphabetic`; the two agree on every non-numeric case. There's
/// nothing for TTS to say and nothing for forced-alignment to lock onto in
/// the punctuation-only case, so a caller synthesizing or aligning
/// per-sentence audio should treat that as an unconditional silent gap.
/// Kept here as the one shared check so a synthesizer and an aligner can't
/// disagree on it: two separate copies of this test once had the same
/// digit-only bug (treating a bare number as unspeakable).
/// Deliberately doesn't special-case `is_footnote_marker` text — unlike a
/// bare marker, that's speakable once normalized ("Footnote one."), which
/// is a decision for the caller to layer on top, not something this shared
/// predicate should assume.
pub fn has_speakable_content(text: &str) -> bool {
    text.chars().any(|c| c.is_alphanumeric())
}

/// Split `text` into sentences, preserving paragraph boundaries.
///
/// Each non-blank line is its own paragraph — blank lines are just
/// separators (allowed but not required). This matches plain text with no
/// blank lines at all (one paragraph per line) as
/// well as old-style blank-line-separated text. There's no "single newline
/// as a hard break within one paragraph" case here: `to_plain_text()`
/// always joins blocks with `\n\n` and converts in-block soft/hard breaks to
/// spaces, so a bare single `\n` in real plain_text is always a paragraph
/// boundary, never a hard break inside one. (Markdown-level hard breaks are
/// a rendering concern — this splitter only ever sees post-`to_plain_text`
/// text.)
///
/// Within each paragraph, sentence boundaries are detected at `.` `!` `?`
/// with abbreviation protection. The last sentence of each paragraph is
/// tagged with `paragraph_end: true`.
pub fn split(text: &str) -> Vec<Sentence> {
    let paragraphs: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let mut result = Vec::new();
    let mut next_id = 0u64;

    for paragraph in paragraphs {
        let sentences = split_paragraph(paragraph);
        let count = sentences.len();
        for (i, text) in sentences.into_iter().enumerate() {
            result.push(Sentence {
                id: next_id,
                text,
                paragraph_end: i == count - 1,
            });
            next_id += 1;
        }
    }

    result
}

/// Split a single paragraph into sentences.
///
/// Pieces stay untrimmed until the very end, on purpose: `merge_outline_labels`
/// and `merge_initials` join a piece onto whatever follows by plain
/// concatenation, not by trimming both sides and inserting a space. Source
/// text that glues a label directly onto what follows with no space at all
/// (`"1.A."`, a Project Gutenberg boilerplate numbering style) needs to stay
/// glued after merging — trimming early and always reinserting a space would
/// turn it into `"1. A."`, text that never existed in the source.
fn split_paragraph(paragraph: &str) -> Vec<String> {
    // Hide abbreviation periods
    let mut protected = paragraph.to_string();
    for abbrev in ABBREVS {
        let pattern = format!("{abbrev}.");
        protected = hide_abbreviation_period(&protected, &pattern);
    }
    for phrase in ABBREV_PHRASES {
        protected = hide_phrase_periods(&protected, phrase);
    }
    for abbrev in BIBLE_BOOK_ABBREVS {
        protected = hide_bible_book_period(&protected, abbrev);
    }
    protected = hide_url_terminators(&protected);

    // Split on single newlines (hard breaks) then Unicode sentence boundaries.
    let raw: Vec<String> = protected
        .lines()
        .flat_map(|line| {
            let ranges = recover_dropped_char_ranges(line, line.unicode_sentences());
            let ranges = reattach_marker_openers(line, ranges);
            let ranges = reattach_marker_prefixes(ranges, line);
            let ranges = reattach_citation_locator_tails(ranges, line);
            ranges.into_iter().map(move |(a, b)| &line[a..b])
        })
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.replace(PERIOD_PLACEHOLDER, ".")
                .replace(QUESTION_PLACEHOLDER, "?")
                .replace(EXCLAMATION_PLACEHOLDER, "!")
        })
        .collect();

    merge_journal_names(merge_initials(merge_outline_labels(raw)))
        .into_iter()
        .map(|s| s.trim().to_owned())
        .flat_map(cap_sentence_length)
        .collect()
}

/// A sentence whose estimated spoken length is over this gets cut into
/// smaller pieces (see `cap_sentence_length`). Matches the largest TTS
/// segment (in spoken words) a caller is expected to send in one request, so
/// that one sentence always fits in a segment; a caller with its own segment
/// limit should check that the two agree.
pub const MAX_SENTENCE_WORDS: usize = 300;

/// Size an over-long sentence is cut toward — below `MAX_SENTENCE_WORDS` so
/// the estimator's error (a few percent either way) still leaves each piece
/// under it.
const SENTENCE_CUT_TARGET_WORDS: usize = 250;

/// How many words either side of the ideal cut position to look for a
/// soft-punctuation boundary before settling for a plain word boundary.
const CUT_LOOKAROUND_WORDS: usize = 15;

/// Estimated spoken-word weight of one whitespace-delimited word, without
/// running the normalizer (a separate crate, downstream of this one).
///
/// Mirrors what normalization does to the shapes that make a sentence run
/// far past its raw word count: a 2–3 letter all-caps token is spelled
/// letter by letter (`"CJ"` -> "C J"), a number is spoken roughly one word
/// per digit, and a hyphen/dash becomes a space. A token with no letters or
/// digits (a bare `|` or `—`) is never spoken and weighs 0. Everything else
/// is one word.
fn estimated_word_weight(word: &str) -> usize {
    if !word.chars().any(char::is_alphanumeric) {
        return 0;
    }
    word.split(['-', '–', '—'])
        .map(|part| {
            let part = part.trim_matches(|c: char| !c.is_alphanumeric());
            if part.is_empty() {
                return 0;
            }
            let len = part.chars().count();
            if (2..=3).contains(&len) && part.chars().all(|c| c.is_ascii_uppercase()) {
                return len;
            }
            let digits = part.chars().filter(char::is_ascii_digit).count();
            if digits > 0 {
                return digits + usize::from(part.chars().any(char::is_alphabetic));
            }
            1
        })
        .sum()
}

/// Cuts `text`, one sentence, into pieces when its estimated spoken length
/// exceeds `MAX_SENTENCE_WORDS`; returns it unchanged otherwise.
///
/// Some sentences have no sentence-terminal punctuation to split on and are
/// still far over any segment's size cap — a reference-list entry's author
/// list (`"Murray CJ, Vos T, Lozano R, ..."`, hundreds of names, one final
/// period) is the real case. Downstream sizes segments in whole sentences,
/// so a sentence this long becomes one unplayable segment unless it is cut
/// here, where the cut becomes a real sentence boundary that ids, audio
/// slicing and highlights all agree on.
///
/// The text is cut into `ceil(estimate / SENTENCE_CUT_TARGET_WORDS)` pieces
/// of roughly equal estimated size. Each cut lands after the word ending in
/// `,` `;` `:` `—` `–` nearest the ideal position (within
/// `CUT_LOOKAROUND_WORDS`), or at the ideal word boundary if there is none.
/// Cuts are only ever between whitespace-separated words, so no word is
/// broken, and every piece is trimmed.
fn cap_sentence_length(text: String) -> Vec<String> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut word_start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), word_start) {
            (false, None) => word_start = Some(i),
            (true, Some(s)) => {
                spans.push((s, i));
                word_start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = word_start {
        spans.push((s, text.len()));
    }

    let weights: Vec<usize> = spans
        .iter()
        .map(|&(a, b)| estimated_word_weight(&text[a..b]))
        .collect();
    let total: usize = weights.iter().sum();
    // A single huge token has no word boundary to cut at.
    if total <= MAX_SENTENCE_WORDS || spans.len() < 2 {
        return vec![text];
    }

    let pieces = total.div_ceil(SENTENCE_CUT_TARGET_WORDS);
    let ends_soft = |word_idx: usize| {
        let (a, b) = spans[word_idx];
        text[a..b].ends_with([',', ';', ':', '—', '–'])
    };

    let mut out = Vec::with_capacity(pieces);
    let mut start = 0usize; // index of the first word of the current piece
    let mut remaining = total;
    for left in (2..=pieces).rev() {
        let want = remaining.div_ceil(left);
        // `ideal`: index of the first word of the next piece, where this
        // piece's running weight first reaches `want`.
        let mut acc = 0;
        let mut ideal = start;
        while ideal < spans.len() - 1 && acc < want {
            acc += weights[ideal];
            ideal += 1;
        }
        let ideal = ideal.max(start + 1);

        let mut cut = ideal;
        'search: for offset in 0..=CUT_LOOKAROUND_WORDS {
            for candidate in [ideal.saturating_sub(offset), ideal + offset] {
                if candidate > start && candidate < spans.len() && ends_soft(candidate - 1) {
                    cut = candidate;
                    break 'search;
                }
            }
        }

        out.push(text[spans[start].0..spans[cut - 1].1].trim().to_owned());
        remaining -= weights[start..cut].iter().sum::<usize>();
        start = cut;
        if start >= spans.len() {
            return out;
        }
    }
    out.push(text[spans[start].0..].trim().to_owned());
    out
}

/// Replaces every word-boundary occurrence of `pattern` (an abbreviation
/// plus its trailing period, e.g. `"p."`) with the same text but with the
/// period swapped for `PERIOD_PLACEHOLDER`, hiding it from
/// `unicode_sentences()`.
///
/// A match only counts if the character immediately before it is not
/// alphanumeric (start of text, whitespace, or punctuation like `(`) — a
/// plain substring replace would also fire mid-word, e.g. hiding the period
/// in `"shop."` because it ends in `"p."`. Case-sensitive, same as the old
/// substring replace this replaces: `ABBREVS` entries are cased
/// deliberately (`"No"` vs. a lowercase collision) and that's preserved
/// here.
fn hide_abbreviation_period(text: &str, pattern: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut consumed = 0usize;

    while let Some(rel) = rest.find(pattern) {
        let abs = consumed + rel;
        let boundary = text[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());

        out.push_str(&rest[..rel]);
        if boundary {
            out.push_str(&pattern[..pattern.len() - 1]);
            out.push_str(PERIOD_PLACEHOLDER);
        } else {
            out.push_str(pattern);
        }

        rest = &rest[rel + pattern.len()..];
        consumed = abs + pattern.len();
    }
    out.push_str(rest);
    out
}

/// Hides every `.`, `?` and `!` inside an `http://` or `https://` URL
/// (running to the next whitespace) behind its placeholder. Unicode's rules
/// end a sentence at a period before a capital even with no space, so a
/// reference DOI `…%3E3.0.CO;2-N` split at `3.0.`, and a `?` before a
/// query string (`watch?si=…`, `index.php?title=…`) split the same way.
/// Punctuation at the URL's end (`… see https://x.org/a.`, `…/a).`) is the
/// sentence's, so it's left visible.
fn hide_url_terminators(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = url_start(rest) {
        out.push_str(&rest[..start]);
        let token_len = rest[start..]
            .find(char::is_whitespace)
            .unwrap_or(rest.len() - start);
        let token = &rest[start..start + token_len];
        let body = token.trim_end_matches(|c: char| {
            matches!(
                c,
                '.' | '?'
                    | '!'
                    | ','
                    | ';'
                    | ':'
                    | ')'
                    | ']'
                    | '}'
                    | '"'
                    | '\''
                    | '\u{201D}'
                    | '\u{2019}'
            )
        });
        for c in body.chars() {
            match c {
                '.' => out.push_str(PERIOD_PLACEHOLDER),
                '?' => out.push_str(QUESTION_PLACEHOLDER),
                '!' => out.push_str(EXCLAMATION_PLACEHOLDER),
                c => out.push(c),
            }
        }
        out.push_str(&token[body.len()..]);
        rest = &rest[start + token_len..];
    }
    out.push_str(rest);
    out
}

/// Byte offset of the first `http://` / `https://` in `text` that starts a
/// word.
fn url_start(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("http") {
        let at = from + rel;
        let scheme = text[at..].starts_with("http://") || text[at..].starts_with("https://");
        if scheme
            && text[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        {
            return Some(at);
        }
        from = at + "http".len();
    }
    None
}

/// Replaces every whole-word occurrence of `phrase` (an `ABBREV_PHRASES`
/// entry) with the same text but with each of its periods swapped for
/// `PERIOD_PLACEHOLDER`. Whole-word on both ends: the character before must
/// not be alphanumeric (same check as `hide_abbreviation_period`), and
/// neither may the one after, so `"Ed. Notebook"` isn't protected.
/// Case-sensitive, like `ABBREVS`.
fn hide_phrase_periods(text: &str, phrase: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut consumed = 0usize;

    while let Some(rel) = rest.find(phrase) {
        let abs = consumed + rel;
        let end = abs + phrase.len();
        let before_ok = text[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());

        out.push_str(&rest[..rel]);
        if before_ok && after_ok {
            out.push_str(&phrase.replace('.', PERIOD_PLACEHOLDER));
        } else {
            out.push_str(phrase);
        }

        rest = &rest[rel + phrase.len()..];
        consumed = end;
    }
    out.push_str(rest);
    out
}

/// Hides the period of every whole-word `abbrev` (a `BIBLE_BOOK_ABBREVS`
/// entry) that whitespace and a chapter:verse number follow, as in
/// `(Gal. 6:1,2 NIV)`. Unicode's rules end a sentence there unless
/// lowercase text comes after the reference, which cut citations in two.
/// Before the match there must be no alphanumeric character, as in
/// `hide_abbreviation_period`; case-sensitive.
fn hide_bible_book_period(text: &str, abbrev: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut consumed = 0usize;

    while let Some(rel) = rest.find(abbrev) {
        let abs = consumed + rel;
        let end = abs + abbrev.len();
        let before_ok = text[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());

        out.push_str(&rest[..rel]);
        if before_ok && starts_with_chapter_verse(&text[end..]) {
            out.push_str(&abbrev[..abbrev.len() - 1]);
            out.push_str(PERIOD_PLACEHOLDER);
        } else {
            out.push_str(abbrev);
        }

        rest = &rest[rel + abbrev.len()..];
        consumed = end;
    }
    out.push_str(rest);
    out
}

/// True if `text` is whitespace, then digits, a colon and a digit: the
/// start of a chapter:verse number such as ` 6:1`.
fn starts_with_chapter_verse(text: &str) -> bool {
    let after_space = text.trim_start();
    if after_space.len() == text.len() {
        return false;
    }
    let digits = after_space.len()
        - after_space
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .len();
    let after_chapter = &after_space[digits..];
    digits > 0
        && after_chapter
            .strip_prefix(':')
            .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
}

/// Recovers characters `unicode_sentences()` silently drops around its
/// returned sentence slices.
///
/// Three places a drop can happen, all handled the same way — extend a
/// neighboring range to absorb the gap instead of losing it:
/// - Between two slices (e.g. a closing quote immediately after a spaced
///   ellipsis, `more . . . ". Next.`: neither returned slice contains the
///   `".` between them).
/// - Before the first slice.
/// - After the last slice — e.g. a trailing ellipsis with nothing after it
///   on the line (`...wedding.…`): the returned slice stops at the period
///   and the `…` is never part of any slice at all.
///
/// Walks the sentence slices in order using pointer offsets into `line`
/// (safe since each slice is guaranteed to be a literal substring of it,
/// produced by the same iterator). No character from the original text is
/// ever lost, regardless of which specific punctuation pattern triggers a
/// future instance of this same upstream quirk.
///
/// Returns byte ranges into `line` rather than materialized slices, so a
/// later pass (`reattach_marker_openers`) can adjust range boundaries
/// before anything is sliced out.
fn recover_dropped_char_ranges<'a>(
    line: &'a str,
    sentences: impl IntoIterator<Item = &'a str>,
) -> Vec<(usize, usize)> {
    let base = line.as_ptr() as usize;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0usize;

    for s in sentences {
        let start = s.as_ptr() as usize - base;
        let end = start + s.len();
        if start > cursor {
            match ranges.last_mut() {
                Some(last) => last.1 = start, // absorb the gap into the previous sentence
                None => {
                    ranges.push((cursor, end)); // absorb a leading gap into this sentence
                    cursor = end;
                    continue;
                }
            }
        }
        ranges.push((start, end));
        cursor = end;
    }

    if cursor < line.len() {
        match ranges.last_mut() {
            Some(last) => last.1 = line.len(), // absorb a trailing gap into the last sentence
            None => ranges.push((cursor, line.len())), // the whole line was one dropped gap
        }
    }

    ranges
}

/// Opening punctuation that can start a footnote/citation marker glued
/// directly onto a preceding sentence with no space (e.g. `resolution.[1]`,
/// `report.(2019)`), paired with its matching closer.
const MARKER_BRACKETS: &[(char, char)] = &[('[', ']'), ('(', ')')];

/// True if `body` is a citation/citation-list shape — comma-separated
/// numbers or number ranges (e.g. `1`, `18, 19`, `4–8`, `1, 4–8`; a range
/// joined by an en dash or a hyphen, as imported journal citations are
/// written) — or has the shape of Augment's own document cross-reference marker: a
/// single word-token identifier, a statement number, and a trailing empty
/// field, comma-separated (e.g. `AUGMENT,71279,`, `OAD,2221,`, or with a
/// space after the first comma, `AUGMENT, 81010,` — real citations in at
/// least one real document aren't consistently typed without it). The
/// second shape is narrow on purpose: it's recognized by its exact
/// three-part structure, not by "contains letters," so a real parenthetical
/// aside like `(see appendix)` still isn't mistaken for a marker.
///
/// A third shape: Markdown footnote-reference syntax, `^` followed by an
/// alphanumeric label (`^3`, `^note1`) — e.g. `[^3]`. Without this,
/// `"...with God.[^3]"` tore into two "sentences" at the period (`^3`
/// matched neither shape above), and whatever renders sentences
/// individually — e.g. an editor that joins consecutive sentence spans
/// with a literal space — ended up inserting that space
/// *inside* the marker (`"God.[ ^3]"`), garbled in both the displayed text
/// and, since the same tear reaches the TTS backend as two sentences
/// instead of one, in synthesized audio too.
///
/// A fourth shape: a general citation-field list.
/// - 2+ fields split on top-level `,`/`;` (`split_top_level_citation_fields`
///   — a separator *inside* a quoted span doesn't count as a boundary),
///   each field made up only of
///   letters/digits/whitespace/`. - : & ' , ; " " "`.
/// - At least one field must contain a digit — the signal that
///   distinguishes a real citation from ordinary parenthetical prose
///   (`(see appendix)` has no digit anywhere, so it's still excluded).
/// - Deliberately permissive about field *role* (author vs. year vs.
///   locator) rather than pattern-matching each citation style
///   individually — one rule covers:
///   - APA: `Smith, 2022, p. 15`; `Smith et al., 2020`;
///     `Smith & Jones, 2021`; multi-source `Smith, 2020; Jones, 2023`
///     (`;` separating distinct sources, standard APA style); a quoted
///     title standing in for an author, `"Understanding Behavioral
///     Economics", 2024`.
///   - IEEE/Vancouver: `3, pp. 6-10`; `3, Ch.  2, pp. 7-10` — extra
///     internal whitespace is fine, only whitespace-ness is checked, not
///     exact spacing.
///   - This crate's own earlier example: `Fox, Works, 8:42` /
///     `Fox, Works, 7:332, 335-6` (volume:page and page-range locators);
///     and a quoted-title + semicolon-separated combination, `Isaac
///     Penington, "Of the Church in its First and Pure State, and its
///     Declining State, ..."; Works of Isaac Penington, vol. 3: pp.133-7`
///     — the comma inside the quoted title doesn't split it apart.
/// - Found via a real document whose citations (`[Fox, Works, 8:42]`,
///   five of them, plus the quoted-title/semicolon shape above) weren't
///   recognized as markers at all — a downstream check for whether a
///   paragraph ends in a finished sentence saw every one as an unfinished
///   fragment and merged the paragraph into the next, chaining unrelated
///   paragraphs across the whole document into oversized TTS segments
///   (seen in 5 separate documents, not a one-off).
///
/// `pub` so a caller's own paragraph-level check (such as grouping
/// paragraphs into TTS segments) can share this exact definition instead of
/// keeping its own copy that could silently drift
/// out of sync with this crate's sentence-level marker recognition.
pub fn is_marker_body(body: &str) -> bool {
    if !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c.is_whitespace())
    {
        return true;
    }
    if is_number_range_list(body) {
        return true;
    }
    if let Some(label) = body.strip_prefix('^') {
        if !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric()) {
            return true;
        }
    }
    if is_citation_field_list(body) {
        return true;
    }
    matches!(body.split(',').collect::<Vec<_>>().as_slice(),
        [ident, digits, tail]
        if !ident.is_empty() && ident.chars().all(|c| c.is_ascii_alphabetic())
        && { let digits = digits.trim(); !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) }
        && tail.trim().is_empty())
}

/// `4–8`, `1, 4–8`, `4-8`: comma-separated fields, each a number or two
/// numbers joined by one en dash or hyphen.
fn is_number_range_list(body: &str) -> bool {
    let is_number = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    body.split(',').all(|field| {
        let field = field.trim();
        match field.split_once(['–', '-']) {
            Some((a, b)) => is_number(a) && is_number(b),
            None => is_number(field),
        }
    })
}

/// Splits `body` on top-level `,`/`;`, treating a quoted span as one atomic
/// field — a separator *inside* quotes doesn't count as a field boundary.
/// Needed for a quoted work title that itself contains commas (`Isaac
/// Penington, "Of the Church in its First and Pure State, and its
/// Declining State, ..."; Works of Isaac Penington, vol. 3: pp.133-7`).
///
/// Recognizes both straight (`"..."`) and curly (`"..."` — U+201C/U+201D)
/// quotes directly: the splitter runs on source text, before any
/// normalization that might straighten curly quotes.
fn split_top_level_citation_fields(body: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut closing_quote: Option<char> = None;
    for (i, c) in body.char_indices() {
        match closing_quote {
            Some(close) if c == close => closing_quote = None,
            Some(_) => {}
            None => match c {
                '"' => closing_quote = Some('"'),
                '\u{201C}' => closing_quote = Some('\u{201D}'),
                ',' | ';' => {
                    fields.push(&body[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    fields.push(&body[start..]);
    fields
}

/// See the fourth-shape doc comment on `is_marker_body` above for the full
/// rationale and worked examples — this is just the character-class check
/// itself: 2+ top-level-`,`/`;`-separated (quote-aware, see
/// `split_top_level_citation_fields`), non-empty, trimmed fields, each
/// restricted to letters/digits/whitespace/`. - : & ' , ; " " "`, at least
/// one field containing a digit. A field is allowed to contain `,`/`;`
/// itself (unlike the top-level separator check) since those can only
/// appear there if they were inside a quoted span to begin with — the
/// splitting above already consumed every real top-level occurrence.
fn is_citation_field_list(body: &str) -> bool {
    let fields: Vec<&str> = split_top_level_citation_fields(body)
        .into_iter()
        .map(str::trim)
        .collect();
    if fields.len() < 2 || fields.iter().any(|f| f.is_empty()) {
        return false;
    }
    // `body` may still carry `PERIOD_PLACEHOLDER` instead of a literal `.`:
    // - `reattach_marker_openers` checks `is_marker_body` before
    //   `split_paragraph` restores hidden abbreviation periods.
    // - A locator field like "p." hits this: "p" is in `ABBREVS`, so its
    //   period is already hidden by the time this check runs.
    // - Treated the same as a literal `.` here, or that field fails the
    //   character-class check on the placeholder alone.
    let field_ok = |f: &str| {
        f.chars().all(|c| {
            c.is_alphabetic()
                || c.is_ascii_digit()
                || c.is_whitespace()
                || matches!(
                    c,
                    '.' | '-'
                        | ':'
                        | '&'
                        | '\''
                        | '\u{FFFE}'
                        | ','
                        | ';'
                        | '"'
                        | '\u{201C}'
                        | '\u{201D}'
                )
        })
    };
    fields.iter().all(|f| field_ok(f))
        && fields.iter().any(|f| f.chars().any(|c| c.is_ascii_digit()))
}

/// `unicode_sentences()` extends a sentence-terminal run (`.`/`!`/`?`)
/// through any immediately following "Close"-category punctuation before
/// placing a boundary (UAX #29 rule SB8a). Closing brackets/quotes
/// correctly belong to that run (`"Really?" she asked.`), but an *opening*
/// bracket glued directly onto the terminator — an unspaced citation marker
/// like `resolution.[1]` — can never belong to the sentence that just
/// ended; it's the start of something new. Left alone, the algorithm still
/// lumps the bracket onto the previous range and starts the next range
/// mid-marker (`"...resolution.["`, `"1] This is because..."`), corrupting
/// both sides.
///
/// A footnote marker reads as belonging to whatever it's citing — the
/// sentence immediately before it, same as a superscript reference would —
/// so this moves the whole marker (opener through its matching closer, e.g.
/// `[1]` or `[18, 19]`) onto the end of the previous (contiguous) range
/// instead, and advances the next range's start past it. Only fires when
/// the marker body is digits/commas/whitespace only — a citation or
/// citation-list shape — so a real bracketed clause that happens to start
/// the next sentence (contains letters) is left untouched.
fn reattach_marker_openers(line: &str, mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    for i in 0..ranges.len().saturating_sub(1) {
        let (start, end) = ranges[i];
        if ranges[i + 1].0 != end {
            continue; // not contiguous — e.g. a spaced marker, already fine
        }
        // UAX #29's terminal-run extension (SB8a) can sweep an opening
        // quote glued directly after the bracket into this same range too
        // — e.g. `shift.("Understanding...` ends up as one range,
        // `"shift.(\""`, not stopping at the `(`. Skip past any such
        // trailing opening quote before looking for the actual bracket
        // opener underneath, so a quoted title right after the opener
        // (`("Title", 2024)`) is still recognized as glued.
        let opener_end = {
            let mut e = end;
            while let Some(c) = line[start..e].chars().last() {
                if matches!(c, '"' | '\u{201C}') {
                    e -= c.len_utf8();
                } else {
                    break;
                }
            }
            e
        };
        let Some((opener, closer)) = line[start..opener_end]
            .chars()
            .last()
            .and_then(|c| MARKER_BRACKETS.iter().copied().find(|(o, _)| *o == c))
        else {
            continue;
        };
        if opener_end - start <= opener.len_utf8() {
            continue; // range is nothing but the opener — nothing to keep
        }

        let (next_start, next_end) = ranges[i + 1];
        let next_text = &line[next_start..next_end];
        let Some(rel_close) = next_text.find(closer) else {
            continue;
        };
        // Reconstruct the full body, prepending back whatever opening quote
        // got swept into range `i` above — `is_marker_body`'s quote-aware
        // field splitter (`split_top_level_citation_fields`) needs the
        // matching open half or a lone closing quote inside the body
        // (e.g. `Title", 2024`) misreads as an unterminated *opening*
        // quote instead, swallowing the rest of the body as one field.
        let swept_open_quote = &line[opener_end..end];
        let body = format!("{swept_open_quote}{}", &next_text[..rel_close]);
        if !is_marker_body(&body) {
            continue; // not a citation/citation-list shape — leave the split alone
        }

        let marker_end = next_start + rel_close + closer.len_utf8();
        ranges[i].1 = marker_end;
        ranges[i + 1].0 = marker_end;
    }
    ranges
}

/// Finds a footnote/citation marker at the very start of `text` (after
/// skipping any leading whitespace) — a bracket pair (`[1]`, `(2019)`,
/// `[18, 19]`) or a same-character asterisk pair (`*1*`) whose body is a
/// citation shape (see `is_marker_body`). Returns the byte length of the
/// whole match, including the leading whitespace skipped over, or `None`.
fn leading_marker_len(text: &str) -> Option<usize> {
    let trimmed = text.trim_start();
    let ws_len = text.len() - trimmed.len();
    let first = trimmed.chars().next()?;

    if first == '*' {
        let rest = &trimmed[first.len_utf8()..];
        let rel_close = rest.find('*')?;
        let body = &rest[..rel_close];
        return is_marker_body(body).then(|| ws_len + first.len_utf8() + rel_close + 1);
    }
    for &(open, close) in MARKER_BRACKETS {
        if first == open {
            let rest = &trimmed[open.len_utf8()..];
            let rel_close = rest.find(close)?;
            let body = &rest[..rel_close];
            if is_marker_body(body) {
                return Some(ws_len + open.len_utf8() + rel_close + close.len_utf8());
            }
        }
    }
    None
}

/// Reattaches a footnote/citation marker sitting at the *start* of a range
/// onto the end of the *preceding* range — covering two cases
/// `reattach_marker_openers` doesn't:
///
/// - **Marker as a prefix of a longer range.** Asterisk isn't in Unicode's
///   "Close" category the way `]`/`)` are, so `unicode_sentences()` never
///   tears an asterisk marker away from a following sentence at all — it
///   just bundles `"*1* Section II discusses..."` into one range. A spaced
///   bracket marker followed by more prose in the same paragraph
///   (`"resolution. [1] Next sentence."`) has the same problem for a
///   different reason: it's not contiguous with the previous range, so
///   `reattach_marker_openers`'s tear-repair doesn't fire, but `[1]`
///   still isn't its own range — it's bundled with what follows.
/// - **Marker as a range's entire remaining content** — the common
///   end-of-paragraph case (e.g. a `"*1*"` footnote, or a spaced
///   `"[12]"` at the end of a paragraph). A marker glued or spaced onto
///   nothing else reads as belonging to the sentence it's citing, not as
///   its own free-standing "sentence".
///
/// Either way the marker is split off as its own chunk and, if there's a
/// preceding real (non-empty) range already emitted, merged onto the end
/// of *that* — so multiple consecutive markers (`"[1][2] Next."`) all land
/// on the same real sentence instead of only merging pairwise.
///
/// If there's no preceding range at all (a marker is the very first
/// content in its paragraph) and the marker is followed by more prose in
/// the same range (e.g. an outline label `"(1) Concept Manipulation—Humans
/// rose..."`, or `"*1* How are you?"`), the marker is left bundled with
/// that prose instead of split off — the same "reads as part of the
/// sentence it's attached to, never free-standing" rule applied in the
/// other direction. Splitting it off here used to leave a short,
/// marker-only "sentence" (a synthesizer that skips no-letter sentences
/// can use `is_footnote_marker` so that at least isn't silently dropped) — but in practice a lone one- or two-word sentence like that
/// (`"one"`, once normalized) is short enough to sometimes fail forced
/// alignment, and its audio drifts into the next segment instead. Only
/// when the marker consumes the *entire* range (an empty remainder — a
/// marker alone in its own paragraph, with no following prose in this
/// range to bundle onto) does it still become its own leading range;
/// `is_footnote_marker`'s exception still matters for that case.
///
/// A leftover empty remainder (when the marker was a range's *entire*
/// content, not just a prefix, and there *was* a preceding range to merge
/// onto) is dropped later by `split_paragraph`'s emptiness filter, same as
/// any other empty range — no separate handling needed for "the whole
/// range was just a marker" vs. "only a prefix was".
fn reattach_marker_prefixes(ranges: Vec<(usize, usize)>, line: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if start == end {
            out.push((start, end));
            continue;
        }
        let Some(marker_len) = leading_marker_len(&line[start..end]) else {
            out.push((start, end));
            continue;
        };
        let marker_end = start + marker_len;
        let remainder = &line[marker_end..end];
        // If the marker doesn't consume the whole range, but what's left is
        // just trailing punctuation/whitespace with no real sentence
        // content (e.g. a reference list's own "(2004). " — UAX #29 already
        // isolated this as its own complete sentence-final range, and the
        // "marker" here is really just a bare citation year followed by
        // that sentence's own terminal period, not a marker glued onto
        // *more prose bundled in the same range*), splitting it off would
        // strand that trailing punctuation as a separate, content-free
        // range. Leave the whole range untouched instead. An *empty*
        // remainder (the marker consumes the entire rest of the range) is
        // the normal, intended case below — a marker with nothing after it
        // at all still needs reattaching onto the preceding sentence.
        if !remainder.is_empty() && !remainder.chars().any(|c| c.is_alphanumeric()) {
            out.push((start, end));
            continue;
        }
        match out.iter().rposition(|&(s, e)| s != e) {
            Some(t) => {
                out[t].1 = marker_end; // extend the last real range to absorb the marker
                out.push((marker_end, end)); // remainder (may be empty; filtered later)
            }
            None if remainder.is_empty() => {
                out.push((start, marker_end)); // no predecessor, nothing to bundle onto — its own range
                out.push((marker_end, end));
            }
            None => out.push((start, end)), // no predecessor, but prose follows — stay bundled
        }
    }
    out
}

/// True if `text` (trimmed) is *only* a torn-off citation locator tail:
/// whatever's left standing alone as its own range after
/// `unicode_sentences()` breaks a citation mid-body — e.g. a book title's
/// own terminal period followed by a nested publisher parenthetical
/// (`"[Thomas Kelly, A Testament of Devotion. (New York: Harper &
/// Brothers, 1941), p. 5]"` breaks at `"Devotion."`, and
/// `reattach_marker_openers` above reattaches the parenthetical onto the
/// preceding sentence — but the trailing `", p. 5]"` is left behind as a
/// new range of its own, since nothing recognizes it as belonging to the
/// sentence before it rather than starting a new one).
///
/// Shape, matched right-to-left since the closing `]` is the one certain
/// anchor: the citation's own closing bracket, an optional trailing
/// period, optional closing punctuation left over from the citation's own
/// enclosing quote/parenthesis (`)`, `"`, `'`, curly variants), an optional
/// `,`/`;` separating it from what came before, an optional page-locator
/// keyword (`p.`/`pp.`/`page`/`pages` — some citation styles number a bare
/// locator with no keyword at all, e.g. `, 5]`), and one or two dash-joined
/// digit runs (`5`, `340-341`, `24—28`) — and nothing else. Checked against
/// the whole (trimmed) range, not just a prefix: a real sentence never puts
/// its own page locator first, so this shape only ever matches a torn-off
/// tail, never the start of ordinary prose.
///
/// Deliberately narrow — recognizes the locator-tail shape itself, not
/// citations in general, so it can't mistake an ordinary bracketed aside
/// (a legal contract's optional clause, a legislative strikeout) for one:
/// prose essentially never happens to be *only* "closing punctuation + an
/// optional page-locator keyword + digits".
fn is_citation_locator_tail(text: &str) -> bool {
    // Runs on the paragraph's still-`ABBREVS`-protected text (abbreviation
    // periods, including "p."/"pp.", are hidden behind `PERIOD_PLACEHOLDER`
    // until later in `split_paragraph`) — normalize back to a literal `.`
    // for matching purposes only; the actual merge in
    // `reattach_citation_locator_tails` still extends the real, unmodified
    // byte range, so this has no effect on what text ends up where.
    let normalized = text.replace(PERIOD_PLACEHOLDER, ".");
    let Some(s) = normalized.trim().strip_suffix(']') else {
        return false;
    };
    let s = s.strip_suffix('.').unwrap_or(s);
    let s = s.trim_start_matches([')', '"', '\'', '\u{201D}', '\u{2019}']);
    let s = s.trim_start_matches([',', ';']).trim_start();
    // The page-locator keyword itself is optional — some citation styles
    // (e.g. `[1 Timothy 3:15; Fox, Journal, p. 24]`'s own sibling style,
    // or a bare `, 5]`) give just the number.
    let s = ["pp.", "pages", "page", "p."]
        .iter()
        .find_map(|kw| s.strip_prefix(kw).map(str::trim_start))
        .unwrap_or(s);

    let is_digit_run = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    match s.find(|c: char| matches!(c, '-' | '\u{2010}'..='\u{2015}')) {
        Some(pos) => {
            let (a, rest) = s.split_at(pos);
            let dash_len = rest.chars().next().map(char::len_utf8).unwrap_or(1);
            is_digit_run(a) && is_digit_run(&rest[dash_len..])
        }
        None => is_digit_run(s),
    }
}

/// Merges a torn-off citation locator tail (see [`is_citation_locator_tail`])
/// onto the end of the preceding real range — the same "reads as part of
/// the sentence it's citing, never free-standing" rule
/// `reattach_marker_prefixes` applies to markers, just triggered by a
/// different shape. Runs after both marker passes so it only ever sees
/// whatever they left behind; skips past any empty range from those
/// passes to find the nearest real predecessor, same technique
/// `reattach_marker_prefixes` uses.
fn reattach_citation_locator_tails(ranges: Vec<(usize, usize)>, line: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if start != end && is_citation_locator_tail(&line[start..end]) {
            if let Some(t) = out.iter().rposition(|&(s, e)| s != e) {
                out[t].1 = end;
                continue;
            }
        }
        out.push((start, end));
    }
    out
}

/// True if `part` (one dot-free component of a label) is a single outline
/// label component: alphanumeric only, and either digits/uppercase (`8`,
/// `XIV`, `A`) or a lowercase Roman numeral ≤ 100 (`ii`; rejects real words
/// like `mix`, which parses as 1009, and non-numerals like `e`/`g`).
fn is_label_part(part: &str) -> bool {
    if part.is_empty() || !part.chars().all(|c| c.is_alphanumeric()) {
        return false;
    }
    let alpha: Vec<char> = part.chars().filter(|c| c.is_alphabetic()).collect();
    if alpha.iter().all(|c| c.is_uppercase()) {
        return true;
    }
    if alpha.iter().all(|c| c.is_lowercase()) {
        return roman::from(&part.to_uppercase()).is_some_and(|n| n <= 100);
    }
    false
}

/// True if `s` is an outline label: one or more `.`-separated parts, each
/// passing `is_label_part`, with at most 4 alphabetic characters in total.
/// Covers single-level labels (`I.`, `A.`, `8.`, `ii.`), multi-level section
/// numbers (`5.7.`, `5.2.3.1.`), glued alphanumeric chains (`1.A.`, `1.E.1.`)
/// and dotted initials/acronyms (`W.E.B.`, `U.S.`, `N.B.`). Never matches a
/// piece containing whitespace, so a label only counts when it is the whole
/// piece — `"in the U.S. "` inside prose is untouched — and mixed-case or
/// non-numeral lowercase parts (`Ph.D.`, `e.g.`) are rejected.
fn is_outline_label(s: &str) -> bool {
    let body = s.trim().trim_end_matches('.');
    if body.chars().filter(|c| c.is_alphabetic()).count() > 4 {
        return false;
    }
    body.split('.').all(is_label_part)
}

/// Merge outline-style labels with the sentence that follows them.
///
/// Unicode sentence segmentation splits `"I. Introduction"` into
/// `["I. ", "Introduction"]` because the capital `I` after the space triggers
/// a boundary; `"5.7. Summing sinusoids"` and `"U.S. Department"` tear the
/// same way. A glued chain tears into several pieces (`"1.A. By"` becomes
/// `["1.", "A. ", "By"]`), so merging repeats while the accumulated piece is
/// still a label (see `is_outline_label`), then attaches the prose that
/// follows.
///
/// Merges by plain concatenation, not trim-both-sides-and-insert-a-space —
/// `sentences` isn't pre-trimmed (see `split_paragraph`), so whatever
/// whitespace (or lack of it) actually sits between the two pieces in the
/// source is preserved. A glued label (`"1.A."`, no space) stays glued;
/// a spaced one (`"I. Introduction"`) keeps its one space, since the
/// pieces already carry it.
fn merge_outline_labels(sentences: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sentences.len());
    let mut iter = sentences.into_iter().peekable();
    while let Some(mut s) = iter.next() {
        while is_outline_label(&s) {
            let Some(next) = iter.peek() else { break };
            s = format!("{s}{next}");
            iter.next();
        }
        out.push(s);
    }
    out
}

/// True if `token` is exactly a single uppercase letter followed by a
/// period (`"E."`, `"R."`) — a bare initial, nothing else.
fn is_single_initial_token(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(c), Some('.'), None) if c.is_ascii_uppercase()
    )
}

/// True if `s`, once trimmed, ends in a bare-initial token (`"E."`,
/// `"Engelbart, D."`, `"J. R. R."`) — the rest of the sentence can be
/// anything.
fn ends_with_bare_initial(s: &str) -> bool {
    s.split_whitespace()
        .last()
        .is_some_and(is_single_initial_token)
}

/// Merges a sentence ending in a bare middle initial onto the sentence that
/// follows, so a name like `"E. B. White wrote this."` or a trailing-initial
/// citation like `"Engelbart, D. C."` reads as one sentence instead of
/// splitting mid-name.
///
/// `unicode_sentences()` (and `merge_outline_labels` above, which exists for
/// the unrelated outline-label case `"I. Introduction"`) both treat a bare
/// single uppercase letter + period as sentence-terminal, so a run of
/// initials tears itself away from the name it belongs to. For a
/// leading-initials name (`"E. B. White"`) the torn-off fragment (`"E. B."`)
/// is no longer a "single label" `merge_outline_labels` recognizes once two
/// or more initials have combined (it contains a space); for a
/// trailing-initials citation (`"Engelbart, D."` / `"C."`) the fragment was
/// never a bare-initials-only piece to begin with, since real text (the
/// surname) precedes it. Either way, nothing pulls the pieces back together
/// without this pass.
///
/// The discriminator is deliberately just "does the sentence *end* in a bare
/// initial" — not "is the whole sentence nothing but initials" — so it
/// catches both `"D. C. Smith"` (leading) and `"Smith, D. C."` (trailing)
/// author-initial styles with the same rule. That's a deliberate
/// over-merging choice: a real sentence that happens to end in a single
/// letter (`"She got an A."`, `"The correct answer is C."`) will also merge
/// forward into whatever follows, which we're treating as the rarer, lower-
/// cost case — a missed split just costs an unusually long pause-free run of
/// text, while a real trailing-initials citation (recurring, not rare, in
/// citation-heavy documents) would otherwise get a hard mid-name pause every
/// single time it appears.
///
/// Merges by plain concatenation, not trim-both-sides-and-insert-a-space —
/// same reasoning as `merge_outline_labels`: `sentences` isn't pre-trimmed,
/// so a glued initials run stays exactly as glued (or spaced) as the source
/// had it.
fn merge_initials(sentences: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sentences.len());
    let mut iter = sentences.into_iter().peekable();
    while let Some(mut s) = iter.next() {
        while ends_with_bare_initial(&s) {
            let Some(next) = iter.peek() else { break };
            s = format!("{s}{next}");
            iter.next();
        }
        out.push(s);
    }
    out
}

/// Rejoins a reference entry's journal name with its volume/pages/year.
///
/// A reference reads "Authors. Title. Journal Vol, pages (year)." and its
/// journal name is usually abbreviated, one period per word — `J. Acoust.
/// Soc. Am. 108, 1170–1180 (2000).` — so `unicode_sentences()` tears it
/// into "J. Acoust." / "Soc." / "Am." / "108, 1170–1180 (2000)." (a
/// spelled-out name like `Brain. 1998;121:233–41.` tears off too).
/// Listing journal abbreviations in `ABBREVS` can't keep up: real
/// documents use hundreds (Acoust, Neurophysiol, Otolaryngol, ...), and
/// short ones like `Am.`/`Res.` would stop real sentence ends in prose
/// from splitting.
///
/// Instead, a run of short name-like pieces (`is_journal_name_piece`) is
/// merged onto the piece right after it when that piece opens with a
/// citation locator (`is_citation_locator`). Requiring the locator is what
/// keeps prose safe: "Done. 5 minutes later, we left." has a name-like
/// piece followed by a digit, but no volume/pages/year shape. The title
/// before the journal, ordinary lowercase prose, stays its own sentence —
/// a natural pause between title and journal.
fn merge_journal_names(sentences: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sentences.len());
    let mut i = 0;
    while i < sentences.len() {
        let mut j = i;
        while j < sentences.len() && is_journal_name_piece(&sentences[j]) {
            j += 1;
        }
        if j > i && j < sentences.len() && is_citation_locator(&sentences[j]) {
            out.push(sentences[i..=j].concat());
            i = j + 1;
        } else {
            out.push(sentences[i].clone());
            i += 1;
        }
    }
    out
}

/// `J. Acoust.`, `Soc.`, `Nat. Rev. Neurosci.`, `J Neurol Neurosurg
/// Psychiatry.`, `Acta Acust. united Ac.`, `Brain.`: at most six words,
/// ending in a period, each word capitalized, a lowercase abbreviation
/// (`acoust.`), or a connector.
fn is_journal_name_piece(s: &str) -> bool {
    const CONNECTORS: &[&str] = &[
        "&", "of", "and", "the", "in", "for", "on", "united", "de", "der", "la", "für",
    ];
    let s = s.trim();
    let words: Vec<&str> = s.split_whitespace().collect();
    s.ends_with('.')
        && !words.is_empty()
        && words.len() <= 6
        && words.iter().all(|w| {
            let first_upper = w.chars().next().is_some_and(|c| c.is_uppercase());
            let lower_abbrev =
                w.ends_with('.') && w[..w.len() - 1].chars().all(|c| c.is_alphabetic());
            let name_chars = w
                .trim_end_matches('.')
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'');
            (first_upper && name_chars) || lower_abbrev || CONNECTORS.contains(w)
        })
}

/// A reference's volume/pages/year: opens with a digit and has a page
/// range (`1170–1180`), a parenthesized year (`(2000)`), a `;`
/// (`2004;24:940–6.`), or a PubMed/DOI tag (`2015.PMID: 26141381`).
fn is_citation_locator(s: &str) -> bool {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    let page_range = chars
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && matches!(w[1], '–' | '-') && w[2].is_ascii_digit());
    let paren_year = chars.windows(6).any(|w| {
        w[0] == '(' && w[1..5].iter().all(|c| c.is_ascii_digit()) && matches!(w[5], ')' | ',' | ' ')
    });
    chars.first().is_some_and(|c| c.is_ascii_digit())
        && (page_range || paren_year || s.contains(';') || s.contains("PMID") || s.contains("doi"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sentences: Vec<Sentence>) -> Vec<String> {
        sentences.into_iter().map(|s| s.text).collect()
    }

    fn paragraph_ends(sentences: &[Sentence]) -> Vec<bool> {
        sentences.iter().map(|s| s.paragraph_end).collect()
    }

    // ── has_speakable_content ────────────────────────────────────────────

    #[test]
    fn has_speakable_content_true_for_ordinary_prose() {
        assert!(has_speakable_content("A cat mewing"));
    }

    #[test]
    fn has_speakable_content_false_for_bare_punctuation() {
        assert!(!has_speakable_content("-"));
        assert!(!has_speakable_content("—"));
        assert!(!has_speakable_content("*"));
        assert!(!has_speakable_content("**"));
        assert!(!has_speakable_content("***"));
    }

    // A bare number (a table cell like "1" or "49") is speakable — it
    // normalizes to "one"/"forty nine" — even though digits aren't
    // `is_alphabetic`. Confirmed on a real page: a stats table's "1"
    // column value was getting silently dropped, matching the "-"/"**"
    // symbol-only case as a false positive it should never have hit.
    #[test]
    fn has_speakable_content_true_for_bare_numbers() {
        assert!(has_speakable_content("1"));
        assert!(has_speakable_content("49"));
    }

    #[test]
    fn has_speakable_content_true_if_any_letter_present() {
        // "p" alone (a stats-table p-value column header) still counts.
        assert!(has_speakable_content("p"));
    }

    // ── split ─────────────────────────────────────────────────────────────

    #[test]
    fn split_empty_string_returns_empty_vec() {
        assert!(split("").is_empty());
    }

    #[test]
    fn split_whitespace_only_returns_empty_vec() {
        assert!(split("   \n   \n   ").is_empty());
    }

    #[test]
    fn split_single_sentence_returns_one_element() {
        let result = split("Hello world.");
        assert_eq!(texts(result.clone()), vec!["Hello world."]);
        assert_eq!(paragraph_ends(&result), vec![true]);
    }

    #[test]
    fn split_two_sentences_in_one_line() {
        let result = split("Hello world. How are you?");
        assert_eq!(texts(result.clone()), vec!["Hello world.", "How are you?"]);
        assert_eq!(paragraph_ends(&result), vec![false, true]);
    }

    #[test]
    fn split_exclamation_and_question_marks() {
        let result = split("Watch out! Are you okay? I think so.");
        assert_eq!(
            texts(result),
            vec!["Watch out!", "Are you okay?", "I think so."]
        );
    }

    // ── paragraph handling ────────────────────────────────────────────────

    #[test]
    fn blank_line_creates_paragraph_boundary() {
        let input = "First paragraph.\n\nSecond paragraph.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec!["First paragraph.", "Second paragraph."]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn multiple_blank_lines_still_one_paragraph_boundary() {
        let input = "First.\n\n\n\nSecond.";
        let result = split(input);
        assert_eq!(texts(result.clone()), vec!["First.", "Second."]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn paragraph_end_only_on_last_sentence_of_paragraph() {
        let input = "One. Two.\n\nThree. Four.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec!["One.", "Two.", "Three.", "Four."]
        );
        assert_eq!(paragraph_ends(&result), vec![false, true, false, true]);
    }

    #[test]
    fn single_newline_is_a_paragraph_boundary() {
        // A bare single `\n` never survives from real markdown-derived
        // plain_text (to_plain_text always uses `\n\n` between blocks), so
        // this is always a paragraph boundary — matching a
        // one-paragraph-per-line plain text format (no blank lines at all).
        let result = split("First line\nSecond line");
        assert_eq!(texts(result.clone()), vec!["First line", "Second line"]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn no_blank_lines_still_splits_into_paragraphs() {
        // One paragraph per line, zero blank lines.
        let input = "First paragraph.\nSecond paragraph.\nThird paragraph.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec!["First paragraph.", "Second paragraph.", "Third paragraph."]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true, true]);
    }

    // ── abbreviations ─────────────────────────────────────────────────────

    #[test]
    fn abbreviation_mr_does_not_cause_false_split() {
        let result = split("Mr. Smith went to the store.");
        assert_eq!(texts(result), vec!["Mr. Smith went to the store."]);
    }

    #[test]
    fn abbreviation_dr_does_not_cause_false_split() {
        let result = split("Dr. Smith made a diagnosis.");
        assert_eq!(texts(result), vec!["Dr. Smith made a diagnosis."]);
    }

    #[test]
    fn abbreviation_mlle_does_not_cause_false_split() {
        let result =
            split("Of Mlle. Germaine Tailleferre one can only repeat Doctor Johnson's dictum.");
        assert_eq!(
            texts(result),
            vec!["Of Mlle. Germaine Tailleferre one can only repeat Doctor Johnson's dictum.",]
        );
    }

    #[test]
    fn abbreviation_mme_does_not_cause_false_split() {
        let result = split("Mme. Curie won two Nobel prizes.");
        assert_eq!(texts(result), vec!["Mme. Curie won two Nobel prizes."]);
    }

    #[test]
    fn abbreviation_no_before_digit_does_not_cause_false_split() {
        // Both "Vol." and "No." are citation abbreviations in ABBREVS, so
        // neither causes a false split before the digit that follows.
        let result = split("Communications of the ACM (Vol. 31, No. 7).");
        assert_eq!(
            texts(result),
            vec!["Communications of the ACM (Vol. 31, No. 7).",]
        );
    }

    #[test]
    fn abbreviation_at_sentence_end_is_not_split() {
        let result = split("She brought food, drinks, etc. Then she left.");
        assert_eq!(
            texts(result),
            vec!["She brought food, drinks, etc. Then she left."]
        );
    }

    #[test]
    fn short_abbreviation_does_not_swallow_period_mid_word() {
        // "p"/"pp" (page citations) are also the tail of ordinary words —
        // a plain substring match on "p." would wrongly hide the period in
        // "shop." or "stop." since neither is preceded by a word boundary.
        let result = split("I need to go to the shop. Then I'll head home.");
        assert_eq!(
            texts(result),
            vec!["I need to go to the shop.", "Then I'll head home.",]
        );
        let result = split("We need to stop. The light is red.");
        assert_eq!(
            texts(result),
            vec!["We need to stop.", "The light is red.",]
        );
    }

    #[test]
    fn short_abbreviation_still_protected_at_a_word_boundary() {
        let result = split("See p. 12 for details.");
        assert_eq!(texts(result), vec!["See p. 12 for details."]);
        let result = split("(p. 12) has the answer.");
        assert_eq!(texts(result), vec!["(p. 12) has the answer."]);
    }

    #[test]
    fn abbreviation_matching_is_case_sensitive() {
        // "Fig" is a citation abbreviation, but "fig" (lowercase) is also
        // an ordinary word — the match must not fire on it, even though it
        // sits at a word boundary.
        let result = split("He ate a fig. Then he left.");
        assert_eq!(texts(result), vec!["He ate a fig.", "Then he left."]);
    }

    #[test]
    fn bible_book_abbreviation_before_chapter_verse_does_not_split() {
        for (text, want) in [
            // A citation in parentheses joins the sentence it cites.
            (
                "Carry each other's burdens. (Gal. 6:1,2 NIV)",
                vec!["Carry each other's burdens. (Gal. 6:1,2 NIV)"],
            ),
            (
                "Carry each other's burdens. Gal. 6:1,2 NIV",
                vec!["Carry each other's burdens.", "Gal. 6:1,2 NIV"],
            ),
            (
                "Children, obey. (Eph. 6:1-4)",
                vec!["Children, obey.", "(Eph. 6:1-4)"],
            ),
            (
                "Love is patient. 1 Cor. 13:4 says so. Then rest.",
                vec!["Love is patient.", "1 Cor. 13:4 says so.", "Then rest."],
            ),
            (
                "Go to him (Matt. 18:15 NIV). Then rest.",
                vec!["Go to him (Matt. 18:15 NIV).", "Then rest."],
            ),
        ] {
            assert_eq!(texts(split(text)), want, "{text}");
        }
    }

    #[test]
    fn bible_book_name_at_a_sentence_end_still_splits() {
        // A full book name takes no period, so "Job." ends a sentence even
        // before a time.
        assert_eq!(
            texts(split("He read Job. Then he slept.")),
            vec!["He read Job.", "Then he slept."]
        );
        assert_eq!(
            texts(split("He read Job. 3:00 AM came.")),
            vec!["He read Job.", "3:00 AM came."]
        );
        // An abbreviation needs a chapter:verse after it.
        assert_eq!(
            texts(split("It is what it Is. Then rest.")),
            vec!["It is what it Is.", "Then rest."]
        );
    }

    #[test]
    fn ed_note_does_not_cause_false_split() {
        let result = split("*Ed. Note: the quotation is an addendum.]");
        assert_eq!(
            texts(result),
            vec!["*Ed. Note: the quotation is an addendum.]"]
        );
        let result = split("[Ed. note: sources are listed by author.]");
        assert_eq!(
            texts(result),
            vec!["[Ed. note: sources are listed by author.]"]
        );
        let result = split("Smith agreed. Ed. Note: this was later retracted.");
        assert_eq!(
            texts(result),
            vec!["Smith agreed.", "Ed. Note: this was later retracted.",]
        );
    }

    #[test]
    fn ed_outside_ed_note_still_splits() {
        // "Ed" is a name as often as an abbreviation, so only the
        // "Ed. Note" phrase is protected.
        let result = split("She called Jo and Ed. Then she left.");
        assert_eq!(
            texts(result),
            vec!["She called Jo and Ed.", "Then she left."]
        );
        let result = split("I asked Ed. Notebooks were on sale.");
        assert_eq!(
            texts(result),
            vec!["I asked Ed.", "Notebooks were on sale."]
        );
    }

    // ── URLs ─────────────────────────────────────────────────────────────

    #[test]
    fn a_url_is_not_split_at_its_own_punctuation() {
        let doi = "https://doi.org/10.1002/1531-8249(200005)47:5%3C614::AID-ANA9%3E3.0.CO;2-N";
        let result = split(&format!("Ann Neurol 2000; 47:614–24. {doi}"));
        assert_eq!(
            texts(result),
            vec![format!("Ann Neurol 2000; 47:614–24. {doi}")]
        );
        let result = split(
            "Retrieved from https://en.wikipedia.org/w/index.php?title=Migraine&oldid=1 today.",
        );
        assert_eq!(
            texts(result),
            vec![
                "Retrieved from https://en.wikipedia.org/w/index.php?title=Migraine&oldid=1 today."
            ]
        );
        let result = split("Watch https://youtube.com/live/X?si=O2Pm. It helps!");
        assert_eq!(
            texts(result),
            vec!["Watch https://youtube.com/live/X?si=O2Pm.", "It helps!"]
        );
    }

    #[test]
    fn a_sentence_still_ends_after_a_url() {
        let result = split("See https://x.org/v3.0.CO. Then go (https://x.org/b). Done.");
        assert_eq!(
            texts(result),
            vec![
                "See https://x.org/v3.0.CO.",
                "Then go (https://x.org/b).",
                "Done."
            ]
        );
        // Only a URL that starts a word: `xhttp://` is not one.
        let result = split("Odd xhttp://v3.0.CO c.");
        assert_eq!(texts(result), vec!["Odd xhttp://v3.0.", "CO c."]);
    }

    // ── middle initials ──────────────────────────────────────────────────

    #[test]
    fn two_middle_initials_do_not_split_the_name() {
        let result = split("E. B. White wrote Charlotte's Web.");
        assert_eq!(texts(result), vec!["E. B. White wrote Charlotte's Web."]);
    }

    #[test]
    fn three_middle_initials_do_not_split_the_name() {
        let result = split("J. R. R. Tolkien wrote The Hobbit.");
        assert_eq!(texts(result), vec!["J. R. R. Tolkien wrote The Hobbit."]);
    }

    #[test]
    fn trailing_initials_citation_does_not_split_mid_name() {
        // "Engelbart, D. C." (surname, then trailing initials) — the
        // fragment before the split ("Engelbart, D.") has real text in it,
        // so it doesn't qualify as a bare-initials-only piece, but it does
        // still end in one, which is enough to merge forward under the
        // deliberate over-merging policy (see `merge_initials`).
        let result = split("Engelbart, D. C. described the framework in detail.");
        assert_eq!(
            texts(result),
            vec!["Engelbart, D. C. described the framework in detail."]
        );
    }

    #[test]
    fn single_letter_grade_at_sentence_end_now_merges_into_next() {
        // Deliberate over-merging tradeoff (see `merge_initials`): a real
        // sentence ending in a single letter merges forward the same as a
        // trailing-initials citation would, since there's no reliable way
        // to tell them apart from the text alone, and a missed split is far
        // cheaper than a mid-name pause.
        let result = split("The correct answer is C. He knew it.");
        assert_eq!(texts(result), vec!["The correct answer is C. He knew it."]);
    }

    #[test]
    fn single_letter_grade_alone_is_not_disturbed() {
        // Nothing follows to merge into, so this is unaffected either way.
        let result = split("She got an A.");
        assert_eq!(texts(result), vec!["She got an A."]);
    }

    // ── ordering ──────────────────────────────────────────────────────────

    #[test]
    fn split_preserves_sentence_order() {
        let result = split("One. Two. Three.");
        assert_eq!(texts(result), vec!["One.", "Two.", "Three."]);
    }

    #[test]
    fn split_multi_paragraph_preserves_order() {
        let input = "Hello world. How are you?\n\nI am fine. Thanks for asking.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec![
                "Hello world.",
                "How are you?",
                "I am fine.",
                "Thanks for asking.",
            ]
        );
        assert_eq!(paragraph_ends(&result), vec![false, true, false, true]);
    }

    // ── outline headers ───────────────────────────────────────────────────

    #[test]
    fn outline_header_merged_with_next() {
        let result = split("I. Introduction\n\nII. Methods");
        assert_eq!(
            texts(result.clone()),
            vec!["I. Introduction", "II. Methods"]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn outline_header_lowercase_next_word() {
        // Lowercase next word — unicode_sentences already keeps as one sentence.
        let result = split("I. introduction\n\nII. methods");
        assert_eq!(
            texts(result.clone()),
            vec!["I. introduction", "II. methods"]
        );
    }

    #[test]
    fn outline_alpha_label_merged() {
        let result = split("A. Background\n\nB. Related Work");
        assert_eq!(
            texts(result.clone()),
            vec!["A. Background", "B. Related Work"]
        );
    }

    #[test]
    fn glued_label_stays_glued_no_space_inserted() {
        // Regression: a real document (Project Gutenberg boilerplate) uses
        // "1.A.", "1.B." numbering — a label glued directly onto the next
        // one with no space at all. merge_outline_labels used to always
        // insert a space when merging ("1." + "A." -> "1. A."), producing
        // text that never existed in the source — so text stored from an
        // earlier split (the original glued "1.A.") no longer matched a
        // fresh split() of the same source (which introduced the space). Merging is now plain concatenation, so a glued source
        // stays glued and a spaced source stays spaced. The glued chain
        // ("1." + "A. ") is itself a label, so it also merges onto the
        // prose that follows.
        let result = split("1.A. By reading or using any part of this work, you agree.");
        assert_eq!(
            texts(result),
            vec!["1.A. By reading or using any part of this work, you agree."]
        );
        // Spaced source is unaffected.
        let result = split("I. Introduction to the topic.");
        assert_eq!(texts(result), vec!["I. Introduction to the topic."]);
    }

    #[test]
    fn lowercase_roman_label_merged() {
        let result = split("i. Introduction\n\nii. Methods\n\niii. Results");
        assert_eq!(
            texts(result.clone()),
            vec!["i. Introduction", "ii. Methods", "iii. Results"]
        );
    }

    #[test]
    fn multi_level_section_number_merged() {
        // Real headings from the Digital Signals Theory book, where every
        // document opens with a `# N.N. Title` heading line. A single-level
        // "8." always merged; a dotted "5.7." used to stay its own sentence.
        let result = split(
            "5.7. Summing sinusoids\n\
             In the previous sections, we have made use of a fact.\n\
             5.7.2.1. Proof\n\
             11.1.3. Special case 1: a at index 0 equals 1\n\
             10.1. Intro text",
        );
        assert_eq!(
            texts(result.clone()),
            vec![
                "5.7. Summing sinusoids",
                "In the previous sections, we have made use of a fact.",
                "5.7.2.1. Proof",
                "11.1.3. Special case 1: a at index 0 equals 1",
                "10.1. Intro text",
            ]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true, true, true, true]);
    }

    #[test]
    fn single_level_number_label_still_merged() {
        let result = split("8. Fast Fourier Transform");
        assert_eq!(texts(result), vec!["8. Fast Fourier Transform"]);
    }

    #[test]
    fn dotted_label_alone_at_paragraph_end_is_unchanged() {
        let result = split("See the next section.\n5.7.");
        assert_eq!(texts(result), vec!["See the next section.", "5.7."]);
        let result = split("Something happened. 5.7.");
        assert_eq!(texts(result), vec!["Something happened.", "5.7."]);
    }

    #[test]
    fn dotted_initials_and_acronyms_merged() {
        // Real shapes from documents in the corpus.
        let result = split("W.E.B. Du B.");
        assert_eq!(texts(result), vec!["W.E.B. Du B."]);
        let result = split("J.B. Braithwaite’s children wrote of their father:");
        assert_eq!(
            texts(result),
            vec!["J.B. Braithwaite’s children wrote of their father:"]
        );
        let result =
            split("U.S. Department of Health and Human Services, Food and Drug Administration.");
        assert_eq!(
            texts(result),
            vec!["U.S. Department of Health and Human Services, Food and Drug Administration."]
        );
        let result = split("N.B. The catalog will tell you.");
        assert_eq!(texts(result), vec!["N.B. The catalog will tell you."]);
    }

    #[test]
    fn glued_label_chain_merges_through_every_piece() {
        // UAX #29 tears these into three pieces ("1." + "E.1. " + prose,
        // "10." + "U.S. " + prose); merging repeats while the accumulated
        // piece is still a label.
        let result = split("1.E.1. The following sentence must appear prominently.");
        assert_eq!(
            texts(result),
            vec!["1.E.1. The following sentence must appear prominently."]
        );
        let result = split("10.U.S. Department of Justice Crime in the United States 2000");
        assert_eq!(
            texts(result),
            vec!["10.U.S. Department of Justice Crime in the United States 2000"]
        );
    }

    #[test]
    fn dotted_non_label_shapes_not_merged() {
        // Mixed case ("Ph") isn't a label part, so this split is unchanged.
        let result = split("Ph.D. Students apply here.");
        assert_eq!(texts(result), vec!["Ph.D.", "Students apply here."]);
    }

    #[test]
    fn is_outline_label_shapes() {
        for s in [
            "I. ", "8. ", "ii.", "5.7. ", "5.2.3.1.", "1.A.", "1.E.1. ", "W.E.B. ", "U.S. ",
            "10.U.S. ",
        ] {
            assert!(is_outline_label(s), "{s:?} should be a label");
        }
        // Lowercase non-numerals ("e", "g", "mix"), mixed case, empty parts,
        // over 4 letters, and anything containing whitespace are not labels.
        for s in [
            "e.g.",
            "i.e.",
            "mix.",
            "Ph.D.",
            "5..7.",
            "A.B.C.D.E.",
            "Wait.",
            "U.S. Army",
            ".",
        ] {
            assert!(!is_outline_label(s), "{s:?} should not be a label");
        }
    }

    #[test]
    fn non_outline_short_sentence_not_merged() {
        // "Wait." is short but ends with real punctuation after a word — not an outline label.
        let result = split("Wait. Are you sure?");
        assert_eq!(texts(result), vec!["Wait.", "Are you sure?"]);
    }

    #[test]
    fn bare_initial_before_parenthetical_year_does_not_strand_the_period() {
        // Regression: a bare initial ("P.") right before a citation year
        // in parens ("(2004)") made `reattach_marker_prefixes` mistake the
        // year for a footnote/citation marker glued onto more prose bundled
        // in the same UAX #29 range — it isn't; the trailing ". " is this
        // sentence's own terminal punctuation. Stripping just "(2004)" and
        // leaving ". " behind stranded that period as its own one-character
        // "sentence", corrupting canonical sentence indices for every
        // sentence after it in the document. Confirmed on a real reference
        // list ("Bacon S. P. (2004). Relative importance...").
        let result = split("Bacon S. P. (2004). Relative importance of temporal information.");
        assert_eq!(
            texts(result),
            vec![
                "Bacon S. P. (2004).",
                "Relative importance of temporal information."
            ]
        );
        // A single non-initial letter before the year doesn't trigger the
        // bug in the first place (UAX #29 doesn't isolate "(2004)." as its
        // own range there) — confirms the fix doesn't change that case.
        let result = split("X (2004). Relative importance.");
        assert_eq!(texts(result), vec!["X (2004).", "Relative importance."]);
    }

    // ── footnote / reference markers ─────────────────────────────────────

    #[test]
    fn footnote_marker_merges_into_preceding_sentence() {
        // A footnote marker that's the last thing in its paragraph reads as
        // belonging to the sentence it cites, not as its own free-standing
        // "sentence" — merged, not split off, regardless of asterisk vs.
        // bracket style or whether it's glued or spaced.
        assert_eq!(
            texts(split("seem promising.*1*")),
            vec!["seem promising.*1*"]
        );
        assert_eq!(
            texts(split("seem promising. *1*")),
            vec!["seem promising. *1*"]
        );
        assert_eq!(
            texts(split("Next sentence. [12]")),
            vec!["Next sentence. [12]"]
        );
        assert_eq!(
            texts(split("Next sentence.[12]")),
            vec!["Next sentence.[12]"]
        );
    }

    #[test]
    fn augment_reference_marker_merges_into_preceding_sentence() {
        // Regression: `is_marker_body` originally only recognized
        // digits/commas/whitespace bodies, so a marker with a leading word
        // token — Augment's own document cross-reference shape
        // (`AUGMENT,71279,`, `OAD,2221,`: identifier, statement number,
        // trailing empty field) — wasn't recognized as a marker at all and
        // came out as its own free-standing "sentence" instead of merging
        // into the citation it belongs to. Real shape from the References
        // section of Engelbart's "Authorship Provisions in AUGMENT" (1984).
        assert_eq!(
            texts(split("...pp. 63-68. (AUGMENT,71279,)")),
            vec!["...pp. 63-68. (AUGMENT,71279,)"]
        );
    }

    #[test]
    fn augment_reference_marker_tolerates_space_after_first_comma() {
        // Regression: the digits segment of the ident,digits,tail check
        // above wasn't trimmed, so a marker typed with a space after the
        // first comma ("AUGMENT, 81010," — a real citation in Engelbart's
        // "Authorship Provisions in AUGMENT", inconsistent with every other citation in
        // the same document) failed the "digits only" check and fell back
        // to being its own free-standing sentence, exactly the bug the
        // no-space case above was already fixed for.
        assert_eq!(
            texts(split("pp. 279-290. (AUGMENT, 81010,)")),
            vec!["pp. 279-290. (AUGMENT, 81010,)"]
        );
    }

    // A reference entry used to tear at every period of its abbreviated
    // journal name (a Nature article's 54 references came out as 178
    // sentences); title and journal now make two sentences per entry.
    #[test]
    fn reference_journal_name_stays_with_its_volume_and_pages() {
        assert_eq!(
            texts(split("2. Krumbholz, K., Patterson, R. D. & Pressnitzer, D. The lower limit of pitch as determined by rate discrimination. J. Acoust. Soc. Am. 108, 1170–1180 (2000).")),
            vec![
                "2. Krumbholz, K., Patterson, R. D. & Pressnitzer, D. The lower limit of pitch as determined by rate discrimination.",
                "J. Acoust. Soc. Am. 108, 1170–1180 (2000).",
            ]
        );
        for (entry, journal) in [
            ("Langner, G. Periodicity coding in the auditory system. Hear. Res. 60, 115–42 (1992).", "Hear. Res. 60, 115–42 (1992)."),
            ("Terhardt, E. On the perception of periodic sound fluctuations (roughness). Acta Acust. united Ac. 30, 201–213 (1974).", "Acta Acust. united Ac. 30, 201–213 (1974)."),
            ("Some title about sound. J. acoust. Soc. Am. 84: 115–123 (1988).", "J. acoust. Soc. Am. 84: 115–123 (1988)."),
            ("Vingen JV, et al. Phonophobia in migraine. Cephalalgia. 2004;24:940–6.", "Cephalalgia. 2004;24:940–6."),
            ("A study of tinnitus outcomes. J Headache Pain. 2015.PMID: 26141381", "J Headache Pain. 2015.PMID: 26141381"),
            ("Old recordings of the brain. Electroencephalogr. Clin. Neurophysiol. 21, 335–344 (1966).", "Electroencephalogr. Clin. Neurophysiol. 21, 335–344 (1966)."),
        ] {
            assert_eq!(texts(split(entry)).last().map(String::as_str), Some(journal), "{entry}");
        }
    }

    // A short capitalized sentence followed by one opening with a number
    // isn't merged unless that number reads as a volume/pages/year.
    #[test]
    fn short_capitalized_sentence_before_a_number_is_not_a_journal_name() {
        let pieces = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for v in [
            &["Done. ", "5 PM came."][..],
            &["Stop. ", "3 CARS."],
            &["We met in Paris. ", "25 (2000) people came."],
        ] {
            assert_eq!(merge_journal_names(pieces(v)), pieces(v), "{v:?}");
        }
        assert!(!is_citation_locator("5 minutes later, we left."));
        assert!(!is_citation_locator("2020 was a hard year."));
        assert!(is_citation_locator("108, 1170–1180 (2000)."));
        assert!(!is_journal_name_piece("We met in Paris."));
        assert!(!is_journal_name_piece(
            "Some physiological characteristics of auditory flutter fusion in man."
        ));
    }

    #[test]
    fn citation_range_marker_stays_with_preceding_sentence() {
        assert_eq!(
            texts(split("Rough sounds are aversive.[4–8] Screams are rough.")),
            vec!["Rough sounds are aversive.[4–8]", "Screams are rough."]
        );
        assert_eq!(
            texts(split(
                "Rough sounds are aversive.[1, 4–8] Screams are rough."
            )),
            vec!["Rough sounds are aversive.[1, 4–8]", "Screams are rough."]
        );
        assert!(is_marker_body("4-8"));
        assert!(!is_marker_body("4–"));
        assert!(!is_marker_body("0.4–2"));
        assert!(!is_marker_body("4–8–9"));
    }

    #[test]
    fn caret_footnote_reference_glued_to_period_stays_with_preceding_sentence() {
        // Regression: Markdown footnote-reference syntax ("[^3]") wasn't a
        // recognized marker shape, so a glued unspaced reference tore into
        // two "sentences" at the period. Downstream, an editor that joins
        // consecutive sentence spans with a literal space
        // landed that space *inside*
        // the marker ("God.[ ^3]") — garbled in the rendered text and, via
        // the same tear reaching the TTS backend as two sentences, in
        // synthesized audio too.
        assert_eq!(
            texts(split(
                "this quest is a search to be in relationship with God.[^3]"
            )),
            vec!["this quest is a search to be in relationship with God.[^3]"]
        );
        // A labeled (non-numeric) footnote reference is the same shape.
        assert_eq!(
            texts(split("as discussed above.[^note1]")),
            vec!["as discussed above.[^note1]"]
        );
    }

    #[test]
    fn citation_field_list_marker_glued_to_period_stays_with_preceding_sentence() {
        // Regression: a real document has citations shaped like
        // "[Fox, Works, 8:42]" (author, title, volume:page) — none of the
        // existing marker shapes matched (middle field is letters, not
        // digits), so this glued marker tore the sentence in half at the
        // period, same class of bug as the AUGMENT and caret-footnote
        // cases above.
        assert_eq!(
            texts(split("is the church.[Fox, Works, 8:42]")),
            vec!["is the church.[Fox, Works, 8:42]"]
        );
        // A page-range locator and multiple comma-separated locators.
        assert_eq!(
            texts(split("were forbidden.[Fox, Works, 7:332, 335-6]")),
            vec!["were forbidden.[Fox, Works, 7:332, 335-6]"]
        );
        // APA and IEEE/Vancouver shapes are the same general rule.
        for marker in [
            "(Smith, 2022, p. 15)",
            "(Smith, 2022)",
            "(Smith et al., 2020)",
            "(Smith & Jones, 2021)",
            "[3, pp. 6-10]",
            "[3, Ch.  2, pp. 7-10]",
            "[3, Fig. 5]",
            "[3, Sec. 4]",
        ] {
            let text = format!("This claim needs support.{marker}");
            assert_eq!(texts(split(&text)), vec![text.clone()], "{marker:?}");
        }
    }

    #[test]
    fn citation_locator_tail_torn_off_by_nested_parenthetical_reattaches() {
        // Regression for doc 3ad6a613-...: a full-bibliography citation
        // whose title ends in a real period, followed by a nested
        // publisher parenthetical and a page locator. `unicode_sentences()`
        // breaks this at "Devotion.", `reattach_marker_openers` reattaches
        // the "(New York: ...)" parenthetical onto that piece, but leaves
        // ", p. 5]" behind as its own range — which used to survive as a
        // standalone "sentence" starting with bare punctuation.
        // Matches the real document's own paragraph structure — the
        // citation is the last thing in its paragraph, and the next
        // sentence starts a new paragraph on the following line. That
        // matters here: it's what leaves ", p. 5]" as the *entire*
        // remainder of its line (nothing after it to bundle with), rather
        // than merged forward into whatever paragraph follows.
        assert_eq!(
            texts(split(
                "history is changed. [Thomas Kelly, A Testament of Devotion. \
                 (New York: Harper & Brothers, 1941), p. 5]\n\
                 I know no other way."
            )),
            vec![
                "history is changed.",
                "[Thomas Kelly, A Testament of Devotion. \
                 (New York: Harper & Brothers, 1941), p. 5]",
                "I know no other way.",
            ]
        );
    }

    #[test]
    fn citation_locator_tail_shapes_from_real_documents() {
        // Same title-period-then-nested-parenthetical shape as the main
        // regression above, exercised against the other real locator
        // shapes seen in the corpus: a hyphenated page range, no space
        // before the period, and an em-dash range.
        for (text, expected) in [
            (
                "So he wrote. [Robert Barclay, An Apology. \
                 (Philadelphia: Friends' Book Store, 1908), pp. 340-341]",
                "[Robert Barclay, An Apology. \
                 (Philadelphia: Friends' Book Store, 1908), pp. 340-341]",
            ),
            (
                "So he wrote. [Penington, Letters. (Second edition, 1975), pp.7-8]",
                "[Penington, Letters. (Second edition, 1975), pp.7-8]",
            ),
            (
                "So he wrote. [William Willimon, A Crisis of Identity. \
                 (Sojourners, 1986), pp. 24\u{2014}28]",
                "[William Willimon, A Crisis of Identity. \
                 (Sojourners, 1986), pp. 24\u{2014}28]",
            ),
        ] {
            let result = texts(split(text));
            assert_eq!(
                result.last().map(String::as_str),
                Some(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn is_citation_locator_tail_rejects_ordinary_prose() {
        // Deliberately narrow: never fires on real sentence content, even
        // content that happens to end with a bracket or mention a page.
        assert!(!is_citation_locator_tail("[Redacted]"));
        assert!(!is_citation_locator_tail("the optional clause is here]"));
        assert!(!is_citation_locator_tail("see page five for details]"));
        assert!(!is_citation_locator_tail("p. 5 is missing the bracket"));
    }

    #[test]
    fn is_citation_locator_tail_accepts_a_bare_number_with_no_keyword() {
        // Not every citation style spells out "p."/"pp." before the
        // locator — some just give the bare number.
        assert!(is_citation_locator_tail(", 5]"));
        assert!(is_citation_locator_tail(", 340-341]"));
    }

    #[test]
    fn is_marker_body_citation_field_list_rejects_plain_prose() {
        // A real parenthetical aside with no digit anywhere — never a
        // citation regardless of how many comma-separated fields it has.
        assert!(!is_marker_body("see appendix"));
        assert!(!is_marker_body("see the appendix, then the index"));
    }

    #[test]
    fn citation_field_list_marker_with_semicolon_and_quoted_title() {
        // Regression: APA-style semicolon-separated multi-source citations
        // (`(Smith, 2020; Jones, 2023)`) and a quoted title standing in
        // for an author both use `;` and/or a quoted span that itself
        // contains commas — a plain comma-split tears through the middle
        // of the quoted title.
        assert_eq!(
            texts(split("This claim has support.(Smith, 2020; Jones, 2023)")),
            vec!["This claim has support.(Smith, 2020; Jones, 2023)"]
        );
        assert_eq!(
            texts(split(
                "the data showed a shift.(\"Understanding Behavioral Economics\", 2024)"
            )),
            vec!["the data showed a shift.(\"Understanding Behavioral Economics\", 2024)"]
        );
        // The real shape this was found from: a quoted work title
        // (containing its own internal commas) followed by a
        // semicolon-separated second reference. Curly quotes, matching
        // the real document.
        let text = "in its Recovery.[Isaac Penington, \u{201C}Of the Church in its First and \
                     Pure State, and its Declining State,\u{201D}; Works, vol. 3: pp.133-7]";
        assert_eq!(texts(split(text)), vec![text.to_string()]);
    }

    #[test]
    fn asterisk_marker_mid_paragraph_no_longer_swallows_next_sentence() {
        // Regression: asterisk isn't in Unicode's "Close" category, so
        // unicode_sentences() places no boundary at all between a marker
        // and whatever prose follows it in the same paragraph — before
        // reattach_marker_prefixes, "*1* Section II discusses more." came
        // out as one bundled sentence, corrupting the real next sentence.
        // Glued and spaced forms hit the exact same bug (asterisk never
        // gets torn from the previous sentence either way — see
        // reattach_marker_prefixes' doc comment); original spacing around
        // the marker is preserved either way, not collapsed.
        assert_eq!(
            texts(split("seem promising.*1* Section II discusses more.")),
            vec!["seem promising.*1*", "Section II discusses more."]
        );
        assert_eq!(
            texts(split("seem promising. *1* Section II discusses more.")),
            vec!["seem promising. *1*", "Section II discusses more."]
        );
    }

    #[test]
    fn spaced_bracket_marker_mid_paragraph_no_longer_swallows_next_sentence() {
        // Regression: a *spaced* bracket marker followed by more prose in
        // the same paragraph isn't contiguous with the previous range, so
        // reattach_marker_openers' tear-repair never fires for it — before
        // reattach_marker_prefixes, "[1] Section II discusses more." came
        // out as one bundled sentence, with the marker read aloud as part
        // of the wrong (following) sentence.
        assert_eq!(
            texts(split("seem promising. [1] Section II discusses more.")),
            vec!["seem promising. [1]", "Section II discusses more."]
        );
    }

    #[test]
    fn consecutive_markers_all_merge_onto_the_same_sentence() {
        // Two back-to-back markers with nothing between them (e.g.
        // duplicate/adjacent citations) shouldn't leave the second one
        // stranded as its own tiny "sentence" once the first is merged away.
        assert_eq!(
            texts(split("seem promising.[1][2] Next sentence.")),
            vec!["seem promising.[1][2]", "Next sentence."]
        );
    }

    #[test]
    fn marker_with_no_preceding_sentence_and_no_following_prose_becomes_its_own_sentence() {
        // A marker as the very first content in its paragraph has no
        // predecessor to merge into, and here nothing follows it in the
        // same range either — nothing to bundle onto, so it becomes its
        // own leading sentence. A synthesizer that skips no-letter
        // sentences can check `is_footnote_marker` to keep this from being
        // silently dropped as content-free.
        assert_eq!(texts(split("*1*")), vec!["*1*"]);
        assert_eq!(texts(split("[1]")), vec!["[1]"]);
    }

    #[test]
    fn marker_with_no_preceding_sentence_stays_bundled_with_following_prose() {
        // A marker as the very first content in its paragraph, followed by
        // more prose in the same range (an outline label like
        // "(1) Concept Manipulation—Humans rose..." or a leading citation
        // like "*1* How are you?"), has no predecessor to merge backward
        // onto — but splitting it off anyway used to leave a short,
        // marker-only "sentence" (e.g. normalizing to just "one") that's
        // prone to failing forced alignment and bleeding into the next
        // segment's audio. Left bundled instead, same as a marker at the
        // end of a paragraph merges backward onto what precedes it.
        assert_eq!(texts(split("*1* How are you?")), vec!["*1* How are you?"]);
        assert_eq!(texts(split("[1] How are you?")), vec!["[1] How are you?"]);
        assert_eq!(
            texts(split(
                "(1) Concept Manipulation\u{2014}Humans rose above the lower forms of life."
            )),
            vec!["(1) Concept Manipulation\u{2014}Humans rose above the lower forms of life."]
        );
    }

    #[test]
    fn unspaced_bracket_marker_stays_with_preceding_sentence() {
        // Regression: a citation marker glued directly onto the period with
        // no space (typical of PDF-extracted footnotes) used to get torn in
        // half by unicode_sentences() — "...Education.[" / "4] I focused...".
        // The marker should end up whole, attached to the sentence it cites.
        let result = split("Higher Education.[4] I focused on the power dynamics.");
        assert_eq!(
            texts(result),
            vec!["Higher Education.[4]", "I focused on the power dynamics."]
        );
    }

    #[test]
    fn unspaced_bracket_marker_real_world_footnote() {
        // Real case from a PDF-imported document.
        let result = split(
            "Every meeting seems to have one of these individuals, and \
             addressing their concerns often requires significant pastoral \
             care efforts, though often without reaching a resolution.[1] \
             This is because the \u{201c}care\u{201d} is an implicit effort \
             to control the behavior of the outrageous Friend, a use of \
             concealed power.",
        );
        let ts = texts(result);
        assert!(ts.iter().any(|s| s.ends_with("a resolution.[1]")));
        assert!(ts.iter().any(|s| s.starts_with("This is because")));
        // No fragment should start mid-marker.
        assert!(!ts.iter().any(|s| s.starts_with("1]")));
    }

    #[test]
    fn unspaced_paren_marker_stays_with_preceding_sentence() {
        // Same UAX #29 quirk applies to parenthetical citations, not just
        // square-bracket ones.
        let result = split("See the report.(2019) Next sentence follows.");
        assert_eq!(
            texts(result),
            vec!["See the report.(2019)", "Next sentence follows."]
        );
    }

    #[test]
    fn unspaced_multi_number_marker_stays_with_preceding_sentence() {
        // Regression: a comma-separated citation list (e.g. "[18, 19]") has
        // whitespace in its body, which the single-number fix originally
        // excluded — leaving it still torn in half. Real case from
        // Engelbart's "Augmenting Human Intellect" (1962).
        let result = split(
            "Ramo has given a number of talks dealing with the future \
             possibilities of computers for \u{201c}extending man\u{2019}s \
             intellect,\u{201d} and wrote several articles.[18, 19] His \
             projections seem slanted more toward larger bodies of humans.",
        );
        let ts = texts(result);
        assert!(ts.iter().any(|s| s.ends_with("articles.[18, 19]")));
        assert!(ts.iter().any(|s| s.starts_with("His projections")));
        assert!(!ts.iter().any(|s| s.starts_with("18, 19]")));
    }

    #[test]
    fn unspaced_three_number_marker_stays_with_preceding_sentence() {
        let result = split("As shown earlier.[1, 2, 3] The results follow.");
        assert_eq!(
            texts(result),
            vec!["As shown earlier.[1, 2, 3]", "The results follow."]
        );
    }

    #[test]
    fn unspaced_bracket_with_letters_is_not_reattached() {
        // A real bracketed clause (contains letters, not just a citation
        // list) that happens to start the next sentence must be left
        // alone — this is not a footnote marker.
        let result = split("End of section.[Redacted] Continue reading here.");
        let ts = texts(result);
        assert!(!ts.iter().any(|s| s.ends_with("[Redacted]")));
    }

    #[test]
    fn reattach_marker_openers_does_not_empty_a_bare_opener_range() {
        // Degenerate case: a "range" that is nothing but the opener itself.
        // Must not be shrunk to empty by the reattach pass.
        let line = "[4] text";
        let ranges = vec![(0, 1), (1, line.len())]; // "[" | "4] text"
        let result = reattach_marker_openers(line, ranges.clone());
        assert_eq!(result, ranges);
    }

    // ── ellipsis ──────────────────────────────────────────────────────────

    #[test]
    fn ellipsis_does_not_split_mid_thought() {
        let result = split("Wait... are you sure? Yes.");
        assert_eq!(texts(result), vec!["Wait... are you sure?", "Yes."]);
    }

    #[test]
    fn no_characters_lost_around_quote_after_spaced_ellipsis() {
        // Regression: unicode_sentences() silently drops the closing quote
        // and period here — neither returned slice contains them — unless
        // recover_dropped_chars reattaches the gap. The quote+period belong
        // with the quoted sentence, not the following one.
        let input = "and then we would say, \"But wait, there's more . . . \". \
                      And then we would play peek-a-boo.";
        let result = texts(split(input));
        assert_eq!(
            result,
            vec![
                "and then we would say, \"But wait, there's more . . . \".",
                "And then we would play peek-a-boo.",
            ]
        );
        // No characters lost: concatenating the sentences (with a single
        // space between, matching how they were originally separated)
        // reconstructs the input exactly.
        assert_eq!(result.join(" "), input);
    }

    #[test]
    fn no_characters_lost_for_trailing_ellipsis_at_end_of_paragraph() {
        // Regression: unicode_sentences() drops a trailing "…" character
        // entirely when it's the last thing on a line, with nothing after
        // it — the returned slice stops at the period, and no slice at all
        // covers the ellipsis. This is a *trailing* gap, distinct from the
        // between-slices case above; recover_dropped_char_ranges needs its
        // own handling for it.
        let input = "Then they told their friends that they \
                      were planning a wedding.…";
        let result = texts(split(input));
        assert_eq!(
            result,
            vec!["Then they told their friends that they were planning a wedding.…"]
        );
    }

    #[test]
    fn trailing_ellipsis_does_not_shift_the_next_paragraph() {
        // Regression: the dropped character above shortened the sentence,
        // which used to make downstream consumers (matching a transcript
        // back to the split sentences) pull in extra text to compensate —
        // cascading a one-sentence drift through the rest of the document.
        // With the character recovered, paragraph boundaries stay put.
        let input = "Sentence ending in an ellipsis.…\n\nA new paragraph entirely.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec![
                "Sentence ending in an ellipsis.…",
                "A new paragraph entirely."
            ]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    // ── cap_sentence_length ──────────────────────────────────────────────

    /// A name made only of letters (digits would add to the estimate), e.g.
    /// `fake_author(0)` -> `"Aaa AB,"`, so each author weighs 3 words
    /// (one surname + two spelled-out initials).
    fn fake_author(i: usize) -> String {
        let letter = |n: usize| (b'a' + (n % 26) as u8) as char;
        let surname = format!(
            "{}{}{}",
            letter(i / 676).to_ascii_uppercase(),
            letter(i / 26),
            letter(i)
        );
        let initials = format!(
            "{}{}",
            letter(i / 3).to_ascii_uppercase(),
            letter(i / 7).to_ascii_uppercase()
        );
        format!("{surname} {initials},")
    }

    fn author_list(n: usize) -> String {
        let authors: Vec<String> = (0..n).map(fake_author).collect();
        // Final author has no trailing comma; the list ends with the entry's
        // one period, as a real reference entry does.
        let mut s = authors.join(" ");
        s.pop();
        format!("{s}.")
    }

    fn estimate(text: &str) -> usize {
        text.split_whitespace().map(estimated_word_weight).sum()
    }

    #[test]
    fn estimated_word_weight_covers_caps_digits_dashes_and_bare_punctuation() {
        assert_eq!(estimated_word_weight("CJ,"), 2);
        assert_eq!(estimated_word_weight("ZAB"), 3);
        // Four letters isn't an acronym spelled out, it's a word.
        assert_eq!(estimated_word_weight("NASA"), 1);
        // One word per digit, over-counting a little on purpose ("2197" is
        // spoken as four words at most, usually two).
        assert_eq!(estimated_word_weight("2197–2223,"), 8);
        assert_eq!(estimated_word_weight("380:"), 3);
        assert_eq!(estimated_word_weight("Barker-Collo"), 2);
        assert_eq!(estimated_word_weight("|"), 0);
        assert_eq!(estimated_word_weight("—"), 0);
        assert_eq!(estimated_word_weight("word."), 1);
    }

    #[test]
    fn sentence_under_threshold_is_untouched() {
        let text = format!("{}.", vec!["word"; MAX_SENTENCE_WORDS - 1].join(" "));
        assert_eq!(texts(split(&text)), vec![text]);
    }

    #[test]
    fn oversize_author_list_is_cut_at_commas_into_bounded_pieces() {
        let list = author_list(300);
        assert!(
            estimate(&list) > MAX_SENTENCE_WORDS,
            "fixture should be oversize"
        );
        let entry = format!(
            "620. {list} Disability-adjusted life years for 291 diseases. Lancet 380: 2197–2223, 2012."
        );
        let result = split(&entry);
        let pieces = texts(result.clone());

        // The list itself became several pieces, then the title and journal.
        assert!(pieces.len() >= 4, "{pieces:?}");
        for piece in &pieces {
            assert!(
                estimate(piece) <= SENTENCE_CUT_TARGET_WORDS + CUT_LOOKAROUND_WORDS,
                "piece too long ({}): {piece:?}",
                estimate(piece)
            );
            assert_eq!(piece.trim(), piece, "pieces must be trimmed");
            assert!(!piece.is_empty());
        }
        // Every author-list piece but the last ends at a comma, never
        // mid-name.
        let list_pieces = &pieces[..pieces.len() - 2];
        for piece in &list_pieces[..list_pieces.len() - 1] {
            assert!(piece.ends_with(','), "should end at a comma: {piece:?}");
        }
        // Nothing lost or reordered.
        let rejoined: Vec<&str> = pieces.iter().flat_map(|p| p.split_whitespace()).collect();
        let original: Vec<&str> = entry.split_whitespace().collect();
        assert_eq!(rejoined, original);
        // One paragraph: only the very last piece ends it.
        let ends = paragraph_ends(&result);
        assert_eq!(ends.iter().filter(|&&e| e).count(), 1);
        assert!(*ends.last().unwrap());
    }

    #[test]
    fn oversize_sentence_with_no_soft_punctuation_still_gets_bounded() {
        let text = format!("{}.", vec!["word"; 700].join(" "));
        let pieces = texts(split(&text));
        assert!(pieces.len() >= 3, "{}", pieces.len());
        for piece in &pieces {
            assert!(estimate(piece) <= SENTENCE_CUT_TARGET_WORDS + CUT_LOOKAROUND_WORDS);
        }
        let rejoined: Vec<&str> = pieces.iter().flat_map(|p| p.split_whitespace()).collect();
        assert_eq!(rejoined, text.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn single_oversize_token_is_left_alone() {
        let text = "9".repeat(400);
        assert_eq!(texts(split(&text)), vec![text]);
    }
}
