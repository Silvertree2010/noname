//! What happens to the files of a session when `/clear` starts a new one.
//!
//! Off by default (`clear-files` in the config folder: `off` / `move` / `delete`). Claude Code keeps
//! every session on disk: the transcript `projects/<folder>/<id>.jsonl`, its tool results and
//! subagents in `projects/<folder>/<id>/`, and `file-history/<id>`, `session-env/<id>`,
//! `tasks/<id>`, `todos/<id>*` and `plans/*<id>*`. With `move` they go to one folder per session
//! inside the folder in `clear-folder` (same layout as under `~/.claude`, so a session can be put
//! back); with `delete` they are removed. The old claude process is stopped first, and the work
//! runs on a background thread, so a slow or unplugged drive never blocks the screen. A moved or
//! deleted session can no longer be resumed from `/resume`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use crate::sys;

/// What `/clear` does with the files of the session it leaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Claude keeps the files (the default).
    Off,
    /// Moved to the move folder.
    Move,
    /// Deleted.
    Delete,
}

impl Mode {
    /// The word saved in the config file and shown in the menu.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Move => "move",
            Mode::Delete => "delete",
        }
    }

    fn parse(s: &str) -> Mode {
        match s.trim() {
            "move" => Mode::Move,
            "delete" => Mode::Delete,
            _ => Mode::Off,
        }
    }

    /// off → move → delete → off.
    pub fn next(self) -> Mode {
        match self {
            Mode::Off => Mode::Move,
            Mode::Move => Mode::Delete,
            Mode::Delete => Mode::Off,
        }
    }
}

/// The saved choice; `off` when nothing was chosen.
pub fn mode() -> Mode {
    std::fs::read_to_string(sys::cfgfile("clear-files")).map_or(Mode::Off, |s| Mode::parse(&s))
}

/// Saves the choice in the `clear-files` config file.
pub fn set_mode(m: Mode) {
    save("clear-files", m.name());
}

/// The folder sessions are moved to: the saved one, or `cleared-sessions` in the config folder.
pub fn folder() -> String {
    std::fs::read_to_string(sys::cfgfile("clear-folder"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Path::new(&sys::cfgdir())
                .join("cleared-sessions")
                .to_string_lossy()
                .into_owned()
        })
}

/// Saves the move folder; `~/` is expanded. Returns the folder as saved.
pub fn set_folder(f: &str) -> Result<String, String> {
    let f = f.trim();
    if f.is_empty() {
        return Err("no folder given".into());
    }
    let f = match f.strip_prefix("~/") {
        Some(r) => Path::new(&sys::home())
            .join(r)
            .to_string_lossy()
            .into_owned(),
        None => f.to_string(),
    };
    if !Path::new(&f).is_absolute() {
        return Err("give a full path".into());
    }
    save("clear-folder", &f);
    Ok(f)
}

fn save(k: &str, v: &str) {
    let f = sys::cfgfile(k);
    if let Some(d) = Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = sys::save(&f, v);
}

/// Claude's own folder (`CLAUDE_CONFIG_DIR` or `~/.claude`).
fn base() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(&sys::home()).join(".claude"))
}

/// A session id as Claude writes it (hex and dashes). Anything else is refused, so an odd id can
/// never point outside the session's own files.
fn valid(sid: &str) -> bool {
    sid.len() >= 8 && sid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Every file and folder of session `sid` under `base`, as paths relative to `base`.
fn files(base: &Path, sid: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base.join("projects")) {
        for e in rd.flatten() {
            let p = Path::new("projects").join(e.file_name());
            for n in [format!("{sid}.jsonl"), sid.to_string()] {
                if base.join(&p).join(&n).exists() {
                    v.push(p.join(n));
                }
            }
        }
    }
    for d in ["file-history", "session-env", "tasks"] {
        let p = Path::new(d).join(sid);
        if base.join(&p).exists() {
            v.push(p);
        }
    }
    for (d, starts) in [("todos", true), ("debug", true), ("plans", false)] {
        if let Ok(rd) = std::fs::read_dir(base.join(d)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if (starts && n.starts_with(sid)) || (!starts && n.contains(sid)) {
                    v.push(Path::new(d).join(n));
                }
            }
        }
    }
    v
}

