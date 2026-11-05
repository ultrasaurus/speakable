//! `speakable`: write sentence timings from a transcript, and demo pages
//! that play them.

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
        /// The recording the transcript is of. The timings refer to it by
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
    },
    /// Write a one-file HTML page that highlights each sentence as the
    /// audio plays.
    Demo {
        /// The markdown document.
        doc: PathBuf,
        /// Its timings JSON.
        timings: PathBuf,
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
        Command::Descript { doc, transcript, audio, output, report: with_report } => {
            let markdown = read(&doc)?;
            let transcript = DescriptTranscript::from_json(&read(&transcript)?)
                .map_err(|e| format!("{}: not a Descript transcript: {e}", transcript.display()))?;
            let audio_ref = relative_path(&audio, &parent_dir(&output)?)?;
            let mut timings = timings_from_descript(&markdown, &transcript, &audio_ref);
            summarize(&timings, with_report);
            if !with_report {
                timings.report = Default::default();
            }
            write(&output, &timings.to_json())?;
            Ok(())
        }
        Command::Demo { doc, timings: timings_path, output } => {
            let markdown = read(&doc)?;
            let timings = Timings::from_json(&read(&timings_path)?).map_err(|e| format!("{}: {e}", timings_path.display()))?;
            timings
                .check(&markdown)
                .map_err(|e| format!("{} doesn't match {}: {e}", timings_path.display(), doc.display()))?;
            let audio = parent_dir(&timings_path)?.join(&timings.audio);
            let audio_src = relative_path(&audio, &parent_dir(&output)?)?;
            write(&output, &render_demo(&markdown, &timings, &audio_src))?;
            Ok(())
        }
    }
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
        "report: {} insertions, {} suspect, {} missing{hint}",
        r.insertions.len(),
        r.suspect.len(),
        r.missing.len(),
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
