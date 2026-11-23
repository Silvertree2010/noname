#![cfg_attr(windows, allow(dead_code))]

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub(crate) const SRV: &str = "noname";
pub(crate) const TOOL: &str = "code_agent";

pub(crate) enum Ev {
    Act(String),
    Done(String),
    Fail(String),
}

pub(crate) fn tools() -> Value {
    json!([{
        "name": TOOL,
        "description": "Run a task in a separate Claude Code session with Code Mode on. Code Mode lets that session write JavaScript that chains many tool calls (reads, greps, shell commands) in one step, so it suits tasks that need many tool calls: sweeping searches, bulk edits, checks across many files or hosts. The session starts with no context from this chat, so describe the task completely. It returns the session's final answer. Prefer normal tools or the Agent tool for small tasks.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The full task, self-contained."},
                "model": {"type": "string", "description": "Optional model alias such as haiku, sonnet, opus or fable."}
            },
            "required": ["task"]
        }
    }])
}

fn uuid() -> String {
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| io::Read::read_exact(&mut f, &mut b))
        .is_ok();
    if !ok {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ ((std::process::id() as u128) << 64);
        b = n.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

const STRIP: [&str; 13] = [
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
    "CLAUDE_CODE_REMOTE",
    "ANTHROPIC_UNIX_SOCKET",
];

fn short(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "~"
    } else {
        s
    }
}

fn act(name: &str, i: &Value) -> String {
    let a = [
        "command",
        "file_path",
        "pattern",
        "path",
        "url",
        "query",
        "description",
        "prompt",
    ]
    .iter()
    .find_map(|k| i[*k].as_str())
    .unwrap_or("");
    if a.is_empty() {
        name.to_string()
    } else {
        format!("{name}  {}", short(a, 160))
    }
}

pub(crate) struct Run {
    #[cfg(unix)]
    ch: std::process::Child,
    #[cfg(unix)]
    _m: std::os::fd::OwnedFd,
    #[cfg(unix)]
    ps: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    dir: PathBuf,
    sig: PathBuf,
    tr: PathBuf,
    pos: u64,
    part: Vec<u8>,
    pub(crate) tools: u64,
    txt: String,
    note: String,
    lp: Instant,
    sd: Option<Instant>,
    fin: bool,
    t0: Instant,
    seen: Instant,
    sh: u64,
    dlg: Option<(u8, Instant)>,
}

const LIM: Duration = Duration::from_secs(100 * 60);
const IDLE: Duration = Duration::from_secs(10 * 60);
const HOLD: Duration = Duration::from_secs(2);

fn dialog(sc: &str) -> Option<u8> {
    let v: Vec<&str> = sc
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let b = &v[v.len().saturating_sub(16)..];
    let menu = b.iter().any(|l| {
        l.strip_prefix('❯').map(str::trim_start).is_some_and(|r| {
            r.chars().next().is_some_and(|c| c.is_ascii_digit()) && r.contains(". ")
        })
    });
    if !menu {
        return None;
    }
    let has = |p: &str| b.iter().any(|l| l.contains(p));
    if has("turn on Code Mode") {
        Some(0)
    } else if has("trust this folder") || has("trust the files in this folder") {
        Some(1)
    } else if has("Do you want to") {
        Some(2)
    } else {
        None
    }
}

fn lines(part: &mut Vec<u8>) -> Vec<String> {
    let Some(cut) = part.iter().rposition(|&b| b == b'\n') else {
        return vec![];
    };
    let rest = part.split_off(cut + 1);
    let done = std::mem::replace(part, rest);
    done.split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect()
}

