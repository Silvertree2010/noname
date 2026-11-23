//! The Codex engine (one `codex exec` per message) and the state it shares with the other
//! one-shot engines (`agy`, `opencode`).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use serde_json::{Value, json};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eng {
    Codex,
    Opencode,
    Agy,
}

pub(crate) struct Cx {
    pub(crate) tid: Option<String>,
    pub(crate) ch: Option<Child>,
    pub(crate) rx: Option<Receiver<Value>>,
    pub(crate) args: Vec<String>,
    pub(crate) im: HashMap<String, usize>,
    pub(crate) last: bool,
    pub(crate) t0: u64,
    pub(crate) eng: Eng,
    pub(crate) tok: u64,
    /// stderr of the current run (agy only) and whether its result event came
    pub(crate) err: String,
    pub(crate) fin: bool,
}

impl Drop for Cx {
    fn drop(&mut self) {
        if let Some(c) = &mut self.ch {
            live::reap_pub(c);
        }
    }
}

pub(crate) fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub(crate) fn one(s: &str, n: usize) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    if l.chars().count() > n {
        l.chars().take(n).collect::<String>() + "…"
    } else {
        l.to_string()
    }
}

fn unwrap(c: &str) -> String {
    for p in [
        "bash -lc ",
        "/bin/bash -lc ",
        "bash -c ",
        "sh -c ",
        "zsh -lc ",
        "powershell.exe -Command ",
        "powershell -Command ",
        "pwsh -Command ",
        "pwsh.exe -Command ",
        "cmd /c ",
        "cmd.exe /c ",
    ] {
        if let Some(r) = c.strip_prefix(p) {
            let r = r.trim();
            if r.len() >= 2
                && ((r.starts_with('"') && r.ends_with('"'))
                    || (r.starts_with('\'') && r.ends_with('\'')))
            {
                return r[1..r.len() - 1].replace("\\\"", "\"");
            }
            return r.to_string();
        }
    }
    c.to_string()
}

fn cfg_model() -> String {
    let h =
        std::env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", crate::sys::home()));
    std::fs::read_to_string(format!("{h}/config.toml"))
        .ok()
        .and_then(|f| {
            f.lines()
                .find(|l| {
                    l.trim_start().starts_with("model ") || l.trim_start().starts_with("model=")
                })
                .map(|l| {
                    l.split('=')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .to_string()
                })
        })
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| "codex".into())
}

pub(crate) fn spawn_err(nm: &str, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        format!("{nm} not found")
    } else {
        format!("{nm} failed to start: {e}")
    }
}

