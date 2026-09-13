//! Core types and parsing for loglens.
//!
//! A log line is expected to start with a timestamp token followed by a
//! level token, e.g. `2026-08-21T10:15:03Z INFO server started`. Anything
//! that doesn't match that shape is still counted toward the total, just
//! not attributed to a level.

use std::fmt;

mod timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn from_token(token: &str) -> Option<Level> {
        match token {
            "TRACE" | "trace" => Some(Level::Trace),
            "DEBUG" | "debug" => Some(Level::Debug),
            "INFO" | "info" => Some(Level::Info),
            "WARN" | "WARNING" | "warn" | "warning" => Some(Level::Warn),
            "ERROR" | "error" => Some(Level::Error),
            _ => None,
        }
    }

    /// Parse a level name from a CLI argument. Accepts the same spellings
    /// as line parsing (including "warning" as an alias for "warn").
    pub fn parse(name: &str) -> Option<Level> {
        Level::from_token(name)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single parsed line.
#[derive(Debug, Clone)]
pub struct Entry {
    pub timestamp: String,
    pub level: Level,
    pub message: String,
}

impl Entry {
    /// Render as a single-line JSON object, one entry at a time - used by
    /// `--follow --json`, where each new line is its own JSON value rather
    /// than an aggregate.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"timestamp\":{},\"level\":\"{}\",\"message\":{}}}",
            json_string(&self.timestamp),
            self.level,
            json_string(&self.message),
        )
    }
}

/// Try to read a timestamp + level + message out of a raw line.
///
/// The parser is deliberately loose: it splits on the first two runs of
/// whitespace and only requires the second token to be a recognized level
/// name. Real log files vary too much in column layout to assume more
/// structure than that.
pub fn parse_line(line: &str) -> Option<Entry> {
    let mut parts = line.splitn(3, char::is_whitespace);
    let timestamp = parts.next()?;
    let level_token = parts.next()?;
    let message = parts.next().unwrap_or("");

    let level = Level::from_token(level_token)?;

    Some(Entry {
        timestamp: timestamp.to_string(),
        level,
        message: message.to_string(),
    })
}

/// Aggregate counts across a set of lines.
#[derive(Debug, Default)]
pub struct Summary {
    pub total_lines: usize,
    pub unparsed_lines: usize,
    pub trace: usize,
    pub debug: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
}

impl Summary {
    pub fn count_for(&self, level: Level) -> usize {
        match level {
            Level::Trace => self.trace,
            Level::Debug => self.debug,
            Level::Info => self.info,
            Level::Warn => self.warn,
            Level::Error => self.error,
        }
    }

    fn record(&mut self, entry: &Entry) {
        match entry.level {
            Level::Trace => self.trace += 1,
            Level::Debug => self.debug += 1,
            Level::Info => self.info += 1,
            Level::Warn => self.warn += 1,
            Level::Error => self.error += 1,
        }
        if self.first_timestamp.is_none() {
            self.first_timestamp = Some(entry.timestamp.clone());
        }
        self.last_timestamp = Some(entry.timestamp.clone());
    }

    /// Render as a single-line JSON object. Hand-rolled because pulling in
    /// serde for eight fields isn't worth taking on a dependency.
    ///
    /// `lines`, when given, is nested under a `"lines"` key holding the
    /// matching entries grouped by level - see `LevelLines`.
    pub fn to_json(&self, lines: Option<&LevelLines>) -> String {
        let mut out = format!(
            "{{\"total_lines\":{},\"unparsed_lines\":{},\"trace\":{},\"debug\":{},\"info\":{},\"warn\":{},\"error\":{},\"first_timestamp\":{},\"last_timestamp\":{}",
            self.total_lines,
            self.unparsed_lines,
            self.trace,
            self.debug,
            self.info,
            self.warn,
            self.error,
            json_opt_string(&self.first_timestamp),
            json_opt_string(&self.last_timestamp),
        );
        if let Some(lines) = lines {
            out.push_str(",\"lines\":");
            out.push_str(&lines.to_json());
        }
        out.push('}');
        out
    }
}

/// Matching entries grouped by level, for callers that want the actual
/// lines rather than just counts - e.g. piping `--json --lines` output
/// through `jq '.lines.error'` to pull out every error line.
#[derive(Debug, Default)]
pub struct LevelLines {
    pub trace: Vec<Entry>,
    pub debug: Vec<Entry>,
    pub info: Vec<Entry>,
    pub warn: Vec<Entry>,
    pub error: Vec<Entry>,
}

