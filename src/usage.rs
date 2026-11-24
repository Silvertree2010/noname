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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