/// `2026-11-24` for a count of days since 1970-01-01 (UTC).
fn ymd(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Moves `from` to `to`: a rename when both are on one drive, else a copy and then a delete.
fn mv(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    cp(from, to)?;
    rm(from)
}

fn cp(from: &Path, to: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(from)?;
    if m.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            cp(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else if m.file_type().is_symlink() {
        // a link is not followed; the link itself is left out of the copy
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn rm(p: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(p)?.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// Moves or deletes the files of session `sid` under `base`; `to` is the move folder. Returns the
/// line shown to the user.
fn run(base: &Path, sid: &str, m: Mode, to: &Path) -> String {
    let v = files(base, sid);
    if v.is_empty() {
        return "no session files to clear".into();
    }
    let short = &sid[..8];
    let mut bad = 0;
    let dest = match m {
        Mode::Off => return String::new(),
        Mode::Delete => None,
        Mode::Move => {
            let days = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64 / 86_400);
            Some(to.join(format!("{}_{sid}", ymd(days))))
        }
    };
    for r in &v {
        let ok = match &dest {
            Some(d) => mv(&base.join(r), &d.join(r)).is_ok(),
            None => rm(&base.join(r)).is_ok(),
        };
        if !ok {
            bad += 1;
        }
    }
    let n = v.len() - bad;
    let what = match &dest {
        Some(d) => format!("moved {n} items of {short} to {}", d.display()),
        None => format!("deleted {n} items of {short}"),
    };
    if bad > 0 {
        format!("{what}  ·  {bad} failed, left in place")
    } else {
        what
    }
}

/// Starts the work for the session `/clear` just left, if the option is on. The result line comes
/// back on the receiver.
pub fn start(sid: &str) -> Option<Receiver<String>> {
    let m = mode();
    if m == Mode::Off || !valid(sid) {
        return None;
    }
    let (tx, rx) = channel();
    let sid = sid.to_string();
    let to = PathBuf::from(folder());
    std::thread::spawn(move || {
        let _ = tx.send(run(&base(), &sid, m, &to));
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "0123abcd-0000-4000-8000-00000000beef";

    fn tree() -> PathBuf {
        let b =
            std::env::temp_dir().join(format!("noname-sweep-{}-{}", std::process::id(), rand()));
        let w = |p: &str| {
            let p = b.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        };
        w(&format!("projects/-home-a/{SID}.jsonl"));
        w(&format!("projects/-home-a/{SID}/tool-results/t.txt"));
        w("projects/-home-a/memory/MEMORY.md");
        w("projects/-home-a/other.jsonl");
        w(&format!("file-history/{SID}/f@v1"));
        w(&format!("session-env/{SID}/e"));
        w(&format!("plans/some-plan-of-{SID}.md"));
        w("plans/keep.md");
        b
    }

    fn rand() -> u64 {
        use std::hash::{BuildHasher, Hasher};
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    }

    #[test]
    fn finds_only_the_sessions_files() {
        let b = tree();
        let mut v: Vec<String> = files(&b, SID)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        v.sort();
        assert_eq!(v.len(), 5, "{v:?}");
        assert!(v.iter().all(|p| p.contains(SID)));
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn move_keeps_the_layout_and_leaves_the_rest() {
        let b = tree();
        let to = b.join("out");
        let s = run(&b, SID, Mode::Move, &to);
        assert!(s.starts_with("moved 5 items"), "{s}");
        let d = std::fs::read_dir(&to)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(d.join(format!("projects/-home-a/{SID}.jsonl")).exists());
        assert!(
            d.join(format!("projects/-home-a/{SID}/tool-results/t.txt"))
                .exists()
        );
        assert!(!b.join(format!("projects/-home-a/{SID}.jsonl")).exists());
        assert!(b.join("projects/-home-a/memory/MEMORY.md").exists());
        assert!(b.join("projects/-home-a/other.jsonl").exists());
        assert!(b.join("plans/keep.md").exists());
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn delete_removes_only_the_session() {
        let b = tree();
        let s = run(&b, SID, Mode::Delete, &b.join("unused"));
        assert!(s.starts_with("deleted 5 items"), "{s}");
        assert!(files(&b, SID).is_empty());
        assert!(b.join("projects/-home-a/memory/MEMORY.md").exists());
        assert!(!b.join("unused").exists());
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn odd_ids_are_refused() {
        assert!(!valid("../../etc"));
        assert!(!valid(""));
        assert!(!valid("abc"));
        assert!(valid(SID));
    }

    #[test]
    fn dates() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(20_416), "2025-11-24");
        assert_eq!(ymd(20_781), "2026-11-24");
    }

    #[test]
    fn modes_cycle() {
        assert_eq!(Mode::Off.next(), Mode::Move);
        assert_eq!(Mode::Move.next(), Mode::Delete);
        assert_eq!(Mode::Delete.next(), Mode::Off);
        assert_eq!(Mode::parse("junk"), Mode::Off);
    }
}
