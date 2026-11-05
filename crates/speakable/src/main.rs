//! `speakable`: write sentence timings from a transcript or an audio file,
//! and demo pages that play them.

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use speakable_timings::{render_demo, timings_from_descript, DescriptTranscript, Timings};

#[derive(Parser)]
#[command(name = "speakable", version, about = "Sentence timings for markdown read aloud")]
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
        Command::Descript { doc, transcript, audio, output, report: with_report, words: with_words } => {
            let markdown = read(&doc)?;
            let transcript = DescriptTranscript::from_json(&read(&transcript)?)
                .map_err(|e| format!("{}: not a Descript transcript: {e}", transcript.display()))?;
            let audio_ref = relative_path(&audio, &parent_dir(&output)?)?;
            let mut timings = timings_from_descript(&markdown, &transcript, &audio_ref);
            summarize(&timings, with_report);
            write_timings(&output, timings_kept(&mut timings, with_report, with_words))
        }
        Command::Align { audio, doc, output, report: with_report, words: with_words } => {
            let markdown = read_doc(&doc)?;
            let audio_ref = relative_path(&audio, &parent_dir(&output)?)?;
            let mut timings = align(&markdown, &doc, &audio, &audio_ref)?;
            summarize(&timings, with_report);
            write_timings(&output, timings_kept(&mut timings, with_report, with_words))
        }
        Command::Demo { doc, timings: timings_path, audio, output } => {
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
                    timings
                        .check(&markdown)
                        .map_err(|e| format!("{} doesn't match {}: {e}", timings_path.display(), doc.display()))?;
                    let audio = parent_dir(&timings_path)?.join(&timings.audio);
                    (timings, audio)
                }
                (None, None) => unreachable!("clap requires timings or --audio"),
            };
            let audio_src = relative_path(&audio, &parent_dir(&output)?)?;
            write(&output, &render_demo(&markdown, &timings, &audio_src))
        }
    }
}

/// Timings for `markdown` from the audio file at `audio`, normalized with
/// the built-in table plus `<doc>.overrides.txt` when there is one.
#[cfg(feature = "align")]
fn align(markdown: &str, doc: &Path, audio: &Path, audio_ref: &str) -> Result<Timings, String> {
    use speakable_normalizer::{parse_overrides, Normalizer};
    let overrides_path = doc.with_extension("overrides.txt");
    let normalizer = match std::fs::read_to_string(&overrides_path) {
        Ok(contents) => Normalizer::builtin().with_overrides(parse_overrides(&contents)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Normalizer::builtin(),
        Err(e) => return Err(format!("{}: {e}", overrides_path.display())),
    };
    speakable_align::align_audio(markdown, &normalizer, audio, audio_ref).map_err(|e| format!("{e:#}"))
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
    let is_markdown =
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    Ok(if is_markdown { text } else { speakable_markdown::from_plain_text(&text) })
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
    eprintln!("{} sentences: {} timed, {} untimed", t.sentences.len(), t.sentences.len() - untimed, untimed);
    let r = &t.report;
    let hint = if with_report { "" } else { " (for more detail, run with --report)" };
    eprintln!(
        "report: {} insertions, {} suspect, {} missing, {} filtered{hint}",
        r.insertions.len(),
        r.suspect.len(),
        r.missing.len(),
        r.filtered.len(),
    );
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
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path(Path::new("/a/b/talk.mp3"), Path::new("/a/b")).unwrap(), "talk.mp3");
        assert_eq!(relative_path(Path::new("/a/audio/talk.mp3"), Path::new("/a/b")).unwrap(), "../audio/talk.mp3");
        assert_eq!(relative_path(Path::new("/a/b/./c/../talk.mp3"), Path::new("/a/b/")).unwrap(), "talk.mp3");
    }
}