pub(crate) fn build(
    c: &Cx,
    mode: usize,
    imgs: &[(&'static str, Vec<u8>)],
    t: u64,
    x: &str,
) -> (Command, Option<String>) {
    let mut cmd = Command::new(crate::sys::bin("codex"));
    cmd.arg("exec");
    if c.tid.is_some() || c.last {
        cmd.arg("resume");
    }
    cmd.args(["--json", "--skip-git-repo-check"]);
    match mode {
        1 => {
            cmd.args(["-s", "read-only"]);
        }
        3 => {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
        }
        _ => {
            cmd.args(["-s", "workspace-write"]);
        }
    }
    cmd.args(&c.args);
    for (k, (m, b)) in imgs.iter().enumerate() {
        let n = format!(
            "noname-{}-{t}-{k}.{}",
            std::process::id(),
            crate::img::ext(m)
        );
        if let Some(f) = crate::sys::tmp_write(&n, b) {
            cmd.arg("-i").arg(f);
        }
    }
    if c.tid.is_none() && c.last {
        cmd.arg("--last");
    }
    cmd.arg("--");
    if let Some(id) = &c.tid {
        cmd.arg(id);
    }
    cmd.arg("-");
    (cmd, Some(x.to_string()))
}

impl App {
    /// Builds an `App` that uses Codex as the engine (one `codex exec` per message). `args` holds the command-line flags (`-c`, `-r`, `-m`) and the arguments passed through to `codex`.
    ///
    /// # Errors
    ///
    /// Returns an error when the current folder cannot be read.
    pub fn codex(args: Vec<String>) -> io::Result<App> {
        let mut a = App::new();
        a.bs.clear();
        a.todo.clear();
        a.ctx = -1.0;
        a.h5 = -1.0;
        a.d7 = -1.0;
        a.cwd = {
            let c = std::env::current_dir()?.to_string_lossy().to_string();
            let h = crate::sys::home();
            if !h.is_empty() && c.starts_with(&h) {
                format!("~{}", &c[h.len()..])
            } else {
                c
            }
        };
        a.br = Command::new("git")
            .args(["branch", "--show-current"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let mut rest = vec![];
        let mut last = false;
        let mut tid = None;
        let mut it = args.into_iter();
        while let Some(x) = it.next() {
            match x.as_str() {
                "-c" | "--continue" => last = true,
                "-r" | "--resume" => tid = it.next(),
                "--model" | "-m" => {
                    if let Some(m) = it.next() {
                        rest.extend(["-m".to_string(), m]);
                    }
                }
                _ => rest.push(x),
            }
        }
        a.mname = rest
            .iter()
            .position(|x| x == "-m")
            .and_then(|i| rest.get(i + 1))
            .cloned()
            .unwrap_or_else(cfg_model);
        a.effs = String::new();
        a.bgw = std::fs::read_to_string(crate::bgfile())
            .ok()
            .and_then(|x| crate::bg::canon(&x))
            .filter(|k| crate::bg::exists(k));
        a.mode = crate::defmode().unwrap_or(0);
        a.cx = Some(Cx {
            tid,
            ch: None,
            rx: None,
            args: rest,
            im: HashMap::new(),
            last,
            t0: 0,
            eng: Eng::Codex,
            tok: 0,
            err: String::new(),
            fin: false,
        });
        Ok(a)
    }

    /// Like `App::codex`, but the engine is the `opencode` CLI. The model name comes from the OpenCode config unless `-m` is given.
    ///
    /// # Errors
    ///
    /// Returns an error when the current folder cannot be read.
    pub fn opencode(args: Vec<String>) -> io::Result<App> {
        let mut a = App::codex(args)?;
        if let Some(c) = &mut a.cx {
            c.eng = Eng::Opencode;
        }
        crate::opencode::load_cmds();
        if !a
            .cx
            .as_ref()
            .is_some_and(|c| c.args.iter().any(|x| x == "-m"))
        {
            a.mname = crate::opencode::cfg_model();
        }
        Ok(a)
    }

    /// Antigravity CLI (`agy`) as the engine: one `agy -p` per message.
    ///
    /// # Errors
    ///
    /// Returns an error when the current folder cannot be read.
    pub fn agy(args: Vec<String>) -> io::Result<App> {
        let mut a = App::codex(args)?;
        if let Some(c) = &mut a.cx {
            c.eng = Eng::Agy;
        }
        if !a
            .cx
            .as_ref()
            .is_some_and(|c| c.args.iter().any(|x| x == "-m"))
        {
            a.mname = crate::agy::cfg_model();
        }
        Ok(a)
    }

    fn kind(&self) -> Eng {
        self.cx.as_ref().map_or(Eng::Codex, |c| c.eng)
    }

    /// Whether this session uses the OpenCode engine.
    pub fn is_opencode(&self) -> bool {
        self.kind() == Eng::Opencode
    }

    /// Whether this session uses the `agy` engine.
    pub fn is_agy(&self) -> bool {
        self.kind() == Eng::Agy
    }

    pub(crate) fn eng(&self) -> &'static str {
        match self.kind() {
            Eng::Opencode => "opencode",
            Eng::Agy => "agy",
            Eng::Codex => "codex",
        }
    }

    /// Whether one of the one-shot engines (Codex, OpenCode or `agy`) is in use instead of the default Claude engine.
    pub fn is_codex(&self) -> bool {
        self.cx.is_some()
    }

    pub(crate) fn csubmit(&mut self, s: String) {
        if self.busy {
            self.toast(format!("{} is busy", self.eng()));
            return;
        }
        self.t0 = self.t;
        self.bs.push(B::U(s.clone(), self.clock()));
        self.busy = true;
        let mode = self.mode;
        let t = self.t;
        let imgs: Vec<(&'static str, Vec<u8>)> = self
            .content(&s)
            .unwrap_or_default()
            .into_iter()
            .map(|(m, b)| (m, b.to_vec()))
            .collect();
        let x = self.expand(&s);
        let nm = self.eng();
        let Some(c) = &mut self.cx else { return };
        c.tok = 0;
        c.err.clear();
        c.fin = false;
        let (mut cmd, inp) = if c.eng == Eng::Agy {
            crate::agy::build(c, mode, &imgs, t, &x)
        } else if c.eng == Eng::Opencode {
            crate::opencode::build(c, mode, &imgs, t, &x)
        } else {
            build(c, mode, &imgs, t, &x)
        };
        let agy = c.eng == Eng::Agy;
        cmd.stdin(if inp.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(if agy { Stdio::piped() } else { Stdio::null() });
        live::guard_pub(&mut cmd);
        let mut ch = match cmd.spawn() {
            Ok(ch) => ch,
            Err(e) => {
                self.busy = false;
                self.bs.push(B::Sys(spawn_err(nm, &e)));
                return;
            }
        };
        live::track_child(&ch);
        if let (Some(mut si), Some(p)) = (ch.stdin.take(), inp) {
            thread::spawn(move || {
                use std::io::Write;
                let _ = si.write_all(p.as_bytes());
            });
        }
        let out = ch.stdout.take().expect("stdout is piped above");
        let (tx, rx) = channel();
        // agy reports sign-in problems and denied permissions on stderr only
        let eh = ch.stderr.take().map(|er| {
            let tx = tx.clone();
            thread::spawn(move || {
                for l in BufReader::new(er).lines() {
                    let Ok(l) = l else { break };
                    if tx.send(json!({"event": "_stderr", "text": l})).is_err() {
                        break;
                    }
                }
            })
        });
        // NONAME_ENGINE_LOG=FILE keeps every raw event line, for bug reports about an engine
        let log = std::env::var("NONAME_ENGINE_LOG")
            .ok()
            .filter(|x| !x.is_empty());
        thread::spawn(move || {
            let mut lf = log.and_then(|p| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .ok()
            });
            for l in BufReader::new(out).lines() {
                let Ok(l) = l else { break };
                if let Some(f) = &mut lf {
                    use std::io::Write;
                    let _ = writeln!(f, "{l}");
                }
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
            // the last stderr lines must arrive before the end marker
            if let Some(h) = eh {
                let _ = h.join();
            }
            let _ = tx.send(json!({"type": "_eof"}));
        });
        c.ch = Some(ch);
        c.rx = Some(rx);
        c.last = false;
        c.t0 = t;
        c.im.clear();
    }

    fn ckill(&mut self) {
        if let Some(c) = &mut self.cx {
            if let Some(mut ch) = c.ch.take() {
                live::untrack_pid(ch.id() as i32);
                live::reap_pub(&mut ch);
            }
            c.rx = None;
        }
    }

    pub(crate) fn cstop(&mut self) {
        self.ckill();
        for b in self.bs.iter_mut() {
            if let B::Tl(t) = b {
                if t.st == St::Run {
                    t.st = St::Err;
                    t.t1 = self.t;
                    t.sum = "interrupted".into();
                }
            }
        }
        self.busy = false;
        self.sb = None;
        self.bs.push(B::Sys("interrupted".into()));
    }

    pub(crate) fn ccmd(&mut self, r: &str) {
        let (c, arg) = r.split_once(' ').unwrap_or((r, ""));
        match c {
            "clear" | "new" => {
                self.ckill();
                if let Some(x) = &mut self.cx {
                    x.tid = None;
                    x.last = false;
                    x.im.clear();
                }
                self.busy = false;
                self.sb = None;
                self.bs.clear();
                self.todo.clear();
                self.tc = self.t;
            }
            "resume" if !arg.trim().is_empty() => {
                if let Some(x) = &mut self.cx {
                    x.tid = Some(arg.trim().to_string());
                }
                self.toast("next message resumes that session");
            }
            "quit" | "exit" => self.quit = true,
            // agy answers its own TUI commands itself, which a one-shot print run cannot do
            _ if self.is_agy() && matches!(c, "model" | "effort" | "usage" | "quota" | "config" | "settings" | "logout" | "help" | "mcp" | "permissions" | "plan" | "agents" | "skills" | "hooks") => {
                self.toast(format!("/{c} is an agy menu command: start noname with --model, --effort or --agent instead"))
            }
            // opencode and agy know their own commands and skills (/researcher, /deep-research, ...): let them run them
            _ if (self.is_opencode() || self.is_agy()) && crate::opencode::split_cmd(&format!("/{r}")).is_some() => self.csubmit(format!("/{r}")),
            _ => self.toast(format!("/{c} is not available with {}", self.eng())),
        }
    }

    pub(crate) fn cpump(&mut self) {
        let evs: Vec<Value> = match self.cx.as_ref().and_then(|c| c.rx.as_ref()) {
            Some(r) => r.try_iter().collect(),
            None => return,
        };
        let k = self.kind();
        for e in evs {
            match k {
                Eng::Opencode => self.ocev(e),
                Eng::Agy => self.agev(e),
                Eng::Codex => self.cev(e),
            }
        }
    }

    pub(crate) fn cev(&mut self, e: Value) {
        match e["type"].as_str().unwrap_or("") {
            "thread.started" => {
                let id = sv(&e, "thread_id");
                if let Some(c) = &mut self.cx {
                    if c.tid.is_none() && !id.is_empty() {
                        c.tid = Some(id);
                    }
                }
            }
            "item.started" | "item.updated" | "item.completed" => {
                let done = e["type"] == "item.completed";
                self.citem(&e["item"], done);
            }
            "turn.completed" => {
                let u = &e["usage"];
                let tok = ["input_tokens", "cached_input_tokens", "output_tokens"]
                    .iter()
                    .map(|k| u[*k].as_u64().unwrap_or(0))
                    .sum::<u64>();
                let t0 = self.cx.as_ref().map(|c| c.t0).unwrap_or(self.t);
                let secs = (self.t - t0) as f32 * 0.033;
                let tk = if tok >= 1000 {
                    format!("{:.1}k", tok as f64 / 1000.0)
                } else {
                    tok.to_string()
                };
                self.bs
                    .push(B::Rc(format!("{secs:.1}s  ·  {tk} tok"), self.t.max(1)));
            }
            "turn.failed" | "error" => {
                let m = e["error"]["message"]
                    .as_str()
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| sv(&e, "message"));
                if !m.is_empty() {
                    self.bs.push(B::T(format!("**codex:** {}", one(&m, 200))));
                }
            }
            "_eof" => {
                self.busy = false;
                self.sb = None;
                for b in self.bs.iter_mut() {
                    if let B::Tl(t) = b {
                        if t.st == St::Run {
                            t.st = St::Ok;
                            t.t1 = self.t;
                        }
                    }
                }
                if let Some(c) = &mut self.cx {
                    if let Some(mut ch) = c.ch.take() {
                        live::untrack_pid(ch.id() as i32);
                        let _ = ch.wait();
                    }
                    c.rx = None;
                }
            }
            _ => {}
        }
    }

    fn citem(&mut self, it: &Value, done: bool) {
        let id = sv(it, "id");
        let ty = sv(it, "type");
        let ex = self.cx.as_ref().and_then(|c| c.im.get(&id).copied());
        let now = self.t.max(1);
        match ty.as_str() {
            "agent_message" | "reasoning" => {
                let txt = sv(it, "text");
                let b = if ty == "reasoning" {
                    B::Th(txt.replace("**", ""))
                } else {
                    B::T(txt)
                };
                match ex.and_then(|i| self.bs.get_mut(i)) {
                    Some(s) => *s = b,
                    None => {
                        self.bs.push(b);
                        let i = self.bs.len() - 1;
                        if let Some(c) = &mut self.cx {
                            c.im.insert(id, i);
                        }
                    }
                }
            }
            "todo_list" => {
                if let Some(a) = it["items"].as_array() {
                    self.todo = a
                        .iter()
                        .map(|x| (if x["completed"] == true { 2 } else { 0 }, sv(x, "text")))
                        .collect();
                }
            }
            "command_execution" | "file_change" | "mcp_tool_call" | "web_search"
            | "collab_tool_call" => {
                let (k, nm, a) = match ty.as_str() {
                    "command_execution" => (
                        Tk::Bash,
                        "bash".to_string(),
                        one(&unwrap(&sv(it, "command")), 140),
                    ),
                    "file_change" => {
                        let ch = it["changes"].as_array().cloned().unwrap_or_default();
                        let p: Vec<String> = ch.iter().map(|c| sv(c, "path")).collect();
                        let p = p
                            .iter()
                            .map(|x| x.rsplit('/').next().unwrap_or(x).to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        (Tk::Edit, "edit".to_string(), p)
                    }
                    "mcp_tool_call" => (Tk::Oth, sv(it, "server"), sv(it, "tool")),
                    "web_search" => (Tk::Web, "web".to_string(), sv(it, "query")),
                    _ => (Tk::Task, "agent".to_string(), one(&sv(it, "prompt"), 120)),
                };
                let st = match sv(it, "status").as_str() {
                    "failed" | "declined" => St::Err,
                    _ if done => St::Ok,
                    _ => St::Run,
                };
                let out = sv(it, "aggregated_output");
                let ls: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
                let sum = match (ty.as_str(), st) {
                    ("command_execution", St::Err) => {
                        format!("exit {}", it["exit_code"].as_i64().unwrap_or(1))
                    }
                    ("command_execution", St::Ok) => {
                        ls.last().map(|l| one(l, 48)).unwrap_or_else(|| "ok".into())
                    }
                    ("file_change", _) => {
                        let ch = it["changes"].as_array().map(|x| x.len()).unwrap_or(0);
                        format!("{ch} files")
                    }
                    (_, St::Err) => "failed".into(),
                    (_, St::Ok) => "done".into(),
                    _ => String::new(),
                };
                let body: Vec<Ln> = ls
                    .iter()
                    .rev()
                    .take(12)
                    .rev()
                    .map(|l| Ln::P(one(l, 200)))
                    .collect();
                match ex {
                    Some(i) => {
                        if let Some(B::Tl(t)) = self.bs.get_mut(i) {
                            t.st = st;
                            t.sum = sum;
                            t.body = body;
                            if st != St::Run {
                                t.t1 = now;
                            }
                        }
                    }
                    None => {
                        self.bs.push(B::Tl(Tool {
                            k,
                            nm,
                            a,
                            sum,
                            body,
                            st,
                            t0: now,
                            t1: if st == St::Run { 0 } else { now },
                        }));
                        let i = self.bs.len() - 1;
                        self.sel = Some(i);
                        if let Some(c) = &mut self.cx {
                            c.im.insert(id, i);
                        }
                    }
                }
            }
            "error" => self.bs.push(B::Sys(one(&sv(it, "message"), 200))),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Cx {
        Cx {
            tid: None,
            ch: None,
            rx: None,
            args: vec![],
            im: HashMap::new(),
            last: false,
            t0: 0,
            eng: Eng::Codex,
            tok: 0,
            err: String::new(),
            fin: false,
        }
    }

    fn args(c: &Cx, x: &str) -> (Vec<String>, Option<String>) {
        let (cmd, i) = build(c, 0, &[], 0, x);
        let v = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        (v, i)
    }

    #[test]
    fn the_prompt_goes_through_stdin() {
        let mut c = cx();
        let (v, i) = args(&c, "-x\nline two");
        assert_eq!(v[..2], ["exec", "--json"]);
        assert_eq!(v[v.len() - 2..], ["--", "-"]);
        assert_eq!(i.as_deref(), Some("-x\nline two"));
        c.last = true;
        let (v, _) = args(&c, "x");
        assert_eq!(v[1], "resume");
        assert_eq!(v[v.len() - 3..], ["--last", "--", "-"]);
        c.tid = Some("abc".into());
        let (v, _) = args(&c, "x");
        assert!(!v.contains(&"--last".to_string()));
        assert_eq!(v[v.len() - 3..], ["--", "abc", "-"]);
    }

    #[test]
    fn spawn_errors_say_what_happened() {
        let e = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(spawn_err("codex", &e), "codex not found");
        let e = io::Error::other("batch file arguments are invalid");
        assert!(spawn_err("codex", &e).contains("batch file arguments are invalid"));
    }

    #[test]
    fn clear_forgets_item_indices() {
        let msg = |id: &str, t: &str| json!({"id": id, "type": "agent_message", "text": t});
        let mut a = App::codex(vec![]).unwrap();
        a.cev(json!({"type": "item.started", "item": msg("i1", "a")}));
        a.cev(json!({"type": "item.started", "item": msg("i2", "b")}));
        a.ccmd("clear");
        assert!(a.bs.is_empty() && a.cx.as_ref().unwrap().im.is_empty());
        a.cev(json!({"type": "item.updated", "item": msg("i2", "c")}));
        assert_eq!(a.bs.len(), 1);
        a.cx.as_mut().unwrap().im.insert("i9".into(), 99);
        a.cev(json!({"type": "item.updated", "item": msg("i9", "d")}));
        a.cx.as_mut().unwrap().im.insert("o9".into(), 99);
        a.ocev(json!({"type": "text", "part": {"id": "o9", "text": "e"}}));
    }
}