impl LevelLines {
    fn record(&mut self, entry: &Entry) {
        match entry.level {
            Level::Trace => self.trace.push(entry.clone()),
            Level::Debug => self.debug.push(entry.clone()),
            Level::Info => self.info.push(entry.clone()),
            Level::Warn => self.warn.push(entry.clone()),
            Level::Error => self.error.push(entry.clone()),
        }
    }

    pub fn for_level(&self, level: Level) -> &[Entry] {
        match level {
            Level::Trace => &self.trace,
            Level::Debug => &self.debug,
            Level::Info => &self.info,
            Level::Warn => &self.warn,
            Level::Error => &self.error,
        }
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"trace\":{},\"debug\":{},\"info\":{},\"warn\":{},\"error\":{}}}",
            json_entry_array(&self.trace),
            json_entry_array(&self.debug),
            json_entry_array(&self.info),
            json_entry_array(&self.warn),
            json_entry_array(&self.error),
        )
    }
}

fn json_entry_array(entries: &[Entry]) -> String {
    let items: Vec<String> = entries.iter().map(Entry::to_json).collect();
    format!("[{}]", items.join(","))
}

fn json_opt_string(value: &Option<String>) -> String {
    match value {
        Some(s) => json_string(s),
        None => "null".to_string(),
    }
}

/// Escape a string for embedding in JSON output. Covers the characters
/// that actually turn up in timestamps and log messages, not the full
/// unicode escape table.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Check whether a parsed entry should be kept under the given level and
/// timestamp-range filters. Shared between the one-shot summary path and
/// `--follow`, so the two never disagree about what counts as "in range".
///
/// `min_level` and `level` are independent: `min_level` is a threshold
/// (this level or above), `level` is an exact match. Passing both is legal
/// - the entry has to satisfy each one that's set.
pub fn passes_filters(
    entry: &Entry,
    min_level: Option<Level>,
    level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
) -> bool {
    let min_level_ok = min_level.map_or(true, |min| entry.level >= min);
    let level_ok = level.map_or(true, |exact| entry.level == exact);
    let since_ok = since.map_or(true, |s| {
        timestamp::compare(&entry.timestamp, s) != std::cmp::Ordering::Less
    });
    let until_ok = until.map_or(true, |u| {
        timestamp::compare(&entry.timestamp, u) != std::cmp::Ordering::Greater
    });
    min_level_ok && level_ok && since_ok && until_ok
}

/// Summarize a full log file's contents, one line at a time.
pub fn summarize<'a, I: Iterator<Item = &'a str>>(lines: I) -> Summary {
    summarize_with_filters(lines, None, None, None)
}

/// Summarize a full log file's contents, dropping any parsed entry below
/// `min_level` from the per-level counts and the first/last timestamp span.
/// Lines that don't parse at all still count toward `unparsed_lines`
/// regardless of the filter - the filter only applies to recognized levels.
pub fn summarize_with_min_level<'a, I: Iterator<Item = &'a str>>(
    lines: I,
    min_level: Option<Level>,
) -> Summary {
    summarize_with_filters(lines, min_level, None, None, None)
}

/// Summarize a full log file's contents, dropping any parsed entry that
/// falls below `min_level`, doesn't exactly match `level`, or falls outside
/// the `[since, until]` timestamp range, from the per-level counts and the
/// first/last timestamp span.
///
/// `since` and `until` are inclusive on both ends. Timestamps in a
/// recognized shape (RFC 3339 with `Z` or a numeric offset, or bare Unix
/// epoch seconds - see the `timestamp` module) are compared chronologically
/// regardless of which of those shapes each side uses. Anything else falls
/// back to a plain string comparison, which only orders correctly for
/// formats that happen to sort the same as text.
///
/// Lines that don't parse at all still count toward `unparsed_lines`
/// regardless of any filter - filters only apply to recognized levels.
pub fn summarize_with_filters<'a, I: Iterator<Item = &'a str>>(
    lines: I,
    min_level: Option<Level>,
    level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
) -> Summary {
    summarize_core(lines, min_level, level, since, until, None)
}

/// Same filtering as `summarize_with_filters`, but also keeps a copy of
/// every entry that passed the filters, grouped by level. Costs more memory
/// than the plain summary, so it's opt-in rather than the default.
pub fn summarize_with_filters_and_lines<'a, I: Iterator<Item = &'a str>>(
    lines: I,
    min_level: Option<Level>,
    level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
) -> (Summary, LevelLines) {
    let mut level_lines = LevelLines::default();
    let summary = summarize_core(lines, min_level, level, since, until, Some(&mut level_lines));
    (summary, level_lines)
}

