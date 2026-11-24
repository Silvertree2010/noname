//! The `/usage` panel: plan limits, extra usage and what eats the limits, from Claude's `get_usage` answer.

use serde_json::Value;

/// Seconds since 1970 for an RFC 3339 time such as `2026-11-24T04:30:00.411104+00:00`.
pub(crate) fn epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, se) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days since 1970-01-01 (civil calendar)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut t = days * 86_400 + h * 3600 + mi * 60 + se;
    // the part after the seconds: optional fraction, then Z or +hh:mm / -hh:mm
    let mut r = &s[19..];
    if let Some(f) = r.strip_prefix('.') {
        r = f.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    if let Some(sign) = r.chars().next().filter(|c| *c == '+' || *c == '-') {
        let z: Vec<&str> = r[1..].split(':').collect();
        let oh = z.first()?.parse::<i64>().ok()?;
        let om = z.get(1).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let off = oh * 3600 + om * 60;
        t -= if sign == '+' { off } else { -off };
    }
    Some(t)
}

/// "in 4h 5m", "in 6d 20h" or "now" for the time until `at`.
pub(crate) fn until(at: i64, now: i64) -> String {
    let s = at - now;
    if s <= 0 {
        return "now".into();
    }
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("in {d}d {h}h")
    } else if h > 0 {
        format!("in {h}h {m}m")
    } else {
        format!("in {}m", m.max(1))
    }
}

fn bar(p: f64) -> String {
    let n = ((p.clamp(0.0, 100.0) / 100.0) * 30.0).round() as usize;
    format!("{}{}", "█".repeat(n), "░".repeat(30 - n))
}

/// The colour code of a row by how full it is: `g` green, `y` yellow, `r` red.
fn col(p: f64) -> char {
    if p >= 90.0 {
        'r'
    } else if p >= 70.0 {
        'y'
    } else {
        'g'
    }
}

fn row(v: &mut Vec<(char, String)>, name: &str, p: f64, at: Option<&str>, now: i64) {
    v.push(('w', name.to_string()));
    let r = at
        .and_then(epoch)
        .map(|t| format!("  ·  resets {}", until(t, now)))
        .unwrap_or_default();
    v.push((col(p), format!("{}  {:.0}% used{r}", bar(p), p)));
    v.push((' ', String::new()));
}

