//! `/phone`: text spoken on the phone lands in this window's input.
//!
//! The phone sends a recording to this PC, the `slashphone` service turns it into text and writes it as
//! `<cache>/slashphone/text/<name>.txt`. The window where `/phone` was typed writes its process id to
//! `<cache>/slashphone/target` and picks those files up; only one window receives at a time.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the receiving window looks for new text.
const EVERY: Duration = Duration::from_millis(400);

/// The receiving side of `/phone` in this window.
pub(crate) struct Ph {
    /// When the text folder was last checked.
    at: Instant,
}

/// The shared folder: `slashphone` under `XDG_CACHE_HOME` or `~/.cache`.
pub(crate) fn dir() -> PathBuf {
    let b = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", crate::sys::home()));
    PathBuf::from(b).join("slashphone")
}

fn me() -> String {
    std::process::id().to_string()
}

/// Makes this window the receiver and starts the PC service (if it isn't running yet).
pub(crate) fn arm() -> Result<Ph, String> {
    let d = dir();
    for s in ["text", "done", "audio"] {
        std::fs::create_dir_all(d.join(s)).map_err(|e| format!("{}: {e}", d.join(s).display()))?;
    }
    std::fs::write(d.join("target"), me()).map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "start", "slashphone.service"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut c| std::thread::spawn(move || c.wait()));
    Ok(Ph {
        at: Instant::now() - EVERY,
    })
}

/// Stops receiving; the target file is removed only when it still names this window.
pub(crate) fn disarm() {
    let t = dir().join("target");
    if std::fs::read_to_string(&t).is_ok_and(|s| s.trim() == me()) {
        let _ = std::fs::remove_file(t);
    }
}

/// What a check found.
pub(crate) enum Got {
    /// Nothing new.
    None,
    /// Another window took over with `/phone`.
    Lost,
    /// A transcript for the input.
    Text(String),
}

impl Ph {
    /// Takes the oldest waiting transcript (moving it to `done/`), at most once every [`EVERY`].
    pub(crate) fn poll(&mut self) -> Got {
        if self.at.elapsed() < EVERY {
            return Got::None;
        }
        self.at = Instant::now();
        let d = dir();
        if !std::fs::read_to_string(d.join("target")).is_ok_and(|s| s.trim() == me()) {
            return Got::Lost;
        }
        take(&d)
    }
}

/// Oldest `.txt` in `text/`, claimed by renaming it into `done/` so no other window gets it too.
fn take(d: &std::path::Path) -> Got {
    let Ok(rd) = std::fs::read_dir(d.join("text")) else {
        return Got::None;
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    v.sort();
    for p in v {
        let to = d.join("done").join(p.file_name().unwrap_or_default());
        if std::fs::rename(&p, &to).is_err() {
            continue;
        }
        let t = std::fs::read_to_string(&to).unwrap_or_default();
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            return Got::Text(t);
        }
    }
    Got::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_oldest_and_moves_it() {
        let d = std::env::temp_dir().join(format!("nn-phone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("text")).unwrap();
        std::fs::create_dir_all(d.join("done")).unwrap();
        std::fs::write(d.join("text/2.txt"), "second").unwrap();
        std::fs::write(d.join("text/1.txt"), " hello\n  there ").unwrap();
        std::fs::write(d.join("text/0.part"), "not yet").unwrap();
        assert!(matches!(take(&d), Got::Text(t) if t == "hello there"));
        assert!(d.join("done/1.txt").exists());
        assert!(matches!(take(&d), Got::Text(t) if t == "second"));
        assert!(matches!(take(&d), Got::None));
        let _ = std::fs::remove_dir_all(&d);
    }
}
