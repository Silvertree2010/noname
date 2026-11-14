use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use serde_json::{Value, json};

use super::*;

pub struct Lv {
    inp: ChildStdin,
    rx: Receiver<Value>,
    ch: Child,
    rid: u64,
    tm: HashMap<String, usize>,
    sm: HashMap<String, String>,
    bm: HashMap<u64, usize>,
    areq: Option<(String, Value, Value)>,
    sid: String,
    args: Vec<String>,
    brx: Option<Receiver<Value>>,
    bch: Option<Child>,
    tb: usize,
    uid: HashMap<usize, String>,
    rwp: Option<(usize, bool)>,
    c0: f64,
    w0: f32,
    pub cmds: Vec<(String, String)>,
}

static KIDS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
static TOK: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn tsvc() -> String {
    std::env::var("NONAME_WORK_TOKEN").unwrap_or_else(|_| "ApolloClaw-konto-firma".into())
}

pub fn work_token() -> Option<String> {
    let s = tsvc();
    if cfg!(windows) {
        return None;
    }
    let o = if cfg!(target_os = "macos") {
        Command::new("security").args(["find-generic-password", "-s", &s, "-w"]).stderr(Stdio::null()).output().ok()?
    } else {
        Command::new("secret-tool").args(["lookup", "service", &s]).stderr(Stdio::null()).output().ok()?
    };
    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if o.status.success() && !t.is_empty() { Some(t) } else { None }
}

fn tokenv(c: &mut Command) {
    c.env_remove("CLAUDE_CONFIG_DIR");
    match TOK.lock().ok().and_then(|t| t.clone()) {
        Some(t) => {
            c.env("CLAUDE_CODE_OAUTH_TOKEN", t);
        }
        None => {
            c.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        }
    }
}

pub fn killall() {
    let ks = KIDS.lock().map(|k| k.clone()).unwrap_or_default();
    if cfg!(windows) {
        for k in &ks {
            crate::sys::kill(*k);
        }
        return;
    }
    for k in &ks {
        crate::sys::term(*k);
    }
    std::thread::sleep(std::time::Duration::from_millis(400));
    for k in &ks {
        crate::sys::kill(*k);
    }
}

pub(crate) fn track_pid(id: i32) {
    if let Ok(mut k) = KIDS.lock() {
        k.push(id);
    }
}

pub(crate) fn untrack_pid(id: i32) {
    if let Ok(mut k) = KIDS.lock() {
        k.retain(|x| *x != id);
    }
}

pub(crate) fn track_child(c: &Child) {
    track(c)
}

fn track(c: &Child) {
    crate::sys::adopt(c);
    if let Ok(mut k) = KIDS.lock() {
        k.push(c.id() as i32);
    }
}

pub(crate) fn reap_pub(c: &mut Child) {
    reap(c)
}

pub(crate) fn guard_pub(c: &mut Command) {
    guard(c);
}

fn reap(c: &mut Child) {
    if let Ok(mut k) = KIDS.lock() {
        k.retain(|x| *x != c.id() as i32);
    }
    crate::sys::term(c.id() as i32);
    for _ in 0..20 {
        if matches!(c.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    crate::sys::kill(c.id() as i32);
    let _ = c.wait();
}

fn guard(c: &mut Command) -> &mut Command {
    crate::sys::guard(c)
}

impl Drop for Lv {
    fn drop(&mut self) {
        reap(&mut self.ch);
        if let Some(c) = &mut self.bch {
            reap(c);
        }
    }
}

const PM: [&str; 4] = ["default", "plan", "auto", "bypassPermissions"];

fn rd(r: impl io::Read + Send + 'static) -> Receiver<Value> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        for l in BufReader::new(r).lines() {
            let Ok(l) = l else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
        let _ = tx.send(json!({"type": "_eof"}));
    });
    rx
}

fn spawn(args: &[String]) -> io::Result<(Child, ChildStdin, Receiver<Value>)> {
    let mut cm = Command::new(crate::sys::bin("claude"));
    tokenv(&mut cm);
    let mut c = guard(&mut cm)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--allow-dangerously-skip-permissions",
            "--replay-user-messages",
            "--prompt-suggestions",
            "true",
            "--include-hook-events",
        ])
        .args(args)
        .env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "1")
        .env_remove("APOLLOCLAW_SWITCH_DATEI")
        .env("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    track(&c);
    let i = c.stdin.take().unwrap();
    let rx = rd(c.stdout.take().unwrap());
    Ok((c, i, rx))
}

fn mnm(id: &str) -> String {
    let s = id.trim_start_matches("claude-");
    let s = s.split('[').next().unwrap_or(s);
    let ps: Vec<&str> = s.split('-').filter(|x| x.len() < 8).collect();
    let nm: Vec<&str> = ps.iter().copied().filter(|x| !x.chars().all(|c| c.is_ascii_digit())).collect();
    let vr: Vec<&str> = ps.iter().copied().filter(|x| x.chars().all(|c| c.is_ascii_digit())).collect();
    if vr.is_empty() { nm.join(" ") } else { format!("{} {}", nm.join(" "), vr.join(".")) }
}

fn rel(p: &str) -> String {
    let h = crate::sys::home();
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    if let Some(r) = p.strip_prefix(&format!("{cwd}{}", std::path::MAIN_SEPARATOR)) {
        return r.to_string();
    }
    if !h.is_empty() {
        if let Some(r) = p.strip_prefix(&h) {
            return format!("~{r}");
        }
    }
    p.to_string()
}

fn one(s: &str, n: usize) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    if l.chars().count() > n { l.chars().take(n).collect::<String>() + "…" } else { l.to_string() }
}

fn strip(s: &str) -> String {
    let mut o = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&n) = it.peek() {
                    it.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            } else {
                it.next();
            }
        } else if c == '\r' {
            if it.peek() != Some(&'\n') {
                if let Some(p) = o.rfind('\n') {
                    o.truncate(p + 1);
                } else {
                    o.clear();
                }
            }
        } else if c == '\t' {
            o.push_str("    ");
        } else if !c.is_control() || c == '\n' {
            o.push(c);
        }
    }
    o
}

impl App {
    /// The Claude session id, empty until claude reports it.
    pub(crate) fn session_id(&self) -> String {
        self.lv.as_ref().map(|l| l.sid.clone()).unwrap_or_default()
    }

    fn subtxt(&mut self, p: &str, s: &str) {
        let Some(i) = self.lv.as_ref().and_then(|l| l.tm.get(p).copied()) else { return };
        if let Some(B::Tl(t)) = self.bs.get_mut(i) {
            for l in s.lines().filter(|l| !l.trim().is_empty()) {
                t.body.push(Ln::C(0, one(l, 200)));
            }
            if t.body.len() > 600 {
                t.body.drain(..t.body.len() - 600);
            }
        }
    }

    fn subact(&mut self, p: &str, line: String) {
        if let Some(i) = self.lv.as_ref().and_then(|l| l.tm.get(p).copied()) {
            if let Some(B::Tl(t)) = self.bs.get_mut(i) {
                t.body.push(Ln::P(one(&line, 200)));
                if t.body.len() > 600 {
                    t.body.remove(0);
                }
            }
        }
        if let Some(t) = self.tasks.iter_mut().find(|t| t.tool == p) {
            t.acts.push(one(&line, 200));
            if t.acts.len() > 50 {
                t.acts.remove(0);
            }
            t.last = one(&line, 200);
        }
    }

    pub(crate) fn fsuggest(&mut self, q: &str) -> bool {
        if self.lv.is_none() {
            return false;
        }
        self.ctrl("fsg", json!({"subtype": "file_suggestions", "query": q}));
        true
    }

    /// Value of the model that `/model <q>` names: exact value, exact display name, or a unique-enough prefix/word such as "opus".
    fn pickmodel(&self, q: &str) -> Option<String> {
        let nm = |s: &str| s.to_lowercase().replace(['-', '.'], " ").trim_start_matches("claude ").to_string();
        let q = nm(q);
        if q.is_empty() {
            return None;
        }
        let by = |f: &dyn Fn(&(String, String, String, Vec<String>)) -> bool| self.models.iter().find(|m| f(m)).map(|m| m.0.clone());
        by(&|m| nm(&m.0) == q)
            .or_else(|| by(&|m| nm(&m.1) == q))
            .or_else(|| by(&|m| nm(&m.1).split(' ').next() == Some(q.as_str())))
            .or_else(|| by(&|m| nm(&m.0).starts_with(&q) || nm(&m.1).starts_with(&q)))
    }