/// The panel text as (colour code, line). Colour codes: `h` heading, `w` white, `g`/`y`/`r` bar colours,
/// `m` muted, `f` faint, space for an empty line.
pub(crate) fn lines(p: &Value, now: i64) -> Vec<(char, String)> {
    let mut v: Vec<(char, String)> = vec![];
    let plan = p["subscription_type"].as_str().unwrap_or("");
    if !plan.is_empty() {
        v.push(('m', format!("plan  {plan}")));
        v.push((' ', String::new()));
    }
    let rl = &p["rate_limits"];
    if p["rate_limits_available"] == false || !rl.is_object() {
        v.push((
            'm',
            "no plan limits here (API key, not a Claude subscription)".into(),
        ));
        v.push((' ', String::new()));
    } else if let Some(ls) = rl["limits"].as_array().filter(|l| !l.is_empty()) {
        for l in ls {
            let Some(pc) = l["percent"].as_f64() else {
                continue;
            };
            let name = match l["kind"].as_str().unwrap_or("") {
                "session" => "Current session".to_string(),
                "weekly_all" => "Current week (all models)".to_string(),
                "weekly_scoped" => match l["scope"]["model"]["display_name"].as_str() {
                    Some(m) => format!("Current week ({m} only)"),
                    None => "Current week (scoped)".to_string(),
                },
                k => k.replace('_', " "),
            };
            row(&mut v, &name, pc, l["resets_at"].as_str(), now);
        }
    } else {
        for (k, name) in [
            ("five_hour", "Current session"),
            ("seven_day", "Current week (all models)"),
            ("seven_day_opus", "Current week (Opus only)"),
            ("seven_day_sonnet", "Current week (Sonnet only)"),
        ] {
            if let Some(pc) = rl[k]["utilization"].as_f64() {
                row(&mut v, name, pc, rl[k]["resets_at"].as_str(), now);
            }
        }
    }
    let sp = &rl["spend"];
    if sp["enabled"] == true {
        let used = sp["used"]["amount_minor"].as_f64().unwrap_or(0.0)
            / 10f64.powi(sp["used"]["exponent"].as_i64().unwrap_or(2) as i32);
        let cur = sp["used"]["currency"].as_str().unwrap_or("");
        row(
            &mut v,
            &format!("Extra usage  ({used:.2} {cur} spent)"),
            sp["percent"].as_f64().unwrap_or(0.0),
            None,
            now,
        );
    } else if rl["extra_usage"]["is_enabled"] == false {
        v.push(('w', "Extra usage".into()));
        v.push(('m', "off".into()));
        v.push((' ', String::new()));
    }
    let wk = &p["behaviors"]["week"];
    let recs = wk["recommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !recs.is_empty() {
        let n = wk["request_count"].as_u64().unwrap_or(0);
        let s = wk["session_count"].as_u64().unwrap_or(0);
        v.push(('h', "What is using your limits this week".into()));
        if n > 0 {
            v.push(('f', format!("{n} requests in {s} sessions")));
        }
        for r in recs.iter().take(5) {
            v.push((
                'w',
                format!(
                    "{:>3.0}%  {}",
                    r["pct"].as_f64().unwrap_or(0.0),
                    r["title"].as_str().unwrap_or("")
                ),
            ));
            if let Some(d) = r["detail"].as_str().filter(|d| !d.is_empty()) {
                v.push(('m', format!("      {d}")));
            }
        }
        v.push((' ', String::new()));
    }
    let s = &p["session"];
    if let Some(c) = s["total_cost_usd"].as_f64().filter(|c| *c > 0.0) {
        let a = s["total_lines_added"].as_u64().unwrap_or(0);
        let r = s["total_lines_removed"].as_u64().unwrap_or(0);
        v.push(('h', "This session".into()));
        v.push((
            'm',
            format!("${c:.2}  ·  {a} lines added  ·  {r} lines removed"),
        ));
        v.push((' ', String::new()));
    }
    while v.last().is_some_and(|x| x.0 == ' ') {
        v.pop();
    }
    v
}

/// The tabs of the settings panel, in Claude Code's order. `/status`, `/config`, `/usage` and `/stats` open on their own tab.
pub(crate) const TABS: [&str; 4] = ["Status", "Config", "Usage", "Stats"];

/// The tab index for a slash command name.
pub(crate) fn tab_of(cmd: &str) -> usize {
    match cmd {
        "status" => 0,
        "config" | "settings" => 1,
        "stats" => 3,
        _ => 2,
    }
}

fn kv(v: &mut Vec<(char, String)>, k: &str, val: &str) {
    if !val.is_empty() {
        v.push(('w', format!("{:<18}{val}", format!("{k}:"))));
    }
}

/// What the Status tab shows next to the answers from Claude.
pub(crate) struct Here<'a> {
    pub sid: &'a str,
    pub cwd: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub mode: &'a str,
}

/// The Status tab: version, session, account, model, MCP servers, memory files and setting sources,
/// from the `initialize`, `get_settings` and `mcp_status` answers.
pub(crate) fn status_lines(
    info: &Value,
    set: &Value,
    mcp: &Value,
    h: &Here,
) -> Vec<(char, String)> {
    let mut v: Vec<(char, String)> = vec![];
    let s = |x: &Value| x.as_str().unwrap_or("").to_string();
    kv(&mut v, "Version", &s(&info["claude_code_version"]));
    kv(&mut v, "Session ID", h.sid);
    kv(&mut v, "cwd", h.cwd);
    let ac = &info["account"];
    let plan = s(&ac["subscriptionType"]);
    let login = if !plan.is_empty() {
        format!("{plan} account")
    } else if ac.is_object() {
        "API key".to_string()
    } else {
        String::new()
    };
    kv(&mut v, "Login method", &login);
    kv(&mut v, "Organization", &s(&ac["organization"]));
    kv(&mut v, "Email", &s(&ac["email"]));
    let pr = s(&ac["apiProvider"]);
    if !pr.is_empty() && pr != "firstParty" {
        kv(&mut v, "API provider", &pr);
    }
    v.push((' ', String::new()));
    kv(&mut v, "Model", h.model);
    kv(&mut v, "Effort", h.effort);
    kv(&mut v, "Permission mode", h.mode);
    kv(&mut v, "Output style", &s(&info["output_style"]));
    if let Some(ms) = mcp["mcpServers"].as_array() {
        let ok = ms.iter().filter(|m| m["status"] == "connected").count();
        let bad: Vec<String> = ms
            .iter()
            .filter(|m| m["status"] != "connected" && m["status"] != "disabled")
            .map(|m| format!("{} ({})", s(&m["name"]), s(&m["status"])))
            .collect();
        let mut t = format!("{ok} connected");
        if !bad.is_empty() {
            t += &format!(" · {}", bad.join(", "));
        }
        kv(&mut v, "MCP servers", &t);
    }
    let mem = memory_files(h.cwd);
    if !mem.is_empty() {
        kv(&mut v, "Memory", &mem.join(", "));
    }
    let src: Vec<String> = set["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|x| match x["source"].as_str().unwrap_or("") {
            "userSettings" => "User".to_string(),
            "projectSettings" => "Project".to_string(),
            "localSettings" => "Project local".to_string(),
            "policySettings" => "Managed".to_string(),
            "flagSettings" => "Command line".to_string(),
            o => o.to_string(),
        })
        .collect();
    if !src.is_empty() {
        kv(&mut v, "Setting sources", &src.join(", "));
    }
    if info.is_null() {
        v.push(('m', "loading…".into()));
    }
    v
}

