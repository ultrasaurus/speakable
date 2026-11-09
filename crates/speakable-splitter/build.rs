//! Writes the rustdoc front page: README.md with each relative link pointed
//! at this version's git tag on GitHub (`v{CARGO_PKG_VERSION}`). Relative
//! links work when reading the README on GitHub but not in rustdoc, and a
//! link to `main` would show docs for an old release today's files. Every
//! release is tagged `v{version}`, so the tag matches the docs.

use std::{env, fs, path::Path};

/// The GitHub repository.
const REPO: &str = "https://github.com/ultrasaurus/speakable";
/// This crate's folder within the repository; empty at its root.
const CRATE_DIR: &str = "crates/speakable-splitter";

fn main() {
    println!("cargo:rerun-if-changed=README.md");
    let readme = fs::read_to_string("README.md").expect("read README.md");
    let tag = format!("v{}", env::var("CARGO_PKG_VERSION").unwrap());
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("README.md");
    fs::write(out, with_tag_links(&readme, &tag)).expect("write README.md");
}

/// `text` with the target of each `[text](target)` link that's relative
/// made absolute at `tag`.
fn with_tag_links(text: &str, tag: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        let (head, tail) = rest.split_at(i + 2);
        out.push_str(head);
        let end = tail.find(')').unwrap_or(tail.len());
        out.push_str(&at_tag(&tail[..end], tag));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// A relative link target as a GitHub URL at `tag`: `tree` for a folder
/// (ending in `/`), `blob` for a file. Absolute URLs and `#anchors` are
/// left alone.
fn at_tag(target: &str, tag: &str) -> String {
    if target.contains(':') || target.starts_with(['#', '/']) {
        return target.to_string();
    }
    let mut parts: Vec<&str> = CRATE_DIR.split('/').filter(|p| !p.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    let kind = if target.ends_with('/') {
        "tree"
    } else {
        "blob"
    };
    format!("{REPO}/{kind}/{tag}/{}", parts.join("/"))
}