    pub(crate) fn setmodel(&mut self, v: &str) {
        let d = self.models.iter().find(|m| m.0 == v).map(|m| m.1.clone()).unwrap_or_else(|| mnm(v));
        if let Some(i) = MID_ALL.iter().position(|x| v.starts_with(x.0)) {
            self.model = i;
        }
        self.mname = d.to_lowercase().replace(' ', "-");
        if self.lv.is_some() {
            self.ctrl("mdl", json!({"subtype": "set_model", "model": v}));
        }
        self.toast(format!("model  {d}"));
    }

    /// Ask claude which model and effort are really in force; the reply (`get-` id) corrects what is shown.
    /// Asked only once a switch has been answered (`mdl-`/`eff-` reply): claude finishes those out of order, so an earlier ask can return the old values.
    pub(crate) fn syncset(&mut self) {
        self.ctrl("get", json!({"subtype": "get_settings"}));
    }

    pub(crate) fn seteffort(&mut self, v: &str) {
        self.effs = v.to_string();
        if self.lv.is_some() {
            self.ctrl("eff", json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": v}}));
        }
        self.toast(format!("effort  {v}"));
    }

    pub(crate) fn mcpact(&mut self, name: &str, toggle: bool) {
        if self.lv.is_none() {
            return;
        }
        let st = self.pk.as_ref().and_then(|p| p.items.iter().find(|x| x.2 == name)).map(|x| x.1.clone()).unwrap_or_default();
        if toggle {
            let en = st.starts_with("disabled");
            self.ctrl("mct", json!({"subtype": "mcp_toggle", "serverName": name, "enabled": en}));
            self.toast(format!("{}  {name}", if en { "enabling" } else { "disabling" }));
        } else {
            self.ctrl("mcr", json!({"subtype": "mcp_reconnect", "serverName": name}));
            self.toast(format!("reconnecting  {name}"));
        }
    }

    pub(crate) fn tpoll(&mut self, i: usize) {
        let Some(t) = self.tasks.get(i) else { return };
        if t.kind > 1 || self.lv.is_none() {
            return;
        }
        let id = t.id.clone();
        self.ctrl(&format!("tko{i}"), json!({"subtype": "get_task_output", "task_id": id}));
    }

    pub(crate) fn tstop(&mut self, i: usize) {
        let Some(t) = self.tasks.get(i) else { return };
        let (id, d) = (t.id.clone(), t.desc.clone());
        if self.lv.is_none() {
            if let Some(t) = self.tasks.get_mut(i) {
                t.st = 3;
                t.t1 = self.t.max(1);
            }
            return;
        }
        self.ctrl("stp", json!({"subtype": "stop_task", "task_id": id}));
        self.toast(format!("stopping  {}", one(&d, 30)));
    }

    pub(crate) fn tbg(&mut self) {
        if self.lv.is_some() && self.busy {
            self.ctrl("bgt", json!({"subtype": "background_tasks"}));
            self.toast("moved to background");
        }
    }
}

fn sv(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn mk(name: &str, i: &Value) -> Tool {
    let (k, nm) = match name {
        "Read" | "NotebookRead" => (Tk::Read, "read".to_string()),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => (Tk::Edit, if name == "Write" { "write".into() } else { "edit".into() }),
        "Bash" | "Monitor" => (Tk::Bash, "bash".into()),
        "Grep" | "Glob" | "LSP" => (Tk::Grep, name.to_lowercase()),
        "WebSearch" | "WebFetch" => (Tk::Web, "web".into()),
        "Task" | "Agent" | "Workflow" => (Tk::Task, "agent".into()),
        n if n.starts_with("mcp__") => {
            let p: Vec<&str> = n.split("__").collect();
            (Tk::Oth, p.get(1).map(|x| x.trim_start_matches("claude_ai_").to_lowercase()).unwrap_or_else(|| "mcp".into()))
        }
        n => (Tk::Oth, n.to_lowercase()),
    };
    let a = match name {
        "Read" | "Edit" | "MultiEdit" | "Write" => rel(&sv(i, "file_path")),
        "NotebookEdit" | "NotebookRead" => rel(&sv(i, "notebook_path")),
        "Bash" => one(&sv(i, "command"), 140),
        "Monitor" => one(&sv(i, "description"), 140),
        "Grep" => {
            let p = sv(i, "path");
            if p.is_empty() { sv(i, "pattern") } else { format!("{}  {}", sv(i, "pattern"), rel(&p)) }
        }
        "Glob" => sv(i, "pattern"),
        "WebSearch" => sv(i, "query"),
        "WebFetch" => sv(i, "url"),
        "Task" | "Agent" => {
            let ty = sv(i, "subagent_type").to_lowercase();
            let d = sv(i, "description");
            if ty.is_empty() || ty == "general-purpose" { d } else { format!("{ty}: {d}") }
        }
        "Skill" => sv(i, "skill"),
        "TodoWrite" => "todos".into(),
        _ => {
            let s = i.to_string();
            if s == "{}" || s == "null" { String::new() } else { one(&s, 90) }
        }
    };
    let mut body = vec![];
    match name {
        "Edit" => {
            for l in sv(i, "old_string").lines() {
                body.push(Ln::D(0, l.into()));
            }
            for l in sv(i, "new_string").lines() {
                body.push(Ln::A(0, l.into()));
            }
        }
        "MultiEdit" => {
            for e in i.get("edits").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
                for l in sv(&e, "old_string").lines() {
                    body.push(Ln::D(0, l.into()));
                }
                for l in sv(&e, "new_string").lines() {
                    body.push(Ln::A(0, l.into()));
                }
            }
        }
        "Write" => {
            for l in sv(i, "content").lines().take(40) {
                body.push(Ln::A(0, l.into()));
            }
        }
        _ => {}
    }
    let sum = if k == Tk::Edit {
        let ad = body.iter().filter(|x| matches!(x, Ln::A(..))).count();
        let de = body.iter().filter(|x| matches!(x, Ln::D(..))).count();
        format!("+{ad} -{de}")
    } else {
        String::new()
    };
    Tool { k, nm, a, sum, body, st: St::Run, t0: 0, t1: 0 }
}

fn pdir(cwd: &str) -> String {
    let base = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| format!("{}/.claude", crate::sys::home()));
    let esc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    format!("{base}/projects/{esc}")
}

fn newest(cwd: &str) -> Option<String> {
    let mut v: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(pdir(cwd))
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path().file_stem()?.to_string_lossy().to_string())))
        .collect();
    v.sort();
    v.pop().map(|x| x.1)
}

