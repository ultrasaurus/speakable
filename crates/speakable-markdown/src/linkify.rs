//! Turning DOIs and bare URLs in a document's prose into markdown links.
//!
//! Only the markdown changes: each link's text is the DOI or URL exactly as
//! it was written, so the plain text (and with it every spoken sentence)
//! comes out the same.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

const DOI_RESOLVER: &str = "https://doi.org/";

/// `markdown` with each DOI (`doi:10.1234/abc`, `DOI: 10.1234/abc`,
/// `doi 10.1234/abc`) linked to its `https://doi.org/` URL and each bare
/// `http://` / `https://` URL linked to itself. Only text outside links,
/// images, code and math is touched, and a silent span (`[...]<!--silent-->`)
/// is left as it is, so running this on its own output changes nothing.
pub fn linkify(markdown: &str) -> String {
    let skip = protected_ranges(markdown);
    let mut out = String::with_capacity(markdown.len());
    let mut copied = 0;
    for range in prose_ranges(markdown) {
        for m in find_links(&markdown[range.clone()]) {
            let (start, end) = (range.start + m.start, range.start + m.end);
            if skip.iter().any(|s| start < s.end && s.start < end) {
                continue;
            }
            out.push_str(&markdown[copied..start]);
            let text = &markdown[start..end];
            out.push('[');
            out.push_str(text);
            out.push_str("](");
            push_href(&mut out, &format!("{}{text}", m.href_prefix));
            out.push(')');
            copied = end;
        }
    }
    out.push_str(&markdown[copied..]);
    out
}

/// How many links and how many images `markdown` has, as `(links, images)`
/// — for a report of a markdown-only change, which the sentences don't show.
pub fn link_and_image_counts(markdown: &str) -> (usize, usize) {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let (mut links, mut images) = (0, 0);
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Link { .. }) => links += 1,
            Event::Start(Tag::Image { .. }) => images += 1,
            _ => {}
        }
    }
    (links, images)
}

/// A link destination, in `<...>` when it holds a parenthesis: an
/// unbalanced one ends a bare destination early, and older DOIs contain
/// them (`10.1002/(SICI)1097-...`).
fn push_href(out: &mut String, href: &str) {
    if href.contains(['(', ')']) {
        out.push('<');
        out.push_str(href);
        out.push('>');
    } else {
        out.push_str(href);
    }
}

/// Byte ranges of `markdown`'s plain prose: its text outside links, images
/// and code blocks. Adjacent text events are joined, since the parser can
/// split one run of text (at a `_` or `*` that isn't emphasis, say) and a
/// URL may span the split.
fn prose_ranges(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut nested = 0usize;
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)) => nested += 1,
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => nested -= 1,
            Event::Text(_) if nested == 0 => match ranges.last_mut() {
                Some(last) if last.end == range.start => last.end = range.end,
                _ => ranges.push(range),
            },
            _ => {}
        }
    }
    ranges
}

/// Byte ranges no link may overlap: math spans and silent spans. Neither is
/// visible to the markdown parser, which sees both as ordinary text.
fn protected_ranges(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<_> = crate::find_math_spans(markdown).into_iter().map(|m| m.start..m.end).collect();
    let mut line_start = 0;
    for line in markdown.split_inclusive('\n') {
        // The same spans `remove_silent_spans` drops from the spoken text.
        let mut from = 0;
        while let Some((start, end)) = crate::find_silent_comment(&line[from..]) {
            let before = &line[..from + start];
            let span_start = match before.strip_suffix(']').and_then(|b| b.rfind('[')) {
                Some(open) => open,
                None => from + start,
            };
            ranges.push(line_start + span_start..line_start + from + end);
            from += end;
        }
        line_start += line.len();
    }
    ranges
}

struct LinkMatch {
    /// Byte range of the link text within the searched text.
    start: usize,
    end: usize,
    /// Prepended to the link text to make the href.
    href_prefix: &'static str,
}

/// The DOIs and bare URLs in `text`, in order.
fn find_links(text: &str) -> Vec<LinkMatch> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_word_start = !text[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric());
        if at_word_start {
            if let Some(m) = match_doi(rest).or_else(|| match_url(rest)) {
                found.push(LinkMatch { start: i + m.start, end: i + m.end, href_prefix: m.href_prefix });
                i += m.end;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// A DOI introduced by `doi` and a colon and/or spaces, at the start of
/// `s`. The link text is the DOI alone, without the `doi:` before it.
fn match_doi(s: &str) -> Option<LinkMatch> {
    if !s.get(..3)?.eq_ignore_ascii_case("doi") {
        return None;
    }
    let after_word = &s[3..];
    let after_colon = after_word.strip_prefix(':').unwrap_or(after_word);
    let start = s.len() - after_colon.trim_start_matches([' ', '\t']).len();
    if start == 3 {
        return None; // "doi" glued to what follows, e.g. "doing"
    }
    let len = doi_len(&s[start..])?;
    Some(LinkMatch { start, end: start + len, href_prefix: DOI_RESOLVER })
}

/// Length of the DOI at the start of `s`: Crossref's pattern for modern
/// DOIs, `10.` and a 4-9 digit registrant code, `/`, then a suffix.
fn doi_len(s: &str) -> Option<usize> {
    let after_prefix = s.strip_prefix("10.")?;
    let digits = after_prefix.len() - after_prefix.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if !(4..=9).contains(&digits) {
        return None;
    }
    let suffix = after_prefix[digits..].strip_prefix('/')?;
    let suffix_len = link_end(suffix);
    (suffix_len > 0).then(|| 3 + digits + 1 + suffix_len)
}

/// An `http://` or `https://` URL at the start of `s`.
fn match_url(s: &str) -> Option<LinkMatch> {
    let scheme = ["https://", "http://"].into_iter().find(|p| s.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p)))?;
    let host_len = link_end(&s[scheme.len()..]);
    (host_len > 0).then(|| LinkMatch { start: 0, end: scheme.len() + host_len, href_prefix: "" })
}

