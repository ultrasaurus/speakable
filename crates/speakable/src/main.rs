//! `speakable`: write sentence timings from a transcript or an audio file,
//! and demo pages that play them.

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use speakable_normalizer::{parse_overrides, Normalizer};
use speakable_timings::{
    document_sentences, render_demo, timings_from_descript, DescriptTranscript, Timings, UnreadRun,
};

#[derive(Parser)]
#[command(
    name = "speakable",
    version,
    about = "Sentence timings for markdown read aloud"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write sentence timings from a Descript word-level transcript export.
    Descript {
        /// The markdown document that was read aloud.
        doc: PathBuf,
        /// Descript's transcript export (JSON).
        transcript: PathBuf,
        /// The audio file the transcript is of. The timings refer to it by
        /// its path relative to the output file.
        #[arg(long)]
        audio: PathBuf,
        /// Where to write the timings JSON.
        #[arg(short, long)]
        output: PathBuf,
        /// Also write the review report (where the transcript and the text
        /// differ) into the timings JSON.
        #[arg(long)]
        report: bool,
        /// Also write each sentence's words with their times.
        #[arg(long)]
        words: bool,
    },
    /// Write sentence timings by aligning an audio file against its text
    /// (forced alignment).
    Align {
        /// The audio file (mp3 or wav). The timings refer to it by its path
        /// relative to the output file.
        audio: PathBuf,
        /// The document that was read aloud: markdown (`.md`, `.markdown`)
        /// or plain text (anything else, one paragraph per line). Text
        /// overrides in `<doc>.overrides.txt` beside it, when present, are
        /// layered over the normalizer's built-in ones.
        doc: PathBuf,
        /// Where to write the timings JSON.
        #[arg(short, long)]
        output: PathBuf,
        /// Also write the aligner's review report (speech not in the text,
        /// words worth checking) into the timings JSON.
        #[arg(long)]
        report: bool,
        /// Also write each sentence's words with their times.
        #[arg(long)]
        words: bool,
    },
    /// Write a one-file HTML page that highlights each sentence as the
    /// audio plays, from timings or straight from the audio.
    Demo {
        /// The document: markdown (`.md`, `.markdown`) or plain text
        /// (anything else, one paragraph per line).
        doc: PathBuf,
        /// Its timings JSON. Give this or `--audio`.
        #[arg(required_unless_present = "audio")]
        timings: Option<PathBuf>,
        /// Align this audio file (as `align` does) instead of reading timings.
        #[arg(long, conflicts_with = "timings")]
        audio: Option<PathBuf>,
        /// Where to write the page. It refers to the audio by relative path.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Print text in its spoken form, as `align` normalizes it before
    /// aligning ("1945" → "nineteen forty five").
    Normalize {
        /// The text to normalize. Give this or `--doc`.
        #[arg(required_unless_present = "doc", conflicts_with = "doc")]
        text: Option<String>,
        /// Normalize a whole document instead, one numbered line per
        /// sentence, split and numbered as `align` does. Its
        /// `<doc>.overrides.txt`, when present, is applied.
        #[arg(long)]
        doc: Option<PathBuf>,
        /// An overrides file to layer over the built-in table (and over
        /// the document's own overrides file, with `--doc`).
        #[arg(long)]
        overrides: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("speakable: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Descript {
            doc,
            transcript,
            audio,
            output,
            report: with_report,
            words: with_words,
        } => {
            let markdown = read(&doc)?;
            let transcript = DescriptTranscript::from_json(&read(&transcript)?)
                .map_err(|e| format!("{}: not a Descript transcript: {e}", transcript.display()))?;
            let audio_ref = relative_path(&audio, &parent_dir(&output)?)?;
            let mut timings = timings_from_descript(&markdown, &transcript, &audio_ref);
            summarize(&timings, with_report);
            write_timings(&output, timings_kept(&mut timings, with_report, with_words))
        }
        Command::Align {
            audio,
            doc,
            output,
            report: with_report,
            words: with_words,
        } => {
            let markdown = read_doc(&doc)?;
            let audio_ref = relative_path(&audio, &parent_dir(&output)?)?;
            let mut timings = align(&markdown, &doc, &audio, &audio_ref)?;
            summarize(&timings, with_report);
            write_timings(&output, timings_kept(&mut timings, with_report, with_words))
        }
        Command::Demo {
            doc,
            timings: timings_path,
            audio,
            output,
        } => {
            let markdown = read_doc(&doc)?;
            let (timings, audio) = match (timings_path, audio) {
                (_, Some(audio)) => {
                    let timings = align(&markdown, &doc, &audio, "")?;
                    summarize(&timings, false);
                    (timings, audio)
                }
                (Some(timings_path), None) => {
                    let timings = Timings::from_json(&read(&timings_path)?)
                        .map_err(|e| format!("{}: {e}", timings_path.display()))?;
                    timings.check(&markdown).map_err(|e| {
                        format!(
                            "{} doesn't match {}: {e}",
                            timings_path.display(),
                            doc.display()
                        )
                    })?;
                    let audio = parent_dir(&timings_path)?.join(&timings.audio);
                    (timings, audio)
                }
                (None, None) => unreachable!("clap requires timings or --audio"),
            };
            let audio_src = relative_path(&audio, &parent_dir(&output)?)?;
            write(&output, &render_demo(&markdown, &timings, &audio_src))
        }
        Command::Normalize {
            text,
            doc,
            overrides,
        } => {
            let mut normalizer = match &doc {
                Some(doc) => doc_normalizer(doc)?,
                None => Normalizer::builtin(),
            };
            if let Some(path) = &overrides {
                normalizer = normalizer.with_overrides(parse_overrides(&read(path)?));
            }
            match (text, doc) {
                (_, Some(doc)) => print!("{}", normalized_sentences(&read_doc(&doc)?, &normalizer)),
                (Some(text), None) => println!("{}", normalizer.normalize(&text)),
                (None, None) => unreachable!("clap requires text or --doc"),
            }
            Ok(())
        }
    }
}

