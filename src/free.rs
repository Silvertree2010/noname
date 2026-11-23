//! Total tokens used through the local FreeLLMAPI pool, for the middle of the footer.
//!
//! Off by default (`free-tokens` in the config folder: `on` / `off`). The number comes from the
//! `free-usage --json` command, which reads the pool's request log read-only; noname never opens
//! the log itself and sends nothing over the network. A second option (`claude-tokens`) adds the
//! all-time total of Claude tokens from `claude-usage --json` after it, with the same spacing as
//! the usage bars.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::sys;

/// Seconds between two reads of the log while the option is on.
pub const EVERY: Duration = Duration::from_secs(30);

/// Whether the free-token total is shown. Reads the `free-tokens` config file once and then keeps the answer.
pub fn enabled() -> bool {
    flag(&FR, "free-tokens")
}

/// Turns the free-token total on or off and saves the choice in the `free-tokens` config file.
pub fn set_enabled(on: bool) {
    setflag(&FR, "free-tokens", on)
}

static FR: AtomicU8 = AtomicU8::new(u8::MAX);
static CL: AtomicU8 = AtomicU8::new(u8::MAX);

fn flag(c: &AtomicU8, k: &str) -> bool {
    let mut m = c.load(Relaxed);
    if m == u8::MAX {
        m = std::fs::read_to_string(sys::cfgfile(k)).is_ok_and(|s| s.trim() == "on") as u8;
        c.store(m, Relaxed);
    }
    m == 1
}

fn setflag(c: &AtomicU8, k: &str, on: bool) {
    c.store(on as u8, Relaxed);
    let f = sys::cfgfile(k);
    if let Some(d) = std::path::Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = sys::save(&f, if on { "on" } else { "off" });
}

/// Seconds between two reads of Claude's own total (it only changes when a turn ends).
pub const CLAUDE_EVERY: Duration = Duration::from_secs(60);

/// The second option: the all-time total of Claude tokens, shown after the free total.
pub fn claude_enabled() -> bool {
    flag(&CL, "claude-tokens")
}

/// Turns the Claude token total on or off and saves the choice in the `claude-tokens` config file.
pub fn set_claude_enabled(on: bool) {
    setflag(&CL, "claude-tokens", on)
}

/// Total tokens from the JSON that `claude-usage --json` prints.
pub fn parse_claude(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("total_tokens")?.as_u64()
}

/// All-time tokens from the JSON that `free-usage --json` prints.
pub fn parse(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("periods")?
        .get("all time")?
        .get("tokens_total")?
        .as_u64()
}

/// `2.87M`, `481.7k`, `950`.
pub fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}

fn run(name: &str, env: &str, parse: fn(&str) -> Option<u64>) -> Option<u64> {
    if let Some(c) = std::env::var(env).ok().filter(|c| !c.trim().is_empty()) {
        let mut w = c.split_whitespace();
        let out = Command::new(w.next()?)
            .args(w)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        return if out.status.success() {
            parse(&String::from_utf8_lossy(&out.stdout))
        } else {
            None
        };
    }
    let local = format!("{}/.local/bin/{name}", sys::home());
    let cmd = if std::path::Path::new(&local).exists() {
        local
    } else {
        name.to_string()
    };
    let out = Command::new(cmd)
        .arg("--json")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// One read on a background thread; `None` on the channel means the command is missing or failed.
pub fn fetch() -> Receiver<Option<u64>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(run("free-usage", "NONAME_FREE_CMD", parse));
    });
    rx
}

/// Same for `claude-usage --json`.
pub fn fetch_claude() -> Receiver<Option<u64>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(run("claude-usage", "NONAME_CLAUDE_CMD", parse_claude));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_time_total() {
        let j = r#"{"now":"x","periods":{"today":{"tokens_total":5},"all time":{"tokens_total":2870620}}}"#;
        assert_eq!(parse(j), Some(2_870_620));
    }

    #[test]
    fn rejects_other_shapes() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"periods":{}}"#), None);
        assert_eq!(
            parse(r#"{"periods":{"all time":{"tokens_total":"x"}}}"#),
            None
        );
    }

    #[test]
    fn parses_claude_total() {
        assert_eq!(
            parse_claude(r#"{"total_tokens":8964150783,"through":"2026-11-14"}"#),
            Some(8_964_150_783)
        );
        assert_eq!(parse_claude("{}"), None);
        assert_eq!(parse_claude("nope"), None);
    }

    #[test]
    fn shortens_numbers() {
        assert_eq!(short(950), "950");
        assert_eq!(short(481_673), "481.7k");
        assert_eq!(short(2_870_620), "2.87M");
        assert_eq!(short(3_100_000_000), "3.10B");
        assert_eq!(short(8_964_150_783), "8.96B");
    }
}