fn rtext(c: &Value) -> String {
    match c.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

impl App {
    pub fn live(args: Vec<String>) -> io::Result<App> {
        let mut a = App::new();
        a.bs.clear();
        a.todo.clear();
        a.ctx = -1.0;
        a.h5 = -1.0;
        a.d7 = -1.0;
        a.mname = "…".into();
        a.effs = String::new();
        a.cwd = rel(&std::env::current_dir()?.to_string_lossy());
        a.wk = work_token().map(|_| ());
        a.work = TOK.lock().ok().is_some_and(|t| t.is_some());
        a.bgw = std::fs::read_to_string(crate::bgfile()).ok().and_then(|x| crate::bg::canon(&x)).filter(|k| crate::bg::exists(k));
        a.br = Command::new("git").args(["branch", "--show-current"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let mut args = args;
        if !args.iter().any(|x| x.starts_with("--permission-mode") || x == "--dangerously-skip-permissions") {
            if let Some(i) = crate::defmode().filter(|i| *i > 0) {
                args.extend(["--permission-mode".into(), PM[i].into()]);
                a.mode = i;
            }
        }
        if !args.iter().any(|x| x == "--effort" || x.starts_with("--effort=")) {
            if let Some(e) = crate::defeff() {
                args.extend(["--effort".into(), e.into()]);
                a.effs = e.into();
            }
        }
        let (ch, inp, rx) = spawn(&args).map_err(|e| if e.kind() == io::ErrorKind::NotFound { io::Error::new(e.kind(), "claude not found, install Claude Code first: https://claude.com/claude-code") } else { e })?;
        a.lv = Some(Lv { inp, rx, ch, rid: 0, tm: HashMap::new(), sm: HashMap::new(), bm: HashMap::new(), areq: None, sid: String::new(), args, brx: None, bch: None, tb: 0, uid: HashMap::new(), rwp: None, c0: 0.0, w0: 0.0, cmds: vec![] });
        a.ctrl("init", json!({"subtype": "initialize", "hooks": null, "perTaskStopAffordance": true}));
        let ar = a.lv.as_ref().map(|l| l.args.clone()).unwrap_or_default();
        let cwd = std::env::current_dir()?.to_string_lossy().to_string();
        let rid = if let Some(i) = ar.iter().position(|x| x == "-r" || x == "--resume") {
            ar.get(i + 1).filter(|x| !x.starts_with('-')).cloned()
        } else if ar.iter().any(|x| x == "-c" || x == "--continue") {
            newest(&cwd)
        } else {
            None
        };
        if let Some(id) = rid {
            if let Some(l) = &mut a.lv {
                l.sid = id.clone();
            }
            a.load(&id, &cwd);
        }
        a.ctrl("use", json!({"subtype": "get_usage"}));
        a.ctrl("ctx", json!({"subtype": "get_context_usage"}));
        Ok(a)
    }

    pub fn cmds(&self) -> Vec<(String, String)> {
        let mut v = match &self.lv {
            Some(l) if !l.cmds.is_empty() => l.cmds.clone(),
            _ if self.is_codex() => vec![("clear".to_string(), "start a new codex thread".to_string())],
            _ => CMDS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        };
        v.retain(|x| x.0 != "theme");
        v.push(("noname".into(), "noname settings".into()));
        v.push(("default-permission".into(), "permission mode new sessions start in".into()));
        v.push(("theme".into(), crate::pal::all().iter().map(|t| t.name).collect::<Vec<_>>().join(" ")));
        v.push(("background".into(), crate::bg::LIST.into()));
        v.push(("resume".into(), "pick an older session in this folder".into()));
        v.retain(|x| x.0 != "copy");
        v.push(("copy".into(), "copy the last answer  ·  /copy code for the last code block".into()));
        for (c, d) in [("model", "pick a model"), ("effort", "pick the effort level"), ("tasks", "background shells, monitors, agents and workflows"), ("mcp", "mcp servers: reconnect, enable or disable"), ("diff", "all changes in this workspace"), ("rename", "name this session")] {
            if self.lv.is_some() || matches!(c, "model" | "effort" | "tasks") {
                v.retain(|x| x.0 != c);
                v.push((c.into(), d.into()));
            }
        }
        if self.swok() {
            v.retain(|x| x.0 != "apolloclaw:switch");
            v.push(("switch".into(), "switch between personal and work account".into()));
        }
        v
    }

    fn swok(&self) -> bool {
        self.wk.is_some() && self.lv.as_ref().is_some_and(|l| l.cmds.iter().any(|c| c.0 == "apolloclaw:switch"))
    }

    pub fn is_live(&self) -> bool {
        self.lv.is_some()
    }

    fn send(&mut self, v: Value) {
        if let Some(l) = &mut self.lv {
            let _ = writeln!(l.inp, "{v}");
            let _ = l.inp.flush();
        }
    }

    fn ctrl(&mut self, tag: &str, req: Value) {
        let Some(l) = &mut self.lv else { return };
        l.rid += 1;
        let id = format!("{tag}-{}", l.rid);
        self.send(json!({"type": "control_request", "request_id": id, "request": req}));
    }

    pub(crate) fn lsubmit(&mut self, s: String) {
        if !self.busy {
            self.t0 = self.t;
        }
        self.bs.push(B::U(s.clone(), self.clock()));
        let w = self.d7;
        if let Some(l) = &mut self.lv {
            l.tb = self.bs.len();
            l.w0 = w;
        }
        self.busy = true;
        self.sb = None;
        let c = match self.content(&s) {
            Some(v) => {
                let mut c: Vec<Value> = v.into_iter().map(|(m, b)| json!({"type": "image", "source": {"type": "base64", "media_type": m, "data": crate::img::b64(b)}})).collect();
                c.push(json!({"type": "text", "text": self.expand(&s)}));
                Value::Array(c)
            }
            None => Value::String(self.expand(&s)),
        };
        self.send(json!({"type": "user", "message": {"role": "user", "content": c}}));
    }

    pub(crate) fn lcmd(&mut self, r: &str) {
        let (c, arg) = r.split_once(' ').unwrap_or((r, ""));
        match c {
            "btw" if !arg.trim().is_empty() => self.lbtw(arg.trim()),
            "clear" | "new" => {
                let args: Vec<String> = self.lv.as_ref().map(|l| l.args.iter().filter(|x| !matches!(x.as_str(), "-c" | "--continue" | "-r" | "--resume")).cloned().collect()).unwrap_or_default();
                if let Ok(mut n) = App::live(args) {
                    n.tc = self.t;
                    n.t = self.t;
                    n.hist = std::mem::take(&mut self.hist);
                    n.pst = std::mem::take(&mut self.pst);
                    *self = n;
                }
            }
            "model" if arg.is_empty() => self.modelpicker(),
            "model" if self.pickmodel(arg.trim()).is_some() => {
                // switch right here, so the name on screen changes at once instead of at the next turn
                let v = self.pickmodel(arg.trim()).unwrap_or_default();
                self.setmodel(&v);
            }
            "effort" if arg.is_empty() => self.effortpicker(),
            "effort" => self.seteffort(arg.trim()),
            "mcp" if arg.is_empty() => {
                self.ctrl("mcs", json!({"subtype": "mcp_status"}));
                self.pk = Some(Pk { kind: 2, title: "mcp".into(), ..Default::default() });
            }
            "diff" if arg.is_empty() => {
                self.ctrl("dif", json!({"subtype": "get_workspace_diff"}));
                self.pk = Some(Pk { kind: 3, title: "diff".into(), ..Default::default() });
            }
            "rename" if !arg.trim().is_empty() => {
                let t = arg.trim().to_string();
                self.ctrl("ren", json!({"subtype": "rename_session", "title": t, "source": "host"}));
                self.stitle = t;
            }
            "quit" | "exit" => self.quit = true,
            "switch" | "apolloclaw:switch" if self.swok() => self.lswitch(),
            "resume" | "continue" if arg.is_empty() => {
                let v = self.sessions();
                if v.is_empty() {
                    self.toast("no sessions here");
                } else {
                    self.rs = Some((v, 0));
                }
            }
            "resume" => self.lresume(arg.trim()),
            _ => {
                if c == "effort" && !arg.is_empty() {
                    self.effs = arg.to_string();
                }
                if !self.busy {
                    self.t0 = self.t;
                }
                self.bs.push(B::Sys(format!("/{r}")));
                if let Some(l) = &mut self.lv {
                    l.tb = self.bs.len();
                }
                self.busy = true;
                self.send(json!({"type": "user", "message": {"role": "user", "content": format!("/{r}")}}));
            }
        }
    }

    fn lbtw(&mut self, q: &str) {
        let sid = self.lv.as_ref().map(|l| l.sid.clone()).unwrap_or_default();
        let mut c = Command::new(crate::sys::bin("claude"));
        c.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence", "--tools", ""]);
        if !sid.is_empty() {
            c.args(["--resume", &sid, "--fork-session"]);
        }
        c.arg(format!("Side question, answer briefly in 1-3 sentences without tools: {q}"));
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        tokenv(&mut c);
        guard(&mut c);
        let Ok(mut ch) = c.spawn() else {
            self.toast("btw failed");
            return;
        };
        track(&ch);
        let rx = rd(ch.stdout.take().unwrap());
        if let Some(l) = &mut self.lv {
            if let Some(mut o) = l.bch.take() {
                reap(&mut o);
            }
            l.brx = Some(rx);
            l.bch = Some(ch);
        }
        self.btw = Some(Btw { q: q.into(), a: String::new(), full: String::new(), fin: false });
    }

    pub fn sessions(&self) -> Vec<(String, String, String)> {
        let cwd = std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let cur = self.lv.as_ref().map(|l| l.sid.clone()).unwrap_or_default();
        let Ok(rd) = std::fs::read_dir(pdir(&cwd)) else { return vec![] };
        let now = std::time::SystemTime::now();
        let mut v: Vec<(std::time::SystemTime, String, String, String)> = vec![];
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let id = p.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            if id == cur {
                continue;
            }
            let Ok(m) = e.metadata().and_then(|m| m.modified()) else { continue };
            let Ok(f) = std::fs::read_to_string(&p) else { continue };
            let (mut first, mut ct, mut at, mut sm, mut br) = (String::new(), String::new(), String::new(), String::new(), String::new());
            let mut n = 0usize;
            for l in f.lines() {
                if !(l.contains("\"type\":\"user\"") || l.contains("\"type\":\"assistant\"") || l.contains("title\"") || l.contains("\"summary\"")) {
                    continue;
                }
                let Ok(o) = serde_json::from_str::<Value>(l) else { continue };
                match o["type"].as_str().unwrap_or("") {
                    "custom-title" => ct = sv(&o, "customTitle"),
                    "ai-title" => at = sv(&o, "aiTitle"),
                    "summary" => sm = sv(&o, "summary"),
                    "assistant" if o["isSidechain"] != true => n += 1,
                    "user" if o["isSidechain"] != true && o["isMeta"] != true => {
                        let c = &o["message"]["content"];
                        let t = match c {
                            Value::String(t) => t.clone(),
                            Value::Array(a) => a.iter().filter(|x| x["type"] == "text").filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join(" "),
                            _ => String::new(),
                        };
                        if t.is_empty() || t.starts_with('<') || t.starts_with("[Request") {
                            continue;
                        }
                        n += 1;
                        if first.is_empty() {
                            first = t;
                        }
                        let b = sv(&o, "gitBranch");
                        if !b.is_empty() && b != "HEAD" {
                            br = b;
                        }
                    }
                    _ => {}
                }
            }
            let title = [ct, at, sm, first].into_iter().find(|x| !x.trim().is_empty()).unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let s = now.duration_since(m).map(|d| d.as_secs()).unwrap_or(0);
            let age = if s < 60 { "just now".to_string() } else if s < 3600 { format!("{}m ago", s / 60) } else if s < 86400 { format!("{}h ago", s / 3600) } else { format!("{}d ago", s / 86400) };
            let mut meta = format!("{age}  ·  {n} messages");
            if !br.is_empty() {
                meta += &format!("  ·  {br}");
            }
            v.push((m, id, one(&title, 120), meta));
        }
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().take(50).map(|(_, id, t, m)| (id, t, m)).collect()
    }

    pub(crate) fn lswitch(&mut self) {
        let to_work = !self.work;
        let tok = if to_work {
            match work_token() {
                Some(t) => Some(t),
                None => {
                    self.bs.push(B::Sys(format!("no work token in the keyring ({})", tsvc())));
                    return;
                }
            }
        } else {
            None
        };
        if let Ok(mut t) = TOK.lock() {
            *t = tok;
        }
        let sid = self.lv.as_ref().map(|l| l.sid.clone()).unwrap_or_default();
        if sid.is_empty() {
            self.work = to_work;
            self.lcmd("clear");
            self.work = to_work;
        } else {
            self.lresume(&sid);
            self.work = to_work;
        }
        self.toast(if to_work { "account  work" } else { "account  personal" });
    }

    pub(crate) fn lresume(&mut self, id: &str) {
        let mut args: Vec<String> = self.lv.as_ref().map(|l| l.args.clone()).unwrap_or_default();
        let mut o = vec![];
        let mut it = args.drain(..);
        while let Some(x) = it.next() {
            match x.as_str() {
                "-c" | "--continue" | "--fork-session" => {}
                "-r" | "--resume" | "--resume-session-at" => {
                    it.next();
                }
                _ => o.push(x),
            }
        }
        o.extend(["--resume".into(), id.to_string()]);
        match App::live(o) {
            Ok(mut n) => {
                n.t = self.t;
                n.hist = std::mem::take(&mut self.hist);
                n.pst = std::mem::take(&mut self.pst);
                n.bgw = self.bgw;
                *self = n;
            }
            Err(_) => self.toast("resume failed"),
        }
    }

    pub(crate) fn lmode(&mut self) {
        let m = PM[self.mode];
        self.ctrl("pm", json!({"subtype": "set_permission_mode", "mode": m}));
    }

    pub(crate) fn lstop(&mut self) {
        self.ctrl("int", json!({"subtype": "interrupt"}));
        if self.ask.is_some() {
            self.lanswer('n');
        }
        if self.qa.is_some() {
            self.qdone(false);
        }
        for b in self.bs.iter_mut() {
            if let B::Tl(t) = b {
                if t.st == St::Run {
                    t.st = St::Err;
                    t.t1 = self.t;
                    t.sum = "interrupted".into();
                }
            }
        }
        self.sb = None;
        self.bs.push(B::Sys("interrupted".into()));
    }

    pub(crate) fn lqa(&mut self, q: Qa, ok: bool) {
        if q.plan {
            let a = q.ans.first().map(|x| x.1.clone()).unwrap_or_default();
            let m = match a.as_str() {
                "yes, auto mode" => Some(2),
                "yes, approve edits" => Some(0),
                "yes, bypass permissions" => Some(3),
                _ => None,
            };
            let r = match m {
                Some(i) if ok => {
                    self.mode = i;
                    self.tm = self.t.max(1);
                    json!({"behavior": "allow", "updatedInput": q.inp, "updatedPermissions": [{"type": "setMode", "mode": PM[i], "destination": "session"}]})
                }
                _ if ok && a != "no, keep planning" => json!({"behavior": "deny", "message": format!("The user wants to keep planning: {a}")}),
                _ => json!({"behavior": "deny", "message": "The user wants to keep planning."}),
            };
            self.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": q.id, "response": r}}));
            return;
        }
        let r = if ok {
            let mut inp = q.inp.clone();
            let m: serde_json::Map<String, Value> = q.ans.iter().map(|(a, b)| (a.clone(), json!(b))).collect();
            inp["answers"] = Value::Object(m);
            json!({"behavior": "allow", "updatedInput": inp})
        } else {
            json!({"behavior": "deny", "message": "The user dismissed the question."})
        };
        self.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": q.id, "response": r}}));
    }

    pub(crate) fn lanswer(&mut self, c: char) {
        let Some(i) = self.ask.take() else { return };
        if let B::Ask(a) = &mut self.bs[i] {
            a.ans = Some(c);
        }
        let Some((id, inp, sug)) = self.lv.as_mut().and_then(|l| l.areq.take()) else { return };
        let r = match c {
            'n' => json!({"behavior": "deny", "message": "The user denied this action."}),
            'a' => json!({"behavior": "allow", "updatedInput": inp, "updatedPermissions": sug}),
            _ => json!({"behavior": "allow", "updatedInput": inp}),
        };
        self.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": r}}));
    }

    pub(crate) fn pump(&mut self) {
        let evs: Vec<Value> = match &self.lv {
            Some(l) => l.rx.try_iter().collect(),
            None => return,
        };
        for e in evs {
            self.ev(e);
        }
        let bev: Vec<Value> = match self.lv.as_ref().and_then(|l| l.brx.as_ref()) {
            Some(r) => r.try_iter().collect(),
            None => vec![],
        };
        for e in bev {
            self.bev(e);
        }
    }

    fn bev(&mut self, e: Value) {
        let Some(b) = &mut self.btw else { return };
        match e["type"].as_str().unwrap_or("") {
            "stream_event" => {
                let d = &e["event"]["delta"];
                if d["type"] == "text_delta" {
                    b.full.push_str(d["text"].as_str().unwrap_or(""));
                }
            }
            "result" => {
                if b.full.is_empty() {
                    b.full = e["result"].as_str().unwrap_or("(no answer)").to_string();
                }
                b.fin = true;
            }
            "_eof" => b.fin = true,
            _ => {}
        }
    }

    fn ev(&mut self, e: Value) {
        let ty = e["type"].as_str().unwrap_or("").to_string();
        if let Some(p) = e.get("parent_tool_use_id").and_then(|v| v.as_str()) {
            let mut p = p.to_string();
            if let Some(r) = self.lv.as_ref().and_then(|l| l.sm.get(&p).cloned()) {
                p = r;
            }
            if e["type"] == "assistant" {
                for c in e["message"]["content"].as_array().cloned().unwrap_or_default() {
                    match c["type"].as_str().unwrap_or("") {
                        "tool_use" => {
                            let n = sv(&c, "name");
                            if matches!(n.as_str(), "Task" | "Agent") {
                                let id = sv(&c, "id");
                                if let Some(l) = &mut self.lv {
                                    l.sm.insert(id, p.clone());
                                }
                            }
                            let t = mk(&n, &c["input"]);
                            self.subact(&p, format!("{} {}", t.nm, t.a).trim().to_string());
                        }
                        "text" => self.subtxt(&p, &sv(&c, "text")),
                        _ => {}
                    }
                }
            }
            return;
        }
        match ty.as_str() {
            "system" => match e["subtype"].as_str().unwrap_or("") {
                "init" => {
                    let sid = sv(&e, "session_id");
                    let rs = self.lv.as_ref().is_some_and(|l| l.sid.is_empty() && l.args.iter().any(|x| matches!(x.as_str(), "-c" | "--continue" | "-r" | "--resume")));
                    if let Some(l) = &mut self.lv {
                        l.sid = sid.clone();
                    }
                    if rs {
                        self.load(&sid, &sv(&e, "cwd"));
                    }
                    let m = sv(&e, "model");
                    if let Some(i) = MID_ALL.iter().position(|x| m.starts_with(x.0)) {
                        self.model = i;
                    }
                    self.mname = mnm(&m);
                    let pm = sv(&e, "permissionMode");
                    if let Some(i) = PM.iter().position(|x| *x == pm) {
                        self.mode = i;
                    }
                }
                "compact_boundary" => self.bs.push(B::Sys("conversation compacted".into())),
                "local_command_output" => {
                    let c = strip(&sv(&e, "content"));
                    if !c.trim().is_empty() {
                        self.bs.push(B::T(c.trim().to_string()));
                    }
                }
                "api_retry" | "api_error" => {
                    let (at, mx) = (e["attempt"].as_u64().or(e["retry_attempt"].as_u64()).unwrap_or(0), e["max_retries"].as_u64().unwrap_or(0));
                    let ms = e["retry_delay_ms"].as_u64().or(e["retry_in_ms"].as_u64()).unwrap_or(0);
                    let st = e["error_status"].as_u64().or(e["error"]["status"].as_u64());
                    let why = match st {
                        Some(429) => "rate limited".to_string(),
                        Some(529) => "overloaded".to_string(),
                        Some(s) => format!("error {s}"),
                        None => "connection error".to_string(),
                    };
                    self.retry = Some((format!("{why}  ·  attempt {at}/{mx}"), self.t + ms * 30 / 1000));
                }
                "model_fallback" => {
                    let c = sv(&e, "content");
                    let f = sv(&e, "fallback_model");
                    self.bs.push(B::Sys(if c.is_empty() { format!("switched to {} ({})", mnm(&f), sv(&e, "trigger").replace('_', " ")) } else { one(&c, 200) }));
                }
                "notification" => {
                    let t = sv(&e, "text");
                    if !t.is_empty() {
                        self.toast(one(&t, 60));
                    }
                }
                "permission_denied" => {
                    let r = sv(&e, "decision_reason_type");
                    let t = mk(&sv(&e, "tool_name"), &Value::Null);
                    self.bs.push(B::Sys(format!("{} denied{}", t.nm, if r.is_empty() { String::new() } else { format!(" by {r}") })));
                }
                "session_title_changed" => self.stitle = one(&sv(&e, "title"), 80),
                "hook_started" => {
                    let id = sv(&e, "hook_id");
                    let now = self.t;
                    self.hooks.retain(|h| h.0 != id);
                    self.hooks.push((id, sv(&e, "hook_event"), now));
                }
                "hook_response" => {
                    let id = sv(&e, "hook_id");
                    self.hooks.retain(|h| h.0 != id);
                    let code = e["exit_code"].as_i64().unwrap_or(0);
                    if sv(&e, "outcome") == "error" || code == 2 {
                        let er = strip(&sv(&e, "stderr"));
                        let er = er.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                        self.bs.push(B::Sys(format!("hook {} {}{}", sv(&e, "hook_event"), if code == 2 { "blocked" } else { "failed" }, if er.is_empty() { String::new() } else { format!(": {}", one(&er, 120)) })));
                    }
                }
                "stop_hook_summary" => {
                    let errs: Vec<String> = e["hook_errors"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(|s| one(s, 120))).collect();
                    if e["prevented_continuation"] == true {
                        self.bs.push(B::Sys(format!("stop hook kept going{}", errs.first().map(|x| format!(": {x}")).unwrap_or_default())));
                    } else if let Some(x) = errs.first() {
                        self.bs.push(B::Sys(format!("stop hook error: {x}")));
                    }
                }
                "task_started" => {
                    let id = sv(&e, "task_id");
                    let ty = sv(&e, "task_type");
                    let kind = match ty.as_str() {
                        "local_bash" => 0,
                        t if t.starts_with("monitor") => 1,
                        t if t.contains("workflow") => 3,
                        _ => 2,
                    };
                    let name = sv(&e, "workflow_name");
                    let mut desc = sv(&e, "description");
                    if desc.is_empty() {
                        desc = name.clone();
                    }
                    let now = self.t.max(1);
                    let tool = sv(&e, "tool_use_id");
                    match self.tasks.iter_mut().find(|t| t.id == id) {
                        Some(t) => {
                            t.desc = desc;
                            t.st = 0;
                        }
                        None => self.tasks.push(Task { id, kind, desc: one(&desc, 200), t0: now, tool, name: one(&name, 60), ..Default::default() }),
                    }
                }
                "task_progress" => {
                    let id = sv(&e, "task_id");
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        let s = sv(&e, "summary");
                        let l = sv(&e, "last_tool_name");
                        if !s.is_empty() {
                            t.last = one(&s, 200);
                        } else if !l.is_empty() {
                            t.last = l;
                        }
                        t.tools = e["usage"]["tool_uses"].as_u64().unwrap_or(t.tools);
                        t.toks = e["usage"]["total_tokens"].as_u64().unwrap_or(t.toks);
                        let mut ph = String::new();
                        for r in e["workflow_progress"].as_array().map(|v| v.as_slice()).unwrap_or(&[]) {
                            if r["type"] != "workflow_agent" {
                                continue;
                            }
                            let ix = r["index"].as_u64().unwrap_or(0);
                            let n = match r["state"].as_str().unwrap_or("") {
                                "done" => 1,
                                "error" => 2,
                                _ => 0,
                            };
                            if n == 0 {
                                ph = sv(r, "phaseTitle");
                            }
                            match t.wf.iter_mut().find(|x| x.0 == ix) {
                                Some(x) => x.1 = x.1.max(n),
                                None => t.wf.push((ix, n)),
                            }
                        }
                        if !ph.is_empty() {
                            t.phase = one(&ph, 80);
                        }
                    }
                }
                "task_updated" | "task_notification" => {
                    let id = sv(&e, "task_id");
                    let st = if e["subtype"] == "task_updated" { sv(&e["patch"], "status") } else { sv(&e, "status") };
                    let now = self.t.max(1);
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        let n = match st.as_str() {
                            "" => t.st,
                            "running" | "pending" => 0,
                            "completed" | "done" | "success" => 1,
                            "failed" | "error" => 2,
                            _ => 3,
                        };
                        if n != 0 && t.st == 0 {
                            t.t1 = now;
                        }
                        t.st = n;
                        let s = sv(&e, "summary");
                        if !s.is_empty() {
                            t.last = one(&s, 200);
                        }
                        let d = sv(&e["patch"], "description");
                        if !d.is_empty() {
                            t.desc = one(&d, 200);
                        }
                    }
                }
                _ => {}
            },
            "stream_event" => {
                self.retry = None;
                self.se(&e["event"])
            }
            "assistant" => {
                // the model that really answered; keeps the name on screen true whatever changed it
                let m = sv(&e["message"], "model");
                if e["parent_tool_use_id"].is_null() && m.starts_with("claude-") {
                    if let Some(i) = MID_ALL.iter().position(|x| m.starts_with(x.0)) {
                        self.model = i;
                    }
                    self.mname = mnm(&m);
                }
                for c in e["message"]["content"].as_array().cloned().unwrap_or_default() {
                    if c["type"] == "tool_use" {
                        self.tuse(&c);
                    }
                }
            }
            "user" => {
                let c = &e["message"]["content"];
                let txt = c.is_string() || c.as_array().is_some_and(|a| a.iter().any(|x| x["type"] == "text"));
                if txt {
                    let u = sv(&e, "uuid");
                    if !u.is_empty() {
                        if let Some(i) = self.bs.iter().rposition(|b| matches!(b, B::U(..))) {
                            if let Some(l) = &mut self.lv {
                                l.uid.entry(i).or_insert(u);
                            }
                        }
                    }
                }
                if let Some(a) = e["message"]["content"].as_array() {
                    for c in a.clone() {
                        if c["type"] == "tool_result" {
                            self.tres(&c, &e["tool_use_result"]);
                        }
                    }
                }
            }
            "control_request" => {
                let r = &e["request"];
                if r["subtype"] == "can_use_tool" && sv(r, "tool_name") == "AskUserQuestion" {
                    let qs: Vec<Qq> = r["input"]["questions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|x| Qq {
                            q: sv(x, "question"),
                            h: sv(x, "header"),
                            opts: x["options"].as_array().cloned().unwrap_or_default().iter().map(|o| (sv(o, "label"), sv(o, "description"))).collect(),
                            multi: x["multiSelect"] == true,
                        })
                        .collect();
                    let id = sv(&e, "request_id");
                    if qs.is_empty() {
                        self.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "updatedInput": r["input"].clone()}}}));
                    } else {
                        let n = qs[0].opts.len();
                        self.qa = Some(Qa { id, inp: r["input"].clone(), qs, i: 0, sel: 0, pick: vec![false; n], ans: vec![], plan: false });
                    }
                } else if r["subtype"] == "can_use_tool" && sv(r, "tool_name") == "ExitPlanMode" {
                    let plan = sv(&r["input"], "plan");
                    if !plan.trim().is_empty() {
                        self.bs.push(B::T(plan.trim().to_string()));
                    }
                    let opts = vec![
                        ("yes, auto mode".to_string(), "Claude works on its own, the classifier checks risky steps".to_string()),
                        ("yes, approve edits".to_string(), "you approve each edit and command".to_string()),
                        ("yes, bypass permissions".to_string(), "no questions at all".to_string()),
                        ("no, keep planning".to_string(), String::new()),
                    ];
                    let id = sv(&e, "request_id");
                    self.qa = Some(Qa { id, inp: r["input"].clone(), qs: vec![Qq { q: "Ready to code?".into(), h: "plan".into(), opts, multi: false }], i: 0, sel: 0, pick: vec![false; 4], ans: vec![], plan: true });
                } else if r["subtype"] == "can_use_tool" {
                    let name = sv(r, "tool_name");
                    let t = mk(&name, &r["input"]);
                    let cmd = if t.a.is_empty() { t.nm.clone() } else { format!("{}  {}", t.nm, t.a) };
                    self.bs.push(B::Ask(Ask { cmd, ans: None }));
                    self.ask = Some(self.bs.len() - 1);
                    if let Some(l) = &mut self.lv {
                        l.areq = Some((sv(&e, "request_id"), r["input"].clone(), r.get("permission_suggestions").cloned().unwrap_or(json!([]))));
                    }
                } else {
                    let id = sv(&e, "request_id");
                    self.send(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "unsupported"}}));
                }
            }
            "control_response" => {
                let r = &e["response"];
                let id = sv(r, "request_id");
                let p = &r["response"];
                if id.starts_with("init") {
                    if let (Some(l), Some(cs)) = (&mut self.lv, p["commands"].as_array()) {
                        l.cmds = cs.iter().map(|c| (sv(c, "name"), one(&sv(c, "description"), 60))).collect();
                    }
                    let ar = self.lv.as_ref().map(|l| l.args.clone()).unwrap_or_default();
                    let want = ar.iter().position(|x| x == "--model").and_then(|i| ar.get(i + 1)).cloned().unwrap_or_else(|| "default".into());
                    if let Some(ms) = p["models"].as_array() {
                        let v: Vec<(String, String, String, Vec<String>)> = ms.iter().filter(|m| m["disabled"] != true).map(|m| (sv(m, "value"), sv(m, "displayName"), sv(m, "description"), m["supportedEffortLevels"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).collect();
                        if !v.is_empty() {
                            let mut v = v;
                            for (id, d) in MID_ALL.iter().skip(4) {
                                if !v.iter().any(|m| m.0 == *id || m.1 == *d) {
                                    v.push((id.to_string(), d.to_string(), String::new(), EFF.iter().map(|x| x.to_string()).collect()));
                                }
                            }
                            self.models = v;
                        }
                        let m = ms.iter().find(|m| sv(m, "value") == want).map(|m| sv(m, "resolvedModel")).filter(|x| !x.is_empty()).unwrap_or(want.clone());
                        if self.mname == "…" {
                            self.mname = mnm(&m);
                            if let Some(i) = MID_ALL.iter().position(|x| m.starts_with(x.0)) {
                                self.model = i;
                            }
                        }
                    }
                    let pm = sv(p, "current_permission_mode");
                    if let Some(i) = PM.iter().position(|x| *x == pm) {
                        self.mode = i;
                    }
                    self.syncset();
                } else if id.starts_with("mdl-") || id.starts_with("eff-") {
                    self.syncset();
                } else if id.starts_with("get-") {
                    let ap = &p["applied"];
                    let ef = sv(ap, "effort");
                    if !ef.is_empty() {
                        self.effs = ef;
                    }
                    let m = sv(ap, "model");
                    if m.starts_with("claude-") {
                        if let Some(i) = MID_ALL.iter().position(|x| m.starts_with(x.0)) {
                            self.model = i;
                        }
                        self.mname = mnm(&m);
                    }
                } else if id.starts_with("rwf") {
                    let n = p["filesChanged"].as_array().map(|x| x.len()).or_else(|| p["files_changed"].as_array().map(|x| x.len()));
                    match n {
                        Some(n) => self.toast(format!("restored {n} files")),
                        None if r["subtype"] == "error" => self.toast(format!("files not restored  {}", one(&sv(r, "error"), 40))),
                        None => self.toast("files restored"),
                    }
                    if let Some((i, conv)) = self.lv.as_mut().and_then(|l| l.rwp.take()) {
                        if conv {
                            self.rconv(i);
                        }
                    }
                } else if let Some(r) = id.strip_prefix("tko") {
                    let i: usize = r.split('-').next().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
                    let o = p["output"].as_str().map(str::to_string).or_else(|| p.as_object().and_then(|m| m.values().find_map(|v| v.as_str().map(str::to_string)))).unwrap_or_default();
                    if let Some(t) = self.tasks.get_mut(i) {
                        t.out = strip(&o);
                    }
                } else if id.starts_with("fsg") {
                    self.fsug = p["suggestions"].as_array().cloned().unwrap_or_default().iter().map(|x| sv(x, "path")).filter(|x| !x.is_empty()).take(12).collect();
                    self.ms = 0;
                } else if id.starts_with("mcs") {
                    let rows: Vec<(String, String, String)> = p["mcpServers"].as_array().cloned().unwrap_or_default().iter().map(|x| {
                        let st = sv(x, "status");
                        let er = sv(x, "error");
                        let n = x["tools"].as_array().map(|t| t.len()).unwrap_or(0);
                        let r = if st == "connected" && n > 0 { format!("{n} tools  connected") } else if !er.is_empty() { format!("{st}  {}", one(&er, 40)) } else { st };
                        (sv(x, "name"), r, sv(x, "name"))
                    }).collect();
                    if let Some(k) = &mut self.pk {
                        if k.kind == 2 {
                            k.sel = k.sel.min(rows.len().saturating_sub(1));
                            k.items = rows;
                        }
                    }
                } else if id.starts_with("mcr") || id.starts_with("mct") {
                    if r["subtype"] == "error" {
                        self.toast(one(&sv(r, "error"), 60));
                    }
                    self.ctrl("mcs", json!({"subtype": "mcp_status"}));
                } else if id.starts_with("dif") {
                    let d = p["diff"].clone();
                    let rows: Vec<(String, String, String)> = d["perFileStats"].as_array().cloned().unwrap_or_default().iter().map(|x| {
                        let st = sv(x, "status");
                        let r = if x["isBinary"] == true { "binary".to_string() } else { format!("+{} -{}", x["added"].as_u64().unwrap_or(0), x["removed"].as_u64().unwrap_or(0)) };
                        let path = sv(x, "path");
                        let label = if x["isUntracked"] == true { format!("{path}  new") } else if !st.is_empty() && st != "modified" { format!("{path}  {st}") } else { path.clone() };
                        (label, r, path)
                    }).collect();
                    let s = &d["stats"];
                    if let Some(k) = &mut self.pk {
                        if k.kind == 3 {
                            k.title = format!("diff  {} files  +{} -{}", s["filesCount"].as_u64().unwrap_or(0), s["linesAdded"].as_u64().unwrap_or(0), s["linesRemoved"].as_u64().unwrap_or(0));
                            k.items = rows;
                        }
                    }
                    if r["subtype"] == "error" {
                        self.toast(one(&sv(r, "error"), 60));
                    }
                    self.diff = Some(d);
                } else if id.starts_with("stp") {
                    if r["subtype"] == "error" {
                        self.toast(format!("not stopped  {}", one(&sv(r, "error"), 40)));
                    }
                } else if id.starts_with("ctx") {
                    let t = p["totalTokens"].as_f64().unwrap_or(0.0);
                    let m = p["maxTokens"].as_f64().unwrap_or(0.0);
                    if m > 0.0 {
                        self.ctx = (t / m * 100.0) as f32;
                    }
                } else if id.starts_with("use") {
                    let rl = &p["rate_limits"];
                    if let Some(u) = rl["five_hour"]["utilization"].as_f64() {
                        self.h5 = u as f32;
                    }
                    if let Some(u) = rl["seven_day"]["utilization"].as_f64() {
                        self.d7 = u as f32;
                    }
                }
            }
            "prompt_suggestion" => {
                let x = sv(&e, "suggestion");
                if !x.trim().is_empty() && self.inp.is_empty() {
                    self.sug = Some(x.trim().to_string());
                }
            }
            "rate_limit_event" => {
                let i = &e["rate_limit_info"];
                if let Some(u) = i["utilization"].as_f64() {
                    match i["rateLimitType"].as_str().unwrap_or("") {
                        "five_hour" => self.h5 = (u * 100.0) as f32,
                        "seven_day" => self.d7 = (u * 100.0) as f32,
                        _ => {}
                    }
                }
            }
            "result" => {
                self.busy = false;
                self.sb = None;
                self.syncset();
                let tb = self.lv.as_ref().map(|l| l.tb).unwrap_or(0);
                let txt = sv(&e, "result");
                let quiet = !self.bs.iter().skip(tb).any(|b| matches!(b, B::T(_) | B::Tl(_)));
                if (quiet || e["is_error"] == true) && !txt.trim().is_empty() {
                    self.bs.push(B::T(txt));
                }
                for b in self.bs.iter_mut() {
                    if let B::Tl(t) = b {
                        if t.st == St::Run {
                            t.st = St::Ok;
                            t.t1 = self.t;
                        }
                    }
                }
                let tot = e["total_cost_usd"].as_f64().unwrap_or(0.0);
                let u = &e["usage"];
                let tok = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum::<u64>();
                let secs = e["duration_ms"].as_f64().unwrap_or(0.0) as f32 / 1000.0;
                let (c0, w0) = self.lv.as_ref().map(|l| (l.c0, l.w0)).unwrap_or((0.0, 0.0));
                let dw = if w0 >= 0.0 { self.d7 - w0 } else { 0.0 };
                if tok > 0 {
                    let mut r = rcfmt(secs, tok, (tot - c0).max(0.0), dw);
                    let ne = self.tedits();
                    if ne > 0 {
                        r += &format!("  ·  {ne} edit{}", if ne == 1 { "" } else { "s" });
                    }
                    self.bs.push(B::Rc(r, self.t.max(1)));
                }
                if let Some(l) = &mut self.lv {
                    l.c0 = tot;
                }
                self.ctrl("ctx", json!({"subtype": "get_context_usage"}));
                self.ctrl("use", json!({"subtype": "get_usage"}));
            }
            "_eof" => {
                self.busy = false;
                self.bs.push(B::Sys("claude exited".into()));
            }
            _ => {}
        }
    }

    fn load(&mut self, sid: &str, cwd: &str) {
        let Ok(f) = std::fs::read_to_string(format!("{}/{sid}.jsonl", pdir(cwd))) else { return };
        let keep = std::mem::take(&mut self.bs);
        for l in f.lines() {
            let Ok(o) = serde_json::from_str::<Value>(l) else { continue };
            if o["isSidechain"] == true || o["isMeta"] == true {
                continue;
            }
            let c = &o["message"]["content"];
            match o["type"].as_str().unwrap_or("") {
                "user" => match c {
                    Value::String(t) if t.starts_with('<') => {
                        if let Some(x) = t.split("<command-name>").nth(1).and_then(|r| r.split('<').next()) {
                            self.bs.push(B::Sys(x.to_string()));
                        }
                    }
                    Value::String(t) if t.starts_with("[Request interrupted") => self.bs.push(B::Sys("interrupted".into())),
                    Value::String(t) => {
                        self.bs.push(B::U(t.clone(), String::new()));
                        let i = self.bs.len() - 1;
                        let u = sv(&o, "uuid");
                        if let Some(l) = &mut self.lv {
                            l.uid.insert(i, u);
                        }
                    }
                    Value::Array(a) => {
                        for x in a.clone() {
                            match x["type"].as_str().unwrap_or("") {
                                "tool_result" => self.tres(&x, &o["toolUseResult"]),
                                "text" if sv(&x, "text").starts_with("[Request interrupted") => self.bs.push(B::Sys("interrupted".into())),
                                "text" if !sv(&x, "text").starts_with('<') => self.bs.push(B::U(sv(&x, "text"), String::new())),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                },
                "assistant" => {
                    for x in c.as_array().cloned().unwrap_or_default() {
                        match x["type"].as_str().unwrap_or("") {
                            "text" => self.bs.push(B::T(sv(&x, "text"))),
                            "tool_use" => self.tuse(&x),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        for b in self.bs.iter_mut() {
            if let B::Tl(t) = b {
                t.t0 = 0;
                t.t1 = 0;
                if t.st == St::Run {
                    t.st = St::Ok;
                }
            }
        }
        self.bs.push(B::Sys(format!("resumed  {}", &sid[..8.min(sid.len())])));
        self.bs.extend(keep);
        if let Some(l) = &mut self.lv {
            l.tb = self.bs.len();
        }
    }

    pub(crate) fn lrewind(&mut self, i: usize, mode: usize) {
        let u = self.lv.as_ref().and_then(|l| l.uid.get(&i).cloned());
        let Some(u) = u else {
            self.toast("cannot rewind this message");
            return;
        };
        if mode == 1 {
            self.rconv(i);
            return;
        }
        if let Some(l) = &mut self.lv {
            l.rwp = Some((i, mode == 0));
        }
        self.ctrl("rwf", json!({"subtype": "rewind_files", "user_message_id": u}));
    }

    fn rconv(&mut self, i: usize) {
        let Some(l) = &self.lv else { return };
        let u = l.uid.get(&i).cloned().unwrap_or_default();
        let sid = l.sid.clone();
        let base: Vec<String> = {
            let mut v = vec![];
            let mut it = l.args.iter();
            while let Some(x) = it.next() {
                match x.as_str() {
                    "-c" | "--continue" | "--fork-session" => {}
                    "-r" | "--resume" | "--resume-session-at" => {
                        it.next();
                    }
                    _ => v.push(x.clone()),
                }
            }
            v
        };
        let cwd = std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let par = std::fs::read_to_string(format!("{}/{sid}.jsonl", pdir(&cwd)))
            .ok()
            .and_then(|f| f.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|o| sv(o, "uuid") == u).map(|o| sv(&o, "parentUuid")))
            .unwrap_or_default();
        let mut ex = base.clone();
        if !par.is_empty() && !sid.is_empty() {
            ex.extend(["--resume".into(), sid.clone(), "--resume-session-at".into(), par.clone(), "--fork-session".into()]);
        }
        let Ok((ch, inp, rx)) = spawn(&ex) else {
            self.toast("rewind failed");
            return;
        };
        let txt = match self.bs.get(i) {
            Some(B::U(t, _)) => t.clone(),
            _ => String::new(),
        };
        let cmds = l.cmds.clone();
        let c0 = l.c0;
        let uid: HashMap<usize, String> = l.uid.iter().filter(|(k, _)| **k < i).map(|(k, v)| (*k, v.clone())).collect();
        self.lv = Some(Lv { inp, rx, ch, rid: 0, tm: HashMap::new(), sm: HashMap::new(), bm: HashMap::new(), areq: None, sid: if par.is_empty() { String::new() } else { sid }, args: base, brx: None, bch: None, tb: 0, uid, rwp: None, c0, w0: -1.0, cmds });
        self.bs.truncate(i);
        self.ask = None;
        self.sb = None;
        self.busy = false;
        self.inp = txt;
        self.cur = self.inp.chars().count();
        self.ctrl("init", json!({"subtype": "initialize", "hooks": null, "perTaskStopAffordance": true}));
        self.toast("rewound");
    }

    fn se(&mut self, ev: &Value) {
        let idx = ev["index"].as_u64().unwrap_or(0);
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                if let Some(l) = &mut self.lv {
                    l.bm.clear();
                }
                self.busy = true;
            }
            "content_block_start" => {
                let cb = &ev["content_block"];
                let b = match cb["type"].as_str().unwrap_or("") {
                    "text" => B::T(String::new()),
                    "thinking" | "redacted_thinking" => B::Th(String::new()),
                    "tool_use" => {
                        let mut t = mk(&sv(cb, "name"), &json!({}));
                        t.t0 = self.t.max(1);
                        let id = sv(cb, "id");
                        self.bs.push(B::Tl(t));
                        let i = self.bs.len() - 1;
                        if let Some(l) = &mut self.lv {
                            l.tm.insert(id, i);
                            l.bm.insert(idx, i);
                        }
                        self.sel = Some(i);
                        return;
                    }
                    _ => return,
                };
                self.bs.push(b);
                let i = self.bs.len() - 1;
                self.sb = Some(i);
                if let Some(l) = &mut self.lv {
                    l.bm.insert(idx, i);
                }
            }
            "content_block_delta" => {
                let Some(i) = self.lv.as_ref().and_then(|l| l.bm.get(&idx).copied()) else { return };
                let d = &ev["delta"];
                let add = match d["type"].as_str().unwrap_or("") {
                    "text_delta" => sv(d, "text"),
                    "thinking_delta" => sv(d, "thinking"),
                    _ => return,
                };
                if let Some(B::T(s) | B::Th(s)) = self.bs.get_mut(i) {
                    s.push_str(&add);
                }
            }
            "content_block_stop" => {
                let i = self.lv.as_ref().and_then(|l| l.bm.get(&idx).copied());
                if i.is_some() && self.sb == i {
                    self.sb = None;
                }
            }
            _ => {}
        }
    }

    fn tuse(&mut self, c: &Value) {
        let id = sv(c, "id");
        let name = sv(c, "name");
        let mut t = mk(&name, &c["input"]);
        if name == "TodoWrite" {
            if let Some(ts) = c["input"]["todos"].as_array() {
                self.todo = ts
                    .iter()
                    .map(|x| {
                        let s = match x["status"].as_str().unwrap_or("") {
                            "completed" => 2,
                            "in_progress" => 1,
                            _ => 0,
                        };
                        (s, sv(x, "content"))
                    })
                    .collect();
                t.a = format!("{} items", self.todo.len());
            }
        }
        let ex = self.lv.as_ref().and_then(|l| l.tm.get(&id).copied());
        match ex {
            Some(i) => {
                if let Some(B::Tl(o)) = self.bs.get_mut(i) {
                    t.t0 = o.t0;
                    t.st = o.st;
                    t.t1 = o.t1;
                    if o.st != St::Run {
                        t.sum = o.sum.clone();
                        if t.k != Tk::Edit {
                            t.body = o.body.clone();
                        }
                    }
                    *o = t;
                }
            }
            None => {
                t.t0 = self.t.max(1);
                self.bs.push(B::Tl(t));
                let i = self.bs.len() - 1;
                if let Some(l) = &mut self.lv {
                    l.tm.insert(id, i);
                }
            }
        }
    }

    fn tres(&mut self, c: &Value, m: &Value) {
        let id = sv(c, "tool_use_id");
        let Some(i) = self.lv.as_ref().and_then(|l| l.tm.get(&id).copied()) else { return };
        let err = c["is_error"] == true;
        let txt = rtext(c);
        let now = self.t.max(1);
        if let Some(B::Tl(t)) = self.bs.get_mut(i) {
            t.t1 = now;
            if t.t0 == 0 {
                t.t0 = now;
            }
            let ls: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
            if err {
                t.st = St::Err;
                t.sum = one(ls.first().copied().unwrap_or("error"), 48);
                t.body = ls.iter().take(8).map(|l| Ln::P(one(l, 200))).collect();
                return;
            }
            t.st = St::Ok;
            match t.k {
                Tk::Edit => {}
                Tk::Task => {
                    if m["status"] == "async_launched" {
                        t.sum = "background".into();
                        return;
                    }
                    let n = m["totalToolUseCount"].as_u64().unwrap_or(t.body.iter().filter(|x| matches!(x, Ln::P(_))).count() as u64);
                    let mut s = vec![];
                    if n > 0 {
                        s.push(if n == 1 { "1 tool".to_string() } else { format!("{n} tools") });
                    }
                    if let Some(k) = m["totalTokens"].as_u64() {
                        s.push(format!("{:.1}k tok", k as f32 / 1000.0));
                    }
                    let ms = m["totalDurationMs"].as_u64().unwrap_or(if t.t1 > t.t0 { (t.t1 - t.t0) * 33 } else { 0 });
                    if ms > 0 {
                        let sc = ms / 1000;
                        s.push(if sc >= 60 { format!("{}m {}s", sc / 60, sc % 60) } else { format!("{sc}s") });
                    }
                    t.sum = if s.is_empty() { "done".into() } else { s.join(" · ") };
                }
                Tk::Read => t.sum = format!("{} lines", txt.lines().count()),
                Tk::Grep => t.sum = format!("{} results", ls.len()),
                Tk::Bash => {
                    t.sum = match ls.last() {
                        Some(l) if !l.starts_with('(') => one(l, 48),
                        _ => "ok".into(),
                    };
                    t.body = ls.iter().rev().take(400).rev().map(|l| Ln::P(one(l, 200))).collect();
                }
                _ => {
                    t.sum = if ls.is_empty() { "done".into() } else { format!("{} lines", ls.len()) };
                    t.body = ls.iter().take(12).map(|l| Ln::P(one(l, 200))).collect();
                }
            }
        }
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn model_command_names_resolve() {
        let a = App::new();
        for (q, want) in [("opus", "claude-opus-5-5"), ("Opus 5.5", "claude-opus-5-5"), ("sonnet", "claude-sonnet-5-5"), ("haiku-5-5", "claude-haiku-5-5"), ("claude-fable-5-1", "claude-fable-5-1")] {
            assert_eq!(a.pickmodel(q).as_deref(), Some(want), "{q}");
        }
        assert_eq!(a.pickmodel("nonsense"), None);
        assert_eq!(a.pickmodel(""), None);
    }

    #[test]
    fn setmodel_changes_the_name_at_once() {
        let mut a = App::new();
        a.setmodel("claude-opus-5-5");
        assert_eq!(a.mname, "opus-5.5");
        a.setmodel("claude-haiku-5-5");
        assert_eq!(a.mname, "haiku-5.5");
    }
}
