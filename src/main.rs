use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
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
    config: Option<String>,
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
    let mut config = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--follow" | "-f" => follow = true,
            "--lines" => lines = true,
            "-h" | "--help" => return Err(usage()),
            "--config" => {
                config = Some(
                    args.next()
                        .ok_or_else(|| format!("--config needs a path\n\n{}", usage()))?,
                );
            }
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
        config,
    })
}

fn usage() -> String {
    "usage: loglens <file> [--json] [--min-level LEVEL] [--level LEVEL] [--since TIMESTAMP] [--until TIMESTAMP] [--grep TEXT] [--follow] [--lines] [--config PATH]\n\n\
     Flags not passed on the command line fall back to environment variables:\n\
     LOGLENS_JSON, LOGLENS_FOLLOW, LOGLENS_LINES (any value other than empty, \"0\", or \"false\" counts as set),\n\
     LOGLENS_MIN_LEVEL, LOGLENS_LEVEL, LOGLENS_SINCE, LOGLENS_UNTIL, LOGLENS_GREP.\n\n\
     Flags still unset after that fall back to --config's file, one KEY=VALUE\n\
     per line (same names as the environment variables above, with or without\n\
     the LOGLENS_ prefix, case-insensitive). Precedence is CLI flag, then\n\
     environment variable, then config file. Without --config, ./.loglensrc is\n\
     used if it exists."
        .to_string()
}

/// Fill in any flag the CLI left unset from a KEY=VALUE source, so a shell
/// profile, wrapper script, or config file can set defaults (e.g.
/// `LOGLENS_MIN_LEVEL=warn`) without every invocation having to repeat them.
/// A flag already set - by an earlier call to this function, or by the CLI -
/// always wins, which is what lets `main` call this once for the environment
/// and again for `--config`'s file and get CLI > env > config file for free.
fn apply_defaults<F>(args: &mut Args, mut lookup: F) -> Result<(), String>
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

/// Read a `--config` file: one `KEY=VALUE` per line, blank lines and `#`
/// comments ignored. Keys match the `LOGLENS_*` environment variable names,
/// with or without the prefix and in any case, so `min_level = warn` and
/// `LOGLENS_MIN_LEVEL = warn` mean the same thing.
fn parse_config_file(path: &str) -> Result<HashMap<String, String>, String> {
    let contents =
        fs::read_to_string(path).map_err(|e| format!("couldn't read config file {path}: {e}"))?;
    let mut values = HashMap::new();
    for (number, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            format!("{path}:{}: expected KEY=VALUE, got: {raw_line}", number + 1)
        })?;
        values.insert(config_key(key), value.trim().to_string());
    }
    Ok(values)
}

/// Looked up in the working directory when `--config` isn't given, so a
/// project can check in its defaults and have them apply without a flag.
const DEFAULT_CONFIG_PATH: &str = ".loglensrc";

/// An explicit `--config` path is always used, and a missing file is then an
/// error worth reporting. The default path is only used if it exists - most
/// directories won't have one, and that shouldn't be a failure.
fn resolve_config_path(explicit: Option<&str>, default: &str) -> Option<String> {
    match explicit {
        Some(path) => Some(path.to_string()),
        None if Path::new(default).is_file() => Some(default.to_string()),
        None => None,
    }
}

fn config_key(raw: &str) -> String {
    let upper = raw.trim().to_ascii_uppercase();
    let stripped = upper.strip_prefix("LOGLENS_").unwrap_or(&upper);
    format!("LOGLENS_{stripped}")
}

