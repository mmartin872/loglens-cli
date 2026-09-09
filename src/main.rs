use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use loglens::{
    parse_line, passes_filters, summarize_with_filters, summarize_with_filters_and_lines, Level,
    LevelLines, Summary,
};

struct Args {
    path: String,
    json: bool,
    min_level: Option<Level>,
    since: Option<String>,
    until: Option<String>,
    follow: bool,
    lines: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut path = None;
    let mut json = false;
    let mut min_level = None;
    let mut since = None;
    let mut until = None;
    let mut follow = false;
    let mut lines = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--follow" | "-f" => follow = true,
            "--lines" => lines = true,
            "-h" | "--help" => return Err(usage()),
            "--min-level" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("--min-level needs a value\n\n{}", usage()))?;
                min_level = Some(
                    Level::parse(&value)
                        .ok_or_else(|| format!("unrecognized level: {value}\n\n{}", usage()))?,
                );
            }
            "--since" => {
                since = Some(
                    args.next()
                        .ok_or_else(|| format!("--since needs a timestamp\n\n{}", usage()))?,
                );
            }
            "--until" => {
                until = Some(
                    args.next()
                        .ok_or_else(|| format!("--until needs a timestamp\n\n{}", usage()))?,
                );
            }
            other if other.starts_with('-') => {
                return Err(format!("unrecognized flag: {other}\n\n{}", usage()))
            }
            other => path = Some(other.to_string()),
        }
    }

    let path = path.ok_or_else(|| format!("missing log file path\n\n{}", usage()))?;
    Ok(Args {
        path,
        json,
        min_level,
        since,
        until,
        follow,
        lines,
    })
}

fn usage() -> String {
    "usage: loglens <file> [--json] [--min-level LEVEL] [--since TIMESTAMP] [--until TIMESTAMP] [--follow] [--lines]"
        .to_string()
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    if args.follow {
        return match run_follow(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("loglens: {err}");
                ExitCode::FAILURE
            }
        };
    }

    let contents = match fs::read_to_string(&args.path) {
        Ok(contents) => contents,
        Err(err) => {
            eprintln!("loglens: couldn't read {}: {err}", args.path);
            return ExitCode::FAILURE;
        }
    };

    let (summary, level_lines) = if args.lines {
        let (summary, level_lines) = summarize_with_filters_and_lines(
            contents.lines(),
            args.min_level,
            args.since.as_deref(),
            args.until.as_deref(),
        );
        (summary, Some(level_lines))
    } else {
        let summary = summarize_with_filters(
            contents.lines(),
            args.min_level,
            args.since.as_deref(),
            args.until.as_deref(),
        );
        (summary, None)
    };

    if args.json {
        println!("{}", summary.to_json(level_lines.as_ref()));
    } else {
        print_human(
            &args.path,
            &summary,
            args.min_level,
            args.since.as_deref(),
            args.until.as_deref(),
            level_lines.as_ref(),
        );
    }

    ExitCode::SUCCESS
}

fn print_human(
    path: &str,
    summary: &Summary,
    min_level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
    level_lines: Option<&LevelLines>,
) {
    println!("{path}");
    if let Some(min_level) = min_level {
        println!("  min level:      {min_level}");
    }
    if let Some(since) = since {
        println!("  since:          {since}");
    }
    if let Some(until) = until {
        println!("  until:          {until}");
    }
    println!("  total lines:    {}", summary.total_lines);
    println!("  unparsed lines: {}", summary.unparsed_lines);
    println!("  trace: {}", summary.trace);
    println!("  debug: {}", summary.debug);
    println!("  info:  {}", summary.info);
    println!("  warn:  {}", summary.warn);
    println!("  error: {}", summary.error);
    if let Some(first) = &summary.first_timestamp {
        println!("  first: {first}");
    }
    if let Some(last) = &summary.last_timestamp {
        println!("  last:  {last}");
    }
    if let Some(level_lines) = level_lines {
        println!("  matching lines:");
        for level in [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ] {
            let entries = level_lines.for_level(level);
            if entries.is_empty() {
                continue;
            }
            println!("    {level}:");
            for entry in entries {
                println!("      {} {}", entry.timestamp, entry.message);
            }
        }
    }
}

