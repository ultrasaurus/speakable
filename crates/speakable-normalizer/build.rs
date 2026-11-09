//! Writes the rustdoc front page: README.md with its links pinned to this
//! version. On GitHub and crates.io the README's relative links and its
//! `docs.rs/<crate>/latest/` links are right, but in the docs for one
//! version they'd show another version's files:
//! - A relative link points at this version's git tag on GitHub
//!   (`v{CARGO_PKG_VERSION}`). Every release is tagged `v{version}`.
//! - A link to this crate's `latest` docs on docs.rs points at this
//!   version's docs instead.

use std::{env, fs, path::Path};

/// The GitHub repository.
const REPO: &str = "https://github.com/ultrasaurus/speakable";
/// This crate's folder within the repository; empty at its root.
const CRATE_DIR: &str = "crates/speakable-normalizer";

fn main() {
    println!("cargo:rerun-if-changed=README.md");
    let readme = fs::read_to_string("README.md").expect("read README.md");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let name = env::var("CARGO_PKG_NAME").unwrap();
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("README.md");
    fs::write(out, with_version_links(&readme, &name, &version)).expect("write README.md");
}

/// `text` with the target of each `[text](target)` link pinned to
/// `version` of the crate `name`.
fn with_version_links(text: &str, name: &str, version: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        let (head, tail) = rest.split_at(i + 2);
        out.push_str(head);
        let end = tail.find(')').unwrap_or(tail.len());
        out.push_str(&pinned(&tail[..end], name, version));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// A link target pinned to `version`:
/// - relative: a GitHub URL at the version's tag, `tree` for a folder
///   (ending in `/`), `blob` for a file
/// - this crate's `latest` docs on docs.rs: the same page at `version`
/// - anything else (other URLs, `#anchors`): unchanged
fn pinned(target: &str, name: &str, version: &str) -> String {
    let latest_docs = format!("https://docs.rs/{name}/latest/");
    if let Some(page) = target.strip_prefix(&latest_docs) {
        return format!("https://docs.rs/{name}/{version}/{page}");
    }
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
    format!("{REPO}/{kind}/v{version}/{}", parts.join("/"))
}