fn main() -> ExitCode {
    let mut args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(message) = apply_defaults(&mut args, |name| env::var(name).ok()) {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }

    if let Some(path) = resolve_config_path(args.config.as_deref(), DEFAULT_CONFIG_PATH) {
        let config = match parse_config_file(&path) {
            Ok(config) => config,
            Err(message) => {
                eprintln!("loglens: {message}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(message) = apply_defaults(&mut args, |name| config.get(name).cloned()) {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
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
            config: None,
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
        apply_defaults(
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
        apply_defaults(&mut args, env_map(&[("LOGLENS_MIN_LEVEL", "warn")])).unwrap();
        assert_eq!(args.min_level, Some(Level::Error));
    }

    #[test]
    fn env_bool_flags_treat_zero_and_false_as_unset() {
        let mut args = bare_args("server.log");
        apply_defaults(&mut args, env_map(&[("LOGLENS_JSON", "0")])).unwrap();
        assert!(!args.json);

        let mut args = bare_args("server.log");
        apply_defaults(&mut args, env_map(&[("LOGLENS_LINES", "false")])).unwrap();
        assert!(!args.lines);
    }

    #[test]
    fn env_min_level_rejects_an_unrecognized_level() {
        let mut args = bare_args("server.log");
        let result = apply_defaults(&mut args, env_map(&[("LOGLENS_MIN_LEVEL", "nonsense")]));
        assert!(result.is_err());
    }

    #[test]
    fn config_key_normalizes_prefix_and_case() {
        assert_eq!(config_key("json"), "LOGLENS_JSON");
        assert_eq!(config_key("JSON"), "LOGLENS_JSON");
        assert_eq!(config_key("loglens_json"), "LOGLENS_JSON");
        assert_eq!(config_key("  min_level  "), "LOGLENS_MIN_LEVEL");
    }

    #[test]
    fn parse_config_file_reads_key_value_pairs_and_skips_comments_and_blanks() {
        let path = std::env::temp_dir().join("loglens_test_config_basic.conf");
        fs::write(
            &path,
            "# a comment\n\njson = true\nmin_level=warn\nLOGLENS_GREP = disk full\n",
        )
        .unwrap();
        let values = parse_config_file(path.to_str().unwrap()).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(values.get("LOGLENS_JSON").map(String::as_str), Some("true"));
        assert_eq!(
            values.get("LOGLENS_MIN_LEVEL").map(String::as_str),
            Some("warn")
        );
        assert_eq!(
            values.get("LOGLENS_GREP").map(String::as_str),
            Some("disk full")
        );
    }

    #[test]
    fn parse_config_file_rejects_a_line_without_an_equals_sign() {
        let path = std::env::temp_dir().join("loglens_test_config_bad.conf");
        fs::write(&path, "not a key value line\n").unwrap();
        let result = parse_config_file(path.to_str().unwrap());
        fs::remove_file(&path).unwrap();

        assert!(result.is_err());
    }

    #[test]
    fn explicit_config_path_is_used_even_if_the_file_is_missing() {
        let resolved = resolve_config_path(Some("nope.conf"), "also-missing.conf");
        assert_eq!(resolved.as_deref(), Some("nope.conf"));
    }

    #[test]
    fn default_config_path_is_skipped_when_the_file_does_not_exist() {
        let path = std::env::temp_dir().join("loglens_test_no_such_default.conf");
        let _ = fs::remove_file(&path);
        assert_eq!(resolve_config_path(None, path.to_str().unwrap()), None);
    }

    #[test]
    fn default_config_path_is_used_when_the_file_exists() {
        let path = std::env::temp_dir().join("loglens_test_default_present.conf");
        fs::write(&path, "json = true\n").unwrap();
        let resolved = resolve_config_path(None, path.to_str().unwrap());
        fs::remove_file(&path).unwrap();
        assert_eq!(resolved.as_deref(), path.to_str());
    }

    #[test]
    fn config_file_defaults_only_fill_in_what_env_left_unset() {
        let mut args = bare_args("server.log");
        args.min_level = Some(Level::Error);
        apply_defaults(&mut args, env_map(&[("LOGLENS_JSON", "1")])).unwrap();

        let mut config = HashMap::new();
        config.insert("LOGLENS_MIN_LEVEL".to_string(), "warn".to_string());
        config.insert("LOGLENS_JSON".to_string(), "0".to_string());
        apply_defaults(&mut args, |name| config.get(name).cloned()).unwrap();

        assert_eq!(args.min_level, Some(Level::Error));
        assert!(args.json);
    }
}