/// Poll `args.path` for new content like `tail -f`, printing each matching
/// entry as it shows up instead of waiting to summarize the whole file.
/// Runs until killed - there's no natural end to "watch this file".
fn run_follow(args: &Args) -> Result<(), String> {
    let mut file = open_at_end(&args.path)?;
    let mut pos = file
        .stream_position()
        .map_err(|e| format!("couldn't read position in {}: {e}", args.path))?;
    let mut buffer = String::new();

    loop {
        let len = fs::metadata(&args.path)
            .map_err(|e| format!("couldn't stat {}: {e}", args.path))?
            .len();

        if len < pos {
            // Shrank since we last looked - most likely the file was
            // truncated or replaced by log rotation. Start over from the top
            // rather than seeking to a position that no longer means anything.
            file = File::open(&args.path).map_err(|e| format!("couldn't open {}: {e}", args.path))?;
            pos = 0;
        }

        if len > pos {
            file.seek(SeekFrom::Start(pos))
                .map_err(|e| format!("couldn't seek {}: {e}", args.path))?;
            let mut chunk = String::new();
            let read = file
                .read_to_string(&mut chunk)
                .map_err(|e| format!("couldn't read {}: {e}", args.path))?;
            pos += read as u64;
            buffer.push_str(&chunk);

            for line in split_complete_lines(&mut buffer) {
                print_follow_line(&line, args);
            }
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn open_at_end(path: &str) -> Result<File, String> {
    let mut file = File::open(path).map_err(|e| format!("couldn't open {path}: {e}"))?;
    file.seek(SeekFrom::End(0))
        .map_err(|e| format!("couldn't seek {path}: {e}"))?;
    Ok(file)
}

/// Pull every complete (newline-terminated) line out of `buffer`, leaving
/// any trailing partial line - a write that hasn't finished landing yet -
/// in place for the next poll to complete.
fn split_complete_lines(buffer: &mut String) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(idx) = buffer.find('\n') {
        let line: String = buffer.drain(..=idx).collect();
        let line = line.trim_end_matches(['\n', '\r']);
        lines.push(line.to_string());
    }
    lines
}

fn print_follow_line(line: &str, args: &Args) {
    if line.trim().is_empty() {
        return;
    }
    let entry = match parse_line(line) {
        Some(entry) => entry,
        None => return,
    };
    if !passes_filters(
        &entry,
        args.min_level,
        args.since.as_deref(),
        args.until.as_deref(),
    ) {
        return;
    }
    if args.json {
        println!("{}", entry.to_json());
    } else {
        println!("{} {} {}", entry.timestamp, entry.level, entry.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_complete_lines_leaves_a_trailing_partial_line_in_the_buffer() {
        let mut buffer = String::from("2026-08-21T10:00:00Z INFO up\npartial line still comi");
        let lines = split_complete_lines(&mut buffer);
        assert_eq!(lines, vec!["2026-08-21T10:00:00Z INFO up"]);
        assert_eq!(buffer, "partial line still comi");
    }

    #[test]
    fn split_complete_lines_strips_carriage_returns() {
        let mut buffer = String::from("2026-08-21T10:00:00Z INFO up\r\n");
        let lines = split_complete_lines(&mut buffer);
        assert_eq!(lines, vec!["2026-08-21T10:00:00Z INFO up"]);
        assert_eq!(buffer, "");
    }

    #[test]
    fn split_complete_lines_handles_several_lines_in_one_chunk() {
        let mut buffer = String::from("a\nb\nc\n");
        let lines = split_complete_lines(&mut buffer);
        assert_eq!(lines, vec!["a", "b", "c"]);
    }
}
