use std::env;
use std::fs;
use std::process::ExitCode;

use loglens::{summarize_with_min_level, Level, Summary};

struct Args {
    path: String,
    json: bool,
    min_level: Option<Level>,
}

fn parse_args() -> Result<Args, String> {
    let mut path = None;
    let mut json = false;
    let mut min_level = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
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
    })
}

fn usage() -> String {
    "usage: loglens <file> [--json] [--min-level LEVEL]".to_string()
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let contents = match fs::read_to_string(&args.path) {
        Ok(contents) => contents,
        Err(err) => {
            eprintln!("loglens: couldn't read {}: {err}", args.path);
            return ExitCode::FAILURE;
        }
    };

    let summary = summarize_with_min_level(contents.lines(), args.min_level);

    if args.json {
        println!("{}", summary.to_json());
    } else {
        print_human(&args.path, &summary, args.min_level);
    }

    ExitCode::SUCCESS
}

fn print_human(path: &str, summary: &Summary, min_level: Option<Level>) {
    println!("{path}");
    if let Some(min_level) = min_level {
        println!("  min level:      {min_level}");
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
}