/// The built-in normalizer plus `<doc>.overrides.txt` when there is one.
fn doc_normalizer(doc: &Path) -> Result<Normalizer, String> {
    let overrides_path = doc.with_extension("overrides.txt");
    match std::fs::read_to_string(&overrides_path) {
        Ok(contents) => Ok(Normalizer::builtin().with_overrides(parse_overrides(&contents))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Normalizer::builtin()),
        Err(e) => Err(format!("{}: {e}", overrides_path.display())),
    }
}

/// Each of the document's sentences in its spoken form, one per line,
/// prefixed with its index: the same sentences and numbers `align` uses,
/// so a sentence named in its warnings can be looked up here.
fn normalized_sentences(markdown: &str, normalizer: &Normalizer) -> String {
    document_sentences(markdown)
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{i}: {}\n", normalizer.normalize(s)))
        .collect()
}

/// Timings for `markdown` from the audio file at `audio`, normalized with
/// the built-in table plus `<doc>.overrides.txt` when there is one.
#[cfg(feature = "align")]
fn align(markdown: &str, doc: &Path, audio: &Path, audio_ref: &str) -> Result<Timings, String> {
    speakable_align::align_audio(markdown, &doc_normalizer(doc)?, audio, audio_ref)
        .map_err(|e| format!("{e:#}"))
}

#[cfg(not(feature = "align"))]
fn align(_markdown: &str, _doc: &Path, _audio: &Path, _audio_ref: &str) -> Result<Timings, String> {
    Err("aligning audio needs the `align` feature; rebuild with it".to_string())
}

/// Drops the report and the words unless asked for.
fn timings_kept(timings: &mut Timings, with_report: bool, with_words: bool) -> &Timings {
    if !with_report {
        timings.report = Default::default();
    }
    if !with_words {
        timings.sentences.iter_mut().for_each(|s| s.words.clear());
    }
    timings
}

fn write_timings(path: &Path, timings: &Timings) -> Result<(), String> {
    write(path, &timings.to_json())
}