impl Run {
    #[cfg(windows)]
    pub(crate) fn start(_: &str, _: Option<&str>, _: &str, _: &str) -> io::Result<Run> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the code agent needs a unix pty",
        ))
    }

    #[cfg(unix)]
    pub(crate) fn start(task: &str, model: Option<&str>, mode: &str, cwd: &str) -> io::Result<Run> {
        use std::fs::File;
        use std::io::Read;
        use std::os::fd::{FromRawFd, OwnedFd};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};

        let id = uuid();
        let dir =
            std::env::temp_dir().join(format!("noname-cm-{}-{}", std::process::id(), &id[..8]));
        std::fs::create_dir_all(&dir)?;
        let sig = dir.join("stop");
        let hk = format!("touch '{}'", sig.to_string_lossy().replace('\'', "'\\''"));
        let st = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": hk}]}]}});
        let (rows, cols) = (50u16, 200u16);
        let (mut m, mut s) = (0, 0);
        let w = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe {
            libc::openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &w as *const _ as *mut _,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let m = unsafe { OwnedFd::from_raw_fd(m) };
        let s = unsafe { OwnedFd::from_raw_fd(s) };
        let mut c = Command::new(crate::sys::bin("claude"));
        for k in STRIP {
            c.env_remove(k);
        }
        crate::live::tokenv_pub(&mut c);
        c.arg("--session-id")
            .arg(&id)
            .arg("--settings")
            .arg(st.to_string())
            .arg("--permission-mode")
            .arg(mode);
        if let Some(md) = model.filter(|x| !x.trim().is_empty()) {
            c.arg("--model").arg(md.trim());
        }
        c.arg("--")
            .arg(task)
            .current_dir(cwd)
            .env("CLAUDE_CODE_EXPERIMENTAL_CODE_MODE", "1")
            .env("TERM", "xterm-256color")
            .env("COLUMNS", cols.to_string())
            .env("LINES", rows.to_string())
            .stdin(Stdio::from(s.try_clone()?))
            .stdout(Stdio::from(s.try_clone()?))
            .stderr(Stdio::from(s.try_clone()?));
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                #[cfg(target_os = "linux")]
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let ch = c.spawn()?;
        drop(s);
        crate::live::track_pid(ch.id() as i32);
        let ps = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let p2 = ps.clone();
        let mut f = File::from(m.try_clone()?);
        std::thread::spawn(move || {
            let mut b = [0u8; 16384];
            while let Ok(n) = f.read(&mut b) {
                if n == 0 {
                    break;
                }
                if let Ok(mut p) = p2.lock() {
                    p.process(&b[..n]);
                }
            }
        });
        let tr = PathBuf::from(format!("{}/{id}.jsonl", crate::live::pdir_pub(cwd)));
        Ok(Run {
            ch,
            _m: m,
            ps,
            dir,
            sig,
            tr,
            pos: 0,
            part: vec![],
            tools: 0,
            txt: String::new(),
            note: String::new(),
            lp: Instant::now(),
            sd: None,
            fin: false,
            t0: Instant::now(),
            seen: Instant::now(),
            sh: 0,
            dlg: None,
        })
    }

    fn read(&mut self, out: &mut Vec<Ev>) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.tr) else {
            return false;
        };
        let n = f.metadata().map(|m| m.len()).unwrap_or(0);
        if n < self.pos {
            self.pos = 0;
            self.part.clear();
        }
        if n == self.pos || f.seek(SeekFrom::Start(self.pos)).is_err() {
            return false;
        }
        let mut b = vec![];
        let Ok(k) = f.take(n - self.pos).read_to_end(&mut b) else {
            return false;
        };
        self.pos += k as u64;
        self.part.extend_from_slice(&b);
        for l in lines(&mut self.part) {
            let Ok(v) = serde_json::from_str::<Value>(&l) else {
                continue;
            };
            if v["type"] != "assistant" {
                continue;
            }
            let Some(cs) = v["message"]["content"].as_array() else {
                continue;
            };
            let mut t = String::new();
            for c in cs {
                match c["type"].as_str().unwrap_or("") {
                    "text" => t.push_str(c["text"].as_str().unwrap_or("")),
                    "tool_use" => {
                        self.tools += 1;
                        out.push(Ev::Act(act(
                            c["name"].as_str().unwrap_or("tool"),
                            &c["input"],
                        )));
                    }
                    _ => {}
                }
            }
            if !t.trim().is_empty() {
                self.txt = t.trim().to_string();
            }
        }
        k > 0
    }

    #[cfg(unix)]
    fn screen(&self) -> String {
        self.ps
            .lock()
            .map(|p| p.screen().contents())
            .unwrap_or_default()
    }

    #[cfg(unix)]
    fn tail(&self) -> String {
        let s = self.screen();
        let v: Vec<&str> = s
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .collect();
        v[v.len().saturating_sub(12)..].join("\n")
    }

    fn answer(&self) -> String {
        let t = if self.txt.is_empty() {
            "(the code agent finished without a text answer)".to_string()
        } else {
            self.txt.clone()
        };
        if self.note.is_empty() {
            t
        } else {
            format!("[{}]\n\n{t}", self.note)
        }
    }

    pub(crate) fn poll(&mut self) -> Vec<Ev> {
        let mut out = vec![];
        if self.fin || self.lp.elapsed().as_millis() < 250 {
            return out;
        }
        self.lp = Instant::now();
        let stop = self.sig.exists();
        if self.read(&mut out) {
            self.seen = Instant::now();
        }
        if stop {
            let sd = *self.sd.get_or_insert_with(Instant::now);
            let w = sd.elapsed().as_millis();
            if (!self.txt.is_empty() && w >= 250) || w > 1500 {
                self.fin = true;
                out.push(Ev::Done(self.answer()));
            }
            return out;
        }
        #[cfg(unix)]
        {
            let sc = self.screen();
            if self.note.is_empty() && sc.contains("Code Mode") && sc.contains("isn't available") {
                self.note = "Code Mode was not available, the agent ran with ordinary tools".into();
            }
            let h = {
                use std::hash::{Hash, Hasher};
                let mut x = std::collections::hash_map::DefaultHasher::new();
                sc.hash(&mut x);
                x.finish()
            };
            if h != self.sh {
                self.sh = h;
                self.seen = Instant::now();
            }
            let d = dialog(&sc);
            let held = match (d, self.dlg) {
                (Some(k), Some((j, t))) if k == j => t.elapsed() >= HOLD,
                (Some(k), _) => {
                    self.dlg = Some((k, Instant::now()));
                    false
                }
                (None, _) => {
                    self.dlg = None;
                    false
                }
            };
            let f = |m: String| Some(Ev::Fail(m));
            let bad = match d {
                Some(0) if held => f(
                    "Code Mode is not confirmed yet. Run `CLAUDE_CODE_EXPERIMENTAL_CODE_MODE=1 claude` once in a terminal and choose \"Yes, turn on Code Mode\".".into(),
                ),
                Some(1) if held => f(
                    "This folder is not trusted for interactive Claude Code yet. Run `claude` here once and trust the folder.".into(),
                ),
                Some(_) if held => f(format!(
                    "The code agent stopped at a permission prompt. Use auto or bypass permission mode for code agents.\n\n{}",
                    self.tail()
                )),
                _ if crate::live::exited(&mut self.ch) && !self.sig.exists() => {
                    f(format!("The code agent exited early.\n\n{}", self.tail()))
                }
                _ if self.seen.elapsed() >= IDLE => f(format!(
                    "The code agent made no progress for {} minutes and was stopped. It may be waiting at a prompt.\n\n{}",
                    IDLE.as_secs() / 60,
                    self.tail()
                )),
                _ if self.t0.elapsed() >= LIM => f(format!(
                    "The code agent ran longer than {} minutes and was stopped.\n\n{}",
                    LIM.as_secs() / 60,
                    self.tail()
                )),
                _ => None,
            };
            if let Some(e) = bad {
                self.fin = true;
                out.push(e);
            }
        }
        out
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        #[cfg(unix)]
        crate::live::reap_pub(&mut self.ch);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_need_the_menu() {
        let task =
            "> fix it. Do you want to check that trust this folder works?\n\n  Working… (3s)";
        assert_eq!(dialog(task), None);
        let p =
            "Bash command\n  rm -rf x\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel";
        assert_eq!(dialog(p), Some(2));
        let t = "Quick safety check\n❯ 1. Yes, I trust this folder\n  2. No, exit";
        assert_eq!(dialog(t), Some(1));
        let c = "Code Mode\n❯ 1. Yes, turn on Code Mode\n  2. No";
        assert_eq!(dialog(c), Some(0));
        let old = format!("Do you want to proceed?\n{}❯ 1. Yes", "line\n".repeat(30));
        assert_eq!(dialog(&old), None);
    }

    #[test]
    fn lines_keep_partial_utf8() {
        let s = "{\"a\":\"ü\"}\n{\"b\":\"€".as_bytes();
        let mut p = s[..s.len() - 1].to_vec();
        assert_eq!(lines(&mut p), vec!["{\"a\":\"ü\"}".to_string()]);
        p.extend_from_slice(&s[s.len() - 1..]);
        p.extend_from_slice(b"\"}\n");
        assert_eq!(lines(&mut p), vec!["{\"b\":\"€\"}".to_string()]);
        assert!(p.is_empty());
    }
}
