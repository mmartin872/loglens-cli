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
    level: Option<Level>,
    since: Option<String>,
    until: Option<String>,
    follow: bool,
    lines: bool,
    grep: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut path = None;
    let mut json = false;
    let mut min_level = None;
    let mut level = None;
    let mut since = None;
    let mut until = None;
    let mut follow = false;
    let mut lines = false;
    let mut grep = None;

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
            "--level" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("--level needs a value\n\n{}", usage()))?;
                level = Some(
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
            "--grep" | "--message-contains" => {
                grep = Some(
                    args.next()
                        .ok_or_else(|| format!("{arg} needs a value\n\n{}", usage()))?,
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
        level,
        since,
        until,
        follow,
        lines,
        grep,
    })
}

fn usage() -> String {
    "usage: loglens <file> [--json] [--min-level LEVEL] [--level LEVEL] [--since TIMESTAMP] [--until TIMESTAMP] [--grep TEXT] [--follow] [--lines]\n\n\
     Flags not passed on the command line fall back to environment variables:\n\
     LOGLENS_JSON, LOGLENS_FOLLOW, LOGLENS_LINES (any value other than empty, \"0\", or \"false\" counts as set),\n\
     LOGLENS_MIN_LEVEL, LOGLENS_LEVEL, LOGLENS_SINCE, LOGLENS_UNTIL, LOGLENS_GREP."
        .to_string()
}

/// Fill in any flag the CLI left unset from its matching environment
/// variable, so a shell profile or wrapper script can set defaults (e.g.
/// `LOGLENS_MIN_LEVEL=warn`) without every invocation having to repeat them.
/// A flag actually passed on the command line always wins.
fn apply_env_defaults<F>(args: &mut Args, mut lookup: F) -> Result<(), String>
where
    F: FnMut(&str) -> Option<String>,
{
    if !args.json {
        args.json = env_flag(&mut lookup, "LOGLENS_JSON");
    }
    if !args.follow {
        args.follow = env_flag(&mut lookup, "LOGLENS_FOLLOW");
    }
    if !args.lines {
        args.lines = env_flag(&mut lookup, "LOGLENS_LINES");
    }
    if args.min_level.is_none() {
        if let Some(value) = lookup("LOGLENS_MIN_LEVEL") {
            args.min_level = Some(Level::parse(&value).ok_or_else(|| {
                format!("LOGLENS_MIN_LEVEL: unrecognized level: {value}\n\n{}", usage())
            })?);
        }
    }
    if args.level.is_none() {
        if let Some(value) = lookup("LOGLENS_LEVEL") {
            args.level = Some(Level::parse(&value).ok_or_else(|| {
                format!("LOGLENS_LEVEL: unrecognized level: {value}\n\n{}", usage())
            })?);
        }
    }
    if args.since.is_none() {
        args.since = lookup("LOGLENS_SINCE");
    }
    if args.until.is_none() {
        args.until = lookup("LOGLENS_UNTIL");
    }
    if args.grep.is_none() {
        args.grep = lookup("LOGLENS_GREP");
    }
    Ok(())
}

fn env_flag<F>(lookup: &mut F, name: &str) -> bool
where
    F: FnMut(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) => !matches!(value.as_str(), "" | "0" | "false" | "FALSE" | "False"),
        None => false,
    }
}

fn main() -> ExitCode {
    let mut args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(message) = apply_env_defaults(&mut args, |name| env::var(name).ok()) {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }

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
            args.level,
            args.since.as_deref(),
            args.until.as_deref(),
            args.grep.as_deref(),
        );
        (summary, Some(level_lines))
    } else {
        let summary = summarize_with_filters(
            contents.lines(),
            args.min_level,
            args.level,
            args.since.as_deref(),
            args.until.as_deref(),
            args.grep.as_deref(),
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
            args.level,
            args.since.as_deref(),
            args.until.as_deref(),
            args.grep.as_deref(),
            level_lines.as_ref(),
        );
    }

    ExitCode::SUCCESS
}

fn print_human(
    path: &str,
    summary: &Summary,
    min_level: Option<Level>,
    level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
    grep: Option<&str>,
    level_lines: Option<&LevelLines>,
) {
    println!("{path}");
    if let Some(min_level) = min_level {
        println!("  min level:      {min_level}");
    }
    if let Some(level) = level {
        println!("  level:          {level}");
    }
    if let Some(since) = since {
        println!("  since:          {since}");
    }
    if let Some(until) = until {
        println!("  until:          {until}");
    }
    if let Some(grep) = grep {
        println!("  grep:           {grep}");
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
        args.level,
        args.since.as_deref(),
        args.until.as_deref(),
        args.grep.as_deref(),
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

    fn bare_args(path: &str) -> Args {
        Args {
            path: path.to_string(),
            json: false,
            min_level: None,
            level: None,
            since: None,
            until: None,
            follow: false,
            lines: false,
            grep: None,
        }
    }

    fn env_map(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn env_defaults_fill_in_unset_flags() {
        let mut args = bare_args("server.log");
        apply_env_defaults(
            &mut args,
            env_map(&[
                ("LOGLENS_JSON", "1"),
                ("LOGLENS_MIN_LEVEL", "warn"),
                ("LOGLENS_GREP", "disk"),
            ]),
        )
        .unwrap();
        assert!(args.json);
        assert_eq!(args.min_level, Some(Level::Warn));
        assert_eq!(args.grep.as_deref(), Some("disk"));
        assert!(!args.follow);
    }

    #[test]
    fn a_flag_set_on_the_command_line_wins_over_the_environment() {
        let mut args = bare_args("server.log");
        args.min_level = Some(Level::Error);
        apply_env_defaults(&mut args, env_map(&[("LOGLENS_MIN_LEVEL", "warn")])).unwrap();
        assert_eq!(args.min_level, Some(Level::Error));
    }

    #[test]
    fn env_bool_flags_treat_zero_and_false_as_unset() {
        let mut args = bare_args("server.log");
        apply_env_defaults(&mut args, env_map(&[("LOGLENS_JSON", "0")])).unwrap();
        assert!(!args.json);

        let mut args = bare_args("server.log");
        apply_env_defaults(&mut args, env_map(&[("LOGLENS_LINES", "false")])).unwrap();
        assert!(!args.lines);
    }

    #[test]
    fn env_min_level_rejects_an_unrecognized_level() {
        let mut args = bare_args("server.log");
        let result = apply_env_defaults(&mut args, env_map(&[("LOGLENS_MIN_LEVEL", "nonsense")]));
        assert!(result.is_err());
    }
}
