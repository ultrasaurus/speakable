//! Reading and rewriting the destinations of a document's markdown images.
//!
//! Only the destination text changes, so the plain text (and with it every
//! spoken sentence) comes out the same.

use pulldown_cmark::{Event, Options, Parser, Tag};

/// Where `markdown`'s images point, in document order, repeats included.
pub fn image_destinations(markdown: &str) -> Vec<String> {
    image_spans(markdown).into_iter().map(|(_, dest)| dest).collect()
}

/// The `http(s)` image destinations, each once, in document order.
pub fn remote_image_urls(markdown: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for dest in image_destinations(markdown) {
        if (dest.starts_with("http://") || dest.starts_with("https://")) && !urls.contains(&dest) {
            urls.push(dest);
        }
    }
    urls
}

/// `markdown` with each image destination `replace` returns `Some` for
/// swapped for that value. A link to the same URL is left alone.
pub fn replace_image_destinations(markdown: &str, replace: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut copied = 0;
    for (span, dest) in image_spans(markdown) {
        if let Some(new) = replace(&dest) {
            out.push_str(&markdown[copied..span.start]);
            out.push_str(&new);
            copied = span.end;
        }
    }
    out.push_str(&markdown[copied..]);
    out
}

/// Each image's destination and the byte range of its text in `markdown`.
fn image_spans(markdown: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let mut spans = Vec::new();
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        let Event::Start(Tag::Image { dest_url, .. }) = event else { continue };
        let raw = &markdown[range.clone()];
        // The destination follows the alt text, so the last match is it.
        if let Some(at) = raw.rfind(dest_url.as_ref()) {
            spans.push((range.start + at..range.start + at + dest_url.len(), dest_url.to_string()));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_remote_images_once_and_skips_local_and_data_ones() {
        let md = "![a](https://x.test/a.png) and ![b](/documents/1/images/b.png)\n\n\
                  ![c](https://x.test/a.png) ![d](data:image/png;base64,AAAA) ![e](http://y.test/e.gif \"title\")";
        assert_eq!(remote_image_urls(md), vec!["https://x.test/a.png", "http://y.test/e.gif"]);
    }

    #[test]
    fn replaces_only_image_destinations() {
        let md = "[the figure](https://x.test/a.png)\n\n![alt [1]](https://x.test/a.png \"t\")";
        let out = replace_image_destinations(md, |d| (d == "https://x.test/a.png").then(|| "/documents/1/images/h.png".into()));
        assert_eq!(out, "[the figure](https://x.test/a.png)\n\n![alt [1]](/documents/1/images/h.png \"t\")");
    }

    #[test]
    fn leaves_markdown_alone_when_nothing_matches() {
        let md = "![a](https://x.test/a.png)";
        assert_eq!(replace_image_destinations(md, |_| None), md);
    }
}