/// The document as markdown: a `.md`/`.markdown` file as it is, anything
/// else as plain text, escaped so `*` or `#` in it stays text.
fn read_doc(path: &Path) -> Result<String, String> {
    let text = read(path)?;
    let is_markdown = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    Ok(if is_markdown {
        text
    } else {
        speakable_markdown::from_plain_text(&text)
    })
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Two lines on stderr: how many sentences got times, and how much the
/// report has for review.
fn summarize(t: &Timings, with_report: bool) {
    let untimed = t.sentences.iter().filter(|s| s.times().is_none()).count();
    eprintln!(
        "{} sentences: {} timed, {} untimed",
        t.sentences.len(),
        t.sentences.len() - untimed,
        untimed
    );
    let r = &t.report;
    let hint = if with_report {
        ""
    } else {
        " (for more detail, run with --report)"
    };
    eprintln!(
        "report: {} insertions, {} suspect, {} missing, {} filtered{hint}",
        r.insertions.len(),
        r.suspect.len(),
        r.missing.len(),
        r.filtered.len(),
    );
    for run in t.unread_runs() {
        eprintln!("{}", unread_warning(t, &run));
    }
}

/// One line naming a run of sentences the reader never said: which ones,
/// how they start, and where the audio skips them.
fn unread_warning(t: &Timings, run: &UnreadRun) -> String {
    let (first, last) = (run.sentences.start, run.sentences.end - 1);
    let which = if first == last {
        format!("sentence {first}")
    } else {
        format!("sentences {first}–{last}")
    };
    let at = match (run.after, run.before) {
        (Some(a), Some(b)) => format!("skipped between {} and {}", mmss(a), mmss(b)),
        (None, Some(b)) => format!("skipped before {}", mmss(b)),
        (Some(a), None) => format!("the audio stops after {}", mmss(a)),
        (None, None) => "nothing in the audio was matched".to_string(),
    };
    const OPENING_WORDS: usize = 6;
    let words: Vec<&str> = t.sentences[first].text.split_whitespace().collect();
    let opening = if words.len() > OPENING_WORDS {
        format!("{}…", words[..OPENING_WORDS].join(" "))
    } else {
        words.join(" ")
    };
    format!("warning: {which} not in the audio, left untimed ({at}): \"{opening}\"")
}

/// Seconds as `m:ss.s`.
fn mmss(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

fn parent_dir(file: &Path) -> Result<PathBuf, String> {
    let abs = absolute(file)?;
    Ok(abs.parent().map(Path::to_path_buf).unwrap_or(abs))
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    let abs = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Resolve `.` and `..` by name, so relative paths come out short.
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// `target` as a `/`-separated path relative to the folder `base`.
fn relative_path(target: &Path, base: &Path) -> Result<String, String> {
    let target = absolute(target)?;
    let base = absolute(base)?;
    let t: Vec<Component> = target.components().collect();
    let b: Vec<Component> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".to_string(); b.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_warnings_name_the_sentences_their_opening_and_where() {
        let json = r#"{"version": "0.1", "audio": "a.mp3", "sentences": [
            {"index": 0, "text": "One.", "start": 0.0, "end": 1.0},
            {"index": 1, "text": "We recommend that Meetings and groups begin a discussion."},
            {"index": 2, "text": "Two."},
            {"index": 3, "text": "Three.", "start": 78.7, "end": 80.0},
            {"index": 4, "text": "Four."}
        ]}"#;
        let t = Timings::from_json(json).unwrap();
        let run = |sentences, after, before| UnreadRun {
            sentences,
            after,
            before,
        };
        assert_eq!(
            unread_warning(&t, &run(1..3, Some(77.25), Some(78.7))),
            "warning: sentences 1–2 not in the audio, left untimed \
             (skipped between 1:17.3 and 1:18.7): \"We recommend that Meetings and groups…\""
        );
        assert_eq!(
            unread_warning(&t, &run(4..5, Some(80.0), None)),
            "warning: sentence 4 not in the audio, left untimed \
             (the audio stops after 1:20.0): \"Four.\""
        );
    }

    #[test]
    fn normalized_sentences_are_numbered_as_align_numbers_them() {
        let markdown = "# Title\n\nBorn in 1945. Read Gal. 6:1,2 today.\n";
        assert_eq!(
            normalized_sentences(markdown, &Normalizer::builtin()),
            "0: Title\n\
             1: Born in nineteen forty five.\n\
             2: Read Galatians chapter six verses one and two today.\n"
        );
        let with_override =
            Normalizer::builtin().with_overrides(parse_overrides("Title\tThe Title\n"));
        assert!(normalized_sentences(markdown, &with_override).starts_with("0: The Title\n"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_path(Path::new("/a/b/talk.mp3"), Path::new("/a/b")).unwrap(),
            "talk.mp3"
        );
        assert_eq!(
            relative_path(Path::new("/a/audio/talk.mp3"), Path::new("/a/b")).unwrap(),
            "../audio/talk.mp3"
        );
        assert_eq!(
            relative_path(Path::new("/a/b/./c/../talk.mp3"), Path::new("/a/b/")).unwrap(),
            "talk.mp3"
        );
    }
}
