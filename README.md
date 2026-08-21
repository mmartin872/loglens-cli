# loglens

A log file is easy to `grep`, but annoying to get a quick shape out of:
how many errors happened, when did the file start and end, how much of
it doesn't even parse as a log line. `loglens` is a small library plus a
CLI that reads a file once and answers those questions.

It expects lines that start with a timestamp token followed by a level
token, which covers a lot of real-world log formats:

```
2026-08-21T10:15:03Z INFO server started on port 8080
2026-08-21T10:15:04Z WARN client disconnected early
2026-08-21T10:16:41Z ERROR failed to write segment: disk full
```

Lines that don't match that shape are counted but not attributed to a
level, so a mixed file (stack traces, blank lines, banners) doesn't
crash the tool, it just shows up in `unparsed_lines`.

## Usage

```
$ loglens server.log
server.log
  total lines:    3
  unparsed lines: 0
  trace: 0
  debug: 0
  info:  1
  warn:  1
  error: 1
  first: 2026-08-21T10:15:03Z
  last:  2026-08-21T10:16:41Z
```

Add `--min-level` to drop everything below a threshold from the counts
and the first/last span - useful for checking whether a file has any
errors without wading through info noise:

```
$ loglens server.log --min-level warn
server.log
  min level:      WARN
  total lines:    3
  unparsed lines: 0
  trace: 0
  debug: 0
  info:  0
  warn:  1
  error: 1
  first: 2026-08-21T10:15:04Z
  last:  2026-08-21T10:16:41Z
```

Lines that don't parse at all still land in `unparsed_lines` regardless
of `--min-level` - the filter only drops recognized levels below the
threshold.

Add `--json` when you want to pipe the result into something else
instead of reading it:

```
$ loglens server.log --json
{"total_lines":3,"unparsed_lines":0,"trace":0,"debug":0,"info":1,"warn":1,"error":1,"first_timestamp":"2026-08-21T10:15:03Z","last_timestamp":"2026-08-21T10:16:41Z"}
```

Both modes are backed by the same `Summary` struct, so the two outputs
never drift apart in what they report.

## As a library

```rust
use loglens::summarize;

let text = std::fs::read_to_string("server.log")?;
let summary = summarize(text.lines());
println!("{} errors seen", summary.error);
```

## Building

Standard library only, no dependencies to fetch:

```
cargo build --release
```

## Status

Early. See the project's issue tracker for what's planned next - a
time range filter on the timestamp, following a file as it grows, and
support for a couple more timestamp shapes are the near-term targets.