/// The CLAUDE.md files Claude reads for this folder, shortened with `~` and relative paths.
fn memory_files(cwd: &str) -> Vec<String> {
    let home = crate::sys::home();
    let cfg = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| format!("{home}/.claude"));
    let mut v = vec![];
    let mut add = |p: String| {
        if std::path::Path::new(&p).is_file() {
            let r = if let Some(x) = p.strip_prefix(&format!("{cwd}/")) {
                x.to_string()
            } else if let Some(x) = p.strip_prefix(&home).filter(|_| !home.is_empty()) {
                format!("~{x}")
            } else {
                p
            };
            if !v.contains(&r) {
                v.push(r);
            }
        }
    };
    add(format!("{cfg}/CLAUDE.md"));
    // parents first, as Claude loads them, then the folder itself
    let mut dirs: Vec<&std::path::Path> = std::path::Path::new(cwd).ancestors().collect();
    dirs.reverse();
    for d in dirs {
        let d = d.to_string_lossy();
        if d == "/" || d.is_empty() {
            continue;
        }
        add(format!("{d}/CLAUDE.md"));
        add(format!("{d}/.claude/CLAUDE.md"));
        add(format!("{d}/CLAUDE.local.md"));
    }
    v
}

/// One setting value in a line: plain values as they are, lists and objects counted.
fn short_val(x: &Value) -> String {
    match x {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "-".into(),
        Value::Array(a) => format!("{} entries", a.len()),
        Value::Object(o) => format!("{} entries", o.len()),
    }
}

/// The Config tab: the settings in effect (merged from all sources), from the `get_settings` answer.
pub(crate) fn config_lines(set: &Value) -> Vec<(char, String)> {
    let mut v: Vec<(char, String)> = vec![];
    let Some(ef) = set["effective"].as_object() else {
        v.push(('m', "loading…".into()));
        return v;
    };
    let mut plain: Vec<(&String, &Value)> = ef.iter().filter(|(_, x)| !x.is_object()).collect();
    plain.sort_by_key(|x| x.0.to_lowercase());
    for (k, x) in plain {
        v.push(('w', format!("{k:<28}{}", short_val(x))));
    }
    let pm = &ef.get("permissions").cloned().unwrap_or(Value::Null);
    if pm.is_object() {
        v.push((' ', String::new()));
        v.push(('h', "Permissions".into()));
        if let Some(m) = pm["defaultMode"].as_str() {
            v.push(('w', format!("{:<28}{m}", "defaultMode")));
        }
        for k in ["allow", "ask", "deny"] {
            if let Some(a) = pm[k].as_array().filter(|a| !a.is_empty()) {
                v.push(('w', format!("{k:<28}{} rules", a.len())));
                for r in a.iter().take(8) {
                    v.push(('m', format!("  {}", r.as_str().unwrap_or(""))));
                }
                if a.len() > 8 {
                    v.push(('f', format!("  … {} more", a.len() - 8)));
                }
            }
        }
    }
    let mut objs: Vec<(&String, &Value)> = ef
        .iter()
        .filter(|(k, x)| x.is_object() && k.as_str() != "permissions")
        .collect();
    objs.sort_by_key(|x| x.0.to_lowercase());
    if !objs.is_empty() {
        v.push((' ', String::new()));
        v.push(('h', "Other".into()));
        for (k, x) in objs {
            v.push(('w', format!("{k:<28}{}", short_val(x))));
        }
    }
    v.push((' ', String::new()));
    v.push((
        'f',
        "read only here: change settings in ~/.claude/settings.json or with /model, /effort, shift+tab".into(),
    ));
    v
}