/// Length of the link that starts `s`: up to whitespace or a character
/// that would break the markdown link around it, less trailing sentence
/// punctuation and any `)` that closes a parenthesis opened before it.
fn link_end(s: &str) -> usize {
    let mut end = s.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`' | '[' | ']' | '|')).unwrap_or(s.len());
    loop {
        let candidate = &s[..end];
        let Some(last) = candidate.chars().next_back() else { return 0 };
        let unbalanced_close = last == ')' && candidate.matches(')').count() > candidate.matches('(').count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_') || unbalanced_close {
            end -= last.len_utf8();
        } else {
            return end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_links_and_images() {
        let md = "See [a](https://a.org) and ![fig](/x.png), [b](https://b.org).\n";
        assert_eq!(link_and_image_counts(md), (2, 1));
        assert_eq!(link_and_image_counts(&linkify("doi:10.1234/abc")), (1, 0));
    }

    #[test]
    fn links_a_doi() {
        assert_eq!(
            linkify("Brain, 107(4), 1123–1142. doi:10.1093/brain/107.4.1123"),
            "Brain, 107(4), 1123–1142. doi:[10.1093/brain/107.4.1123](https://doi.org/10.1093/brain/107.4.1123)",
        );
    }

    #[test]
    fn doi_variants() {
        assert_eq!(linkify("DOI: 10.1121/1.1908630"), "DOI: [10.1121/1.1908630](https://doi.org/10.1121/1.1908630)");
        assert_eq!(linkify("doi 10.17226/11099"), "doi [10.17226/11099](https://doi.org/10.17226/11099)");
        assert_eq!(
            linkify("doi:10.1111/j.1468-2982.2008.01834.x"),
            "doi:[10.1111/j.1468-2982.2008.01834.x](https://doi.org/10.1111/j.1468-2982.2008.01834.x)",
        );
    }

    #[test]
    fn not_a_doi() {
        for text in ["doing 10.1234/abc", "doi:10.12/abc", "doi:10.1234", "pseudoi:10.1234/abc", "doi: see below"] {
            assert_eq!(linkify(text), text);
        }
    }

    #[test]
    fn trailing_punctuation_stays_outside() {
        assert_eq!(linkify("doi:10.1121/1.1908630."), "doi:[10.1121/1.1908630](https://doi.org/10.1121/1.1908630).");
        assert_eq!(linkify("See https://example.com/a, then"), "See [https://example.com/a](https://example.com/a), then");
        assert_eq!(linkify("(see https://example.com/a)"), "(see [https://example.com/a](https://example.com/a))");
    }

    #[test]
    fn parentheses_in_a_doi() {
        assert_eq!(
            linkify("doi:10.1002/(SICI)1097-4571(199806)49:8<693"),
            "doi:[10.1002/(SICI)1097-4571(199806)49:8](<https://doi.org/10.1002/(SICI)1097-4571(199806)49:8>)<693",
        );
        assert_eq!(
            linkify("https://en.wikipedia.org/wiki/Foo_(bar)"),
            "[https://en.wikipedia.org/wiki/Foo_(bar)](<https://en.wikipedia.org/wiki/Foo_(bar)>)",
        );
    }

    #[test]
    fn links_bare_urls() {
        assert_eq!(
            linkify("Press. doi:10.17226/11099 https://www.ncbi.nlm.nih.gov/books/NBK207834/"),
            "Press. doi:[10.17226/11099](https://doi.org/10.17226/11099) \
             [https://www.ncbi.nlm.nih.gov/books/NBK207834/](https://www.ncbi.nlm.nih.gov/books/NBK207834/)",
        );
        assert_eq!(
            linkify("http://dx.doi.org/10.1111/ene.12296"),
            "[http://dx.doi.org/10.1111/ene.12296](http://dx.doi.org/10.1111/ene.12296)",
        );
        assert_eq!(linkify("a https://x.org/a_b_c b"), "a [https://x.org/a_b_c](https://x.org/a_b_c) b");
    }

    #[test]
    fn leaves_existing_links_alone() {
        for text in [
            "doi:[10.1097/AUD.0000000000001339](https://doi.org/10.1097/aud.0000000000001339)",
            "[https://example.com](https://example.com)",
            "[see here](https://example.com)",
            "<https://example.com>",
            "![doi:10.1234/abc](https://example.com/x.png)",
            "[ref]: https://example.com",
        ] {
            assert_eq!(linkify(text), text);
        }
    }

    #[test]
    fn leaves_code_math_and_silent_alone() {
        for text in [
            "`https://example.com`",
            "```\nhttps://example.com\n```",
            "$x = https://example.com$",
            "[https://example.com]<!--silent--> spoken",
        ] {
            assert_eq!(linkify(text), text);
        }
    }

    #[test]
    fn running_twice_changes_nothing_more() {
        let once = linkify("doi:10.1093/brain/107.4.1123 and https://example.com/(a) and doi:10.1002/(SICI)1097");
        assert_eq!(linkify(&once), once);
    }

    #[test]
    fn spoken_text_is_unchanged() {
        let text = "# Refs\n\n1. Kayan. doi:10.1093/brain/107.4.1123\n2. NRC. doi:10.17226/11099 https://www.ncbi.nlm.nih.gov/books/NBK207834/\n\n| a | https://x.org/t |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(crate::to_plain_text(&linkify(text)), crate::to_plain_text(text));
    }
}