fn summarize_core<'a, I: Iterator<Item = &'a str>>(
    lines: I,
    min_level: Option<Level>,
    level: Option<Level>,
    since: Option<&str>,
    until: Option<&str>,
    mut collect: Option<&mut LevelLines>,
) -> Summary {
    let mut summary = Summary::default();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        summary.total_lines += 1;
        match parse_line(line) {
            Some(entry) => {
                if passes_filters(&entry, min_level, level, since, until) {
                    summary.record(&entry);
                    if let Some(collector) = collect.as_mut() {
                        collector.record(&entry);
                    }
                }
            }
            None => summary.unparsed_lines += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_line() {
        let entry = parse_line("2026-08-21T10:15:03Z ERROR disk full").unwrap();
        assert_eq!(entry.timestamp, "2026-08-21T10:15:03Z");
        assert_eq!(entry.level, Level::Error);
        assert_eq!(entry.message, "disk full");
    }

    #[test]
    fn rejects_a_line_without_a_recognized_level() {
        assert!(parse_line("2026-08-21T10:15:03Z something happened").is_none());
    }

    #[test]
    fn summary_counts_by_level_and_tracks_span() {
        let text = "2026-08-21T10:00:00Z INFO up\n2026-08-21T10:00:01Z ERROR down\nnot a log line\n";
        let summary = summarize(text.lines());
        assert_eq!(summary.total_lines, 3);
        assert_eq!(summary.info, 1);
        assert_eq!(summary.error, 1);
        assert_eq!(summary.unparsed_lines, 1);
        assert_eq!(summary.first_timestamp.as_deref(), Some("2026-08-21T10:00:00Z"));
        assert_eq!(summary.last_timestamp.as_deref(), Some("2026-08-21T10:00:01Z"));
    }

    #[test]
    fn parses_level_names_case_insensitively() {
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("nonsense"), None);
    }

    #[test]
    fn min_level_drops_lower_levels_from_counts_and_span() {
        let text = "2026-08-21T10:00:00Z INFO up\n2026-08-21T10:00:01Z ERROR down\n2026-08-21T10:00:02Z INFO steady\n";
        let summary = summarize_with_min_level(text.lines(), Some(Level::Error));
        assert_eq!(summary.total_lines, 3);
        assert_eq!(summary.unparsed_lines, 0);
        assert_eq!(summary.info, 0);
        assert_eq!(summary.error, 1);
        assert_eq!(summary.first_timestamp.as_deref(), Some("2026-08-21T10:00:01Z"));
        assert_eq!(summary.last_timestamp.as_deref(), Some("2026-08-21T10:00:01Z"));
    }

    #[test]
    fn min_level_still_counts_unparsed_lines() {
        let text = "not a log line\n2026-08-21T10:00:00Z INFO up\n";
        let summary = summarize_with_min_level(text.lines(), Some(Level::Error));
        assert_eq!(summary.total_lines, 2);
        assert_eq!(summary.unparsed_lines, 1);
        assert_eq!(summary.info, 0);
    }

    #[test]
    fn exact_level_keeps_only_that_level_unlike_min_level() {
        let text = "2026-08-21T10:00:00Z INFO up\n2026-08-21T10:00:01Z WARN careful\n2026-08-21T10:00:02Z ERROR down\n";
        let summary = summarize_with_filters(text.lines(), None, Some(Level::Warn), None, None);
        assert_eq!(summary.total_lines, 3);
        assert_eq!(summary.info, 0);
        assert_eq!(summary.warn, 1);
        assert_eq!(summary.error, 0);
        assert_eq!(
            summary.first_timestamp.as_deref(),
            Some("2026-08-21T10:00:01Z")
        );
    }

    #[test]
    fn exact_level_still_counts_unparsed_lines() {
        let text = "not a log line\n2026-08-21T10:00:00Z INFO up\n";
        let summary = summarize_with_filters(text.lines(), None, Some(Level::Error), None, None);
        assert_eq!(summary.total_lines, 2);
        assert_eq!(summary.unparsed_lines, 1);
        assert_eq!(summary.info, 0);
    }

    #[test]
    fn exact_level_combines_with_time_range() {
        let text = "2026-08-21T10:00:00Z WARN in range\n2026-08-21T12:00:00Z WARN out of range\n2026-08-21T10:30:00Z ERROR in range but wrong level\n";
        let summary = summarize_with_filters(
            text.lines(),
            None,
            Some(Level::Warn),
            Some("2026-08-21T09:00:00Z"),
            Some("2026-08-21T11:00:00Z"),
        );
        assert_eq!(summary.warn, 1);
        assert_eq!(summary.error, 0);
    }

    #[test]
    fn time_range_drops_entries_outside_the_window() {
        let text = "2026-08-21T09:00:00Z INFO early\n2026-08-21T10:00:00Z INFO in range\n2026-08-21T11:00:00Z INFO late\n";
        let summary = summarize_with_filters(
            text.lines(),
            None,
            None,
            Some("2026-08-21T09:30:00Z"),
            Some("2026-08-21T10:30:00Z"),
        );
        assert_eq!(summary.total_lines, 3);
        assert_eq!(summary.info, 1);
        assert_eq!(
            summary.first_timestamp.as_deref(),
            Some("2026-08-21T10:00:00Z")
        );
        assert_eq!(
            summary.last_timestamp.as_deref(),
            Some("2026-08-21T10:00:00Z")
        );
    }

    #[test]
    fn time_range_bounds_are_inclusive() {
        let text = "2026-08-21T10:00:00Z INFO edge\n";
        let summary = summarize_with_filters(
            text.lines(),
            None,
            None,
            Some("2026-08-21T10:00:00Z"),
            Some("2026-08-21T10:00:00Z"),
        );
        assert_eq!(summary.info, 1);
    }

    #[test]
    fn time_range_bound_can_use_a_different_recognized_format_than_the_log() {
        // Log lines are RFC 3339; the --since bound is given as Unix epoch
        // seconds for the same instant as the second line.
        let text = "2026-08-21T09:00:00Z INFO early\n2026-08-21T10:15:03Z INFO on the bound\n2026-08-21T11:00:00Z INFO late\n";
        let summary = summarize_with_filters(text.lines(), None, None, Some("1787307303"), None);
        assert_eq!(summary.info, 2);
        assert_eq!(
            summary.first_timestamp.as_deref(),
            Some("2026-08-21T10:15:03Z")
        );
    }

    #[test]
    fn time_range_combines_with_min_level() {
        let text = "2026-08-21T10:00:00Z ERROR in range but filtered by level\n2026-08-21T12:00:00Z ERROR out of range\n";
        let summary = summarize_with_filters(
            text.lines(),
            Some(Level::Warn),
            None,
            Some("2026-08-21T09:00:00Z"),
            Some("2026-08-21T11:00:00Z"),
        );
        assert_eq!(summary.error, 1);
        assert_eq!(
            summary.first_timestamp.as_deref(),
            Some("2026-08-21T10:00:00Z")
        );
    }

    #[test]
    fn collects_matching_lines_grouped_by_level() {
        let text = "2026-08-21T10:00:00Z INFO up\n2026-08-21T10:00:01Z ERROR down\n2026-08-21T10:00:02Z ERROR still down\nnot a log line\n";
        let (summary, lines) =
            summarize_with_filters_and_lines(text.lines(), None, None, None, None);
        assert_eq!(summary.error, 2);
        assert_eq!(lines.for_level(Level::Info).len(), 1);
        assert_eq!(lines.for_level(Level::Error).len(), 2);
        assert_eq!(lines.for_level(Level::Error)[0].message, "down");
        assert_eq!(lines.for_level(Level::Error)[1].message, "still down");
        assert!(lines.for_level(Level::Debug).is_empty());
    }

    #[test]
    fn collected_lines_respect_the_same_filters_as_the_summary() {
        let text = "2026-08-21T10:00:00Z INFO up\n2026-08-21T10:00:01Z ERROR down\n";
        let (summary, lines) =
            summarize_with_filters_and_lines(text.lines(), Some(Level::Error), None, None, None);
        assert_eq!(summary.info, 0);
        assert!(lines.for_level(Level::Info).is_empty());
        assert_eq!(lines.for_level(Level::Error).len(), 1);
    }

    #[test]
    fn level_lines_to_json_nests_entries_per_level() {
        let text = "2026-08-21T10:00:00Z INFO up\n";
        let (_, lines) = summarize_with_filters_and_lines(text.lines(), None, None, None, None);
        assert_eq!(
            lines.to_json(),
            "{\"trace\":[],\"debug\":[],\"info\":[{\"timestamp\":\"2026-08-21T10:00:00Z\",\"level\":\"INFO\",\"message\":\"up\"}],\"warn\":[],\"error\":[]}"
        );
    }

    #[test]
    fn summary_to_json_nests_lines_when_given() {
        let text = "2026-08-21T10:00:00Z INFO up\n";
        let (summary, lines) =
            summarize_with_filters_and_lines(text.lines(), None, None, None, None);
        let json = summary.to_json(Some(&lines));
        assert!(json.contains("\"lines\":{"));
        assert!(json.contains("\"info\":[{\"timestamp\""));
    }

    #[test]
    fn summary_to_json_omits_lines_when_not_given() {
        let summary = summarize(std::iter::empty());
        assert!(!summary.to_json(None).contains("\"lines\""));
    }
}