/// Days since 1970 for a `YYYY-MM-DD` date (the start of an RFC 3339 time works too).
fn day(s: &str) -> Option<i64> {
    epoch(&format!("{}T00:00:00Z", s.get(..10)?)).map(|t| t.div_euclid(86_400))
}

fn tokens(m: &Value) -> u64 {
    [
        "inputTokens",
        "outputTokens",
        "cacheReadInputTokens",
        "cacheCreationInputTokens",
    ]
    .iter()
    .map(|k| m[*k].as_u64().unwrap_or(0))
    .sum()
}

fn dur(ms: u64) -> String {
    let s = ms / 1000;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// The Stats tab from Claude Code's own `stats-cache.json`: favourite model, tokens, sessions, streaks,
/// active days, peak hour and the top models. `today` is days since 1970; `name` turns a model id into its display name.
pub(crate) fn stats_lines(st: &Value, today: i64, name: fn(&str) -> String) -> Vec<(char, String)> {
    let mut v: Vec<(char, String)> = vec![];
    if !st.is_object() {
        v.push((
            'm',
            "no stats yet: Claude Code writes them to ~/.claude/stats-cache.json".into(),
        ));
        return v;
    }
    let two = |v: &mut Vec<(char, String)>, a: &str, x: String, b: &str, y: String| {
        let l = format!("{:<18}{x}", format!("{a}:"));
        v.push(('w', format!("{l:<40}{:<18}{y}", format!("{b}:"))));
    };
    let mut ms: Vec<(String, &Value)> = st["modelUsage"]
        .as_object()
        .map(|o| o.iter().map(|(k, x)| (k.clone(), x)).collect())
        .unwrap_or_default();
    ms.sort_by_key(|x| std::cmp::Reverse(tokens(x.1)));
    let total: u64 = ms.iter().map(|x| tokens(x.1)).sum();
    if let Some((m, _)) = ms.first() {
        two(
            &mut v,
            "Favorite model",
            name(m),
            "Total tokens",
            crate::free::short(total),
        );
    }
    let ls = &st["longestSession"];
    two(
        &mut v,
        "Sessions",
        crate::free::short(st["totalSessions"].as_u64().unwrap_or(0)),
        "Longest session",
        ls["duration"].as_u64().map(dur).unwrap_or("N/A".into()),
    );
    let mut days: Vec<i64> = st["dailyActivity"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|d| d["messageCount"].as_u64().unwrap_or(0) > 0)
        .filter_map(|d| day(d["date"].as_str().unwrap_or("")))
        .collect();
    days.sort_unstable();
    days.dedup();
    let (mut best, mut run) = (0, 0);
    for (i, d) in days.iter().enumerate() {
        run = if i > 0 && days[i - 1] + 1 == *d {
            run + 1
        } else {
            1
        };
        best = best.max(run);
    }
    // the current streak counts back from today, or from yesterday when today has no activity yet
    let mut cur = 0;
    // when Claude Code has not counted the last days yet, count back from its last counted day instead
    let counted = st["lastComputedDate"]
        .as_str()
        .and_then(day)
        .unwrap_or(today);
    let mut at = if days.last() == Some(&today) {
        today
    } else if counted < today - 1 {
        counted
    } else {
        today - 1
    };
    while days.binary_search(&at).is_ok() {
        cur += 1;
        at -= 1;
    }
    let pl = |n: i64| format!("{n} {}", if n == 1 { "day" } else { "days" });
    two(
        &mut v,
        "Current streak",
        pl(cur),
        "Longest streak",
        pl(best),
    );
    let first = st["firstSessionDate"]
        .as_str()
        .and_then(day)
        .or(days.first().copied());
    let span = first.map(|f| (today - f + 1).max(1)).unwrap_or(0);
    let peak = st["hourCounts"]
        .as_object()
        .and_then(|o| {
            o.iter()
                .filter_map(|(k, x)| Some((k.parse::<u32>().ok()?, x.as_u64()?)))
                .max_by_key(|x| (x.1, std::cmp::Reverse(x.0)))
        })
        .map(|(h, _)| format!("{h}:00-{}:00", h + 1))
        .unwrap_or("N/A".into());
    two(
        &mut v,
        "Active days",
        format!("{}/{span}", days.len()),
        "Peak hour",
        peak,
    );
    if total > 0 {
        let sum = |k: &str| ms.iter().map(|x| x.1[k].as_u64().unwrap_or(0)).sum::<u64>();
        v.push((
            'f',
            format!(
                "Input {} · Output {} · Cache read {} · Cache write {}",
                crate::free::short(sum("inputTokens")),
                crate::free::short(sum("outputTokens")),
                crate::free::short(sum("cacheReadInputTokens")),
                crate::free::short(sum("cacheCreationInputTokens"))
            ),
        ));
    }
    if !ms.is_empty() {
        v.push((' ', String::new()));
        v.push(('h', "Models".into()));
        for (m, x) in ms.iter().take(3) {
            let t = tokens(x);
            let pc = if total > 0 {
                t as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            v.push(('w', format!("• {}  ({pc:.1}%)", name(m))));
            v.push((
                'm',
                format!(
                    "  In: {} · Out: {}",
                    crate::free::short(x["inputTokens"].as_u64().unwrap_or(0)),
                    crate::free::short(x["outputTokens"].as_u64().unwrap_or(0))
                ),
            ));
        }
    }
    v.push((' ', String::new()));
    let mut foot = format!("Stats from the last {}", pl(span));
    if let Some(d) = st["lastComputedDate"].as_str() {
        foot += &format!("  ·  counted up to {d} by Claude Code");
    }
    v.push(('f', foot));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabs_by_command() {
        assert_eq!(TABS[tab_of("status")], "Status");
        assert_eq!(TABS[tab_of("config")], "Config");
        assert_eq!(TABS[tab_of("settings")], "Config");
        assert_eq!(TABS[tab_of("usage")], "Usage");
        assert_eq!(TABS[tab_of("stats")], "Stats");
    }

    #[test]
    fn status_tab() {
        let info = json!({"claude_code_version": "2.1.331", "output_style": "default",
            "account": {"email": "a@b.c", "organization": "Org", "subscriptionType": "Claude Max", "apiProvider": "firstParty"}});
        let set = json!({"sources": [{"source": "userSettings"}, {"source": "localSettings"}]});
        let mcp = json!({"mcpServers": [{"name": "x", "status": "connected"}, {"name": "y", "status": "failed"}]});
        let h = Here {
            sid: "abc",
            cwd: "/nowhere",
            model: "opus 5.5",
            effort: "high",
            mode: "auto",
        };
        let t: Vec<String> = status_lines(&info, &set, &mcp, &h)
            .into_iter()
            .map(|x| x.1)
            .collect();
        let has = |s: &str| t.iter().any(|x| x.contains(s));
        assert!(has("2.1.331") && has("Claude Max account") && has("a@b.c") && has("abc"));
        assert!(has("1 connected · y (failed)"));
        assert!(has("User, Project local"));
        assert!(!has("API provider"));
    }

    #[test]
    fn config_tab() {
        let set = json!({"effective": {"theme": "dark", "autoCompactEnabled": true,
            "permissions": {"allow": ["Bash(ls)"], "defaultMode": "auto"}, "env": {"A": "1"}}});
        let t: Vec<String> = config_lines(&set).into_iter().map(|x| x.1).collect();
        let has = |s: &str| t.iter().any(|x| x.contains(s));
        assert!(has("theme") && has("dark") && has("autoCompactEnabled") && has("true"));
        assert!(has("1 rules") && has("Bash(ls)") && has("env"));
        assert_eq!(config_lines(&Value::Null)[0].1, "loading…");
    }

    #[test]
    fn stats_tab() {
        let today = day("2026-11-24").unwrap_or(0);
        let st = json!({
            "firstSessionDate": "2026-11-15T02:10:31.561Z",
            "lastComputedDate": "2026-11-23",
            "totalSessions": 12,
            "longestSession": {"duration": 3_723_000},
            "hourCounts": {"5": 10, "17": 3},
            "dailyActivity": [
                {"date": "2026-11-16", "messageCount": 3},
                {"date": "2026-11-17", "messageCount": 3},
                {"date": "2026-11-18", "messageCount": 3},
                {"date": "2026-11-22", "messageCount": 1},
                {"date": "2026-11-23", "messageCount": 2}
            ],
            "modelUsage": {
                "claude-haiku-5-5": {"inputTokens": 10, "outputTokens": 10},
                "claude-opus-5-5": {"inputTokens": 1000, "outputTokens": 2000, "cacheReadInputTokens": 7000}
            }
        });
        let t: Vec<String> = stats_lines(&st, today, |m| m.to_uppercase())
            .into_iter()
            .map(|x| x.1)
            .collect();
        let has = |s: &str| t.iter().any(|x| x.contains(s));
        assert!(has("CLAUDE-OPUS-5-5"), "{t:?}");
        assert!(has("Current streak:   2 days"), "{t:?}");
        assert!(has("Longest streak:   3 days"), "{t:?}");
        assert!(has("Active days:      5/10"), "{t:?}");
        assert!(has("Peak hour:        5:00-6:00"), "{t:?}");
        assert!(has("Longest session:  1h 2m"), "{t:?}");
        assert!(has("Stats from the last 10 days"), "{t:?}");
        assert!(
            stats_lines(&Value::Null, 0, |m| m.into())[0]
                .1
                .contains("no stats")
        );
        // a cache last counted 3 days ago: the streak counts back from that day, not from yesterday
        let mut old = st.clone();
        old["lastComputedDate"] = json!("2026-11-18");
        let later = day("2026-11-21").unwrap_or(0);
        let t: Vec<String> = stats_lines(&old, later, |m| m.into())
            .into_iter()
            .map(|x| x.1)
            .collect();
        assert!(
            t.iter().any(|x| x.contains("Current streak:   3 days")),
            "{t:?}"
        );
    }

    #[test]
    fn epoch_of_rfc3339() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2000-03-01T00:00:00+00:00"), Some(951_868_800));
        assert_eq!(
            epoch("2026-11-24T04:30:00.411104+00:00"),
            Some(1_795_494_600)
        );
        assert_eq!(
            epoch("2026-11-24T06:30:00+02:00"),
            epoch("2026-11-24T04:30:00Z")
        );
        assert_eq!(epoch("garbage"), None);
    }

    #[test]
    fn time_until() {
        assert_eq!(until(100, 100), "now");
        assert_eq!(until(100 + 3 * 3600 + 5 * 60, 100), "in 3h 5m");
        assert_eq!(until(100 + 6 * 86_400 + 20 * 3600, 100), "in 6d 20h");
        assert_eq!(until(130, 100), "in 1m");
    }

    #[test]
    fn panel_from_limits() {
        let now = epoch("2026-11-24T00:30:00Z").unwrap_or(0);
        let p = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {
                "limits": [
                    {"kind": "session", "percent": 4, "resets_at": "2026-11-24T04:30:00+00:00"},
                    {"kind": "weekly_all", "percent": 75, "resets_at": "2026-11-30T21:00:00+00:00"},
                    {"kind": "weekly_scoped", "percent": 95, "resets_at": null,
                     "scope": {"model": {"display_name": "Fable"}}}
                ],
                "extra_usage": {"is_enabled": false}
            }
        });
        let l = lines(&p, now);
        let t: Vec<&str> = l.iter().map(|x| x.1.as_str()).collect();
        assert!(t.contains(&"plan  max"));
        assert!(t.contains(&"Current session"));
        assert!(t.contains(&"Current week (Fable only)"));
        assert!(t.iter().any(|x| x.contains("4% used  ·  resets in 4h 0m")));
        assert!(l.iter().any(|x| x.0 == 'y' && x.1.contains("75% used")));
        assert!(l.iter().any(|x| x.0 == 'r' && x.1.contains("95% used")));
        assert!(t.contains(&"off"));
    }

    #[test]
    fn panel_without_plan_limits() {
        let l = lines(&json!({"rate_limits_available": false}), 0);
        assert!(l[0].1.contains("no plan limits"));
    }
}
