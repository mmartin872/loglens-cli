use std::env;
use std::fs;
use std::process::ExitCode;

use loglens::{summarize, Summary};

struct Args {
    path: String,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut path = None;
    let mut json = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => return Err(usage()),
            other if other.starts_with('-') => {
                return Err(format!("unrecognized flag: {other}\n\n{}", usage()))
            }
            other => path = Some(other.to_string()),
        }
    }

    let path = path.ok_or_else(|| format!("missing log file path\n\n{}", usage()))?;
    Ok(Args { path, json })
}

fn usage() -> String {
    "usage: loglens <file> [--json]".to_string()
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

    let summary = summarize(contents.lines());

    if args.json {
        println!("{}", summary.to_json());
    } else {
        print_human(&args.path, &summary);
    }

    ExitCode::SUCCESS
}

fn print_human(path: &str, summary: &Summary) {
    println!("{path}");
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
