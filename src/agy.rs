//! Antigravity CLI (`agy`) as the engine: one `agy -p ... --output-format stream-json` per message.
//!
//! Event format: <https://antigravity.google/docs/cli/headless/> (`init`, `step_update`, `result`).

use std::process::Command;

use serde_json::Value;

use super::codex::{Cx, one, sv};
use super::*;

/// The model from `~/.gemini/antigravity-cli/settings.json`, or "antigravity".
/// The `init` event replaces it with the real model once a run has started.
pub(crate) fn cfg_model() -> String {
    std::fs::read_to_string(format!(
        "{}/.gemini/antigravity-cli/settings.json",
        crate::sys::home()
    ))
    .ok()
    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    .and_then(|v| {
        ["model", "defaultModel"]
            .iter()
            .find_map(|k| v[*k].as_str().filter(|m| !m.is_empty()).map(String::from))
    })
    .unwrap_or_else(|| "antigravity".into())
}

/// One `agy -p` per message, like the Codex engine runs one `codex exec`.
/// Slash commands and skills need no special case: print mode expands them itself.
pub(crate) fn build(
    c: &Cx,
    mode: usize,
    imgs: &[(&'static str, Vec<u8>)],
    t: u64,
    x: &str,
) -> Command {
    let mut cmd = Command::new(crate::sys::bin("agy"));
    cmd.args(["--output-format", "stream-json"]);
    match mode {
        1 => {
            cmd.args(["--mode", "plan"]);
        }
        // `--mode auto` needs the terminal sandbox and is not offered to business accounts: accept-edits works everywhere
        2 => {
            cmd.args(["--mode", "accept-edits"]);
        }
        3 => {
            cmd.arg("--dangerously-skip-permissions");
        }
        _ => {}
    }
    if let Some(id) = &c.tid {
        cmd.args(["--conversation", id]);
    } else if c.last {
        cmd.arg("-c");
    }
    // App::codex turns --model into -m; agy spells it out
    cmd.args(
        c.args
            .iter()
            .map(|a| if a == "-m" { "--model" } else { a.as_str() }),
    );
    // print mode takes text only: an image goes in as a file path the agent can open
    let mut p = x.to_string();
    for (k, (m, b)) in imgs.iter().enumerate() {
        let n = format!(
            "noname-{}-{t}-{k}.{}",
            std::process::id(),
            crate::img::ext(m)
        );
        if let Some(f) = crate::sys::tmp_write(&n, b) {
            p.push_str(&format!("\n\n(attached image: {})", f.display()));
        }
    }
    cmd.arg("-p").arg(p);
    cmd
}

fn base(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// The first non-empty string among `keys`, whatever the capitals of the key in `p`.
fn pick(p: &Value, keys: &[&str]) -> String {
    let Some(o) = p.as_object() else {
        return String::new();
    };
    keys.iter()
        .find_map(|k| {
            o.iter()
                .find(|(n, v)| {
                    n.eq_ignore_ascii_case(k) && v.as_str().is_some_and(|s| !s.is_empty())
                })
                .and_then(|(_, v)| v.as_str())
        })
        .unwrap_or("")
        .to_string()
}

fn secs_tok(secs: f32, tok: u64) -> String {
    let tk = if tok >= 1000 {
        format!("{:.1}k", tok as f64 / 1000.0)
    } else {
        tok.to_string()
    };
    format!("{secs:.1}s  ·  {tk} tok")
}

impl App {
    pub(crate) fn agev(&mut self, e: Value) {
        match e["event"].as_str().unwrap_or("") {
            "init" => {
                if let Some(m) = e["init"]["model"].as_str().filter(|m| !m.is_empty()) {
                    self.mname = m.to_string();
                }
            }
            "step_update" => self.agstep(&e["step_update"]),
            "result" => self.agresult(&e["result"]),
            "_stderr" => {
                if let Some(c) = &mut self.cx {
                    if c.err.len() < 4000 {
                        c.err.push_str(&sv(&e, "text"));
                        c.err.push('\n');
                    }
                }
            }
            _ if e["type"] == "_eof" => {
                let (fin, err) = self
                    .cx
                    .as_ref()
                    .map(|c| (c.fin, c.err.clone()))
                    .unwrap_or((true, String::new()));
                let ls: Vec<&str> = err
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                if !fin {
                    // the run ended with no result event: sign-in, unknown model, crash
                    let why = if ls.is_empty() {
                        "agy ended without an answer (is it installed and signed in? run `agy` once)".to_string()
                    } else {
                        ls.iter()
                            .rev()
                            .take(2)
                            .rev()
                            .map(|l| one(l, 160))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    };
                    self.bs.push(B::T(format!("**agy:** {why}")));
                } else if let Some(l) = ls.iter().find(|l| {
                    let l = l.to_lowercase();
                    l.contains("permission") || l.contains("denied") || l.contains("not allowed")
                }) {
                    // a denied tool is a soft failure: the run still ends well, so say it
                    self.bs.push(B::Sys(one(l, 160)));
                }
                self.cev(e);
            }
            _ => {}
        }
    }

    fn agresult(&mut self, r: &Value) {
        let id = sv(r, "conversation_id");
        let shown = self.cx.as_ref().is_some_and(|c| {
            c.im.iter()
                .any(|(k, &i)| k.starts_with('s') && matches!(self.bs.get(i), Some(B::T(_))))
        });
        if let Some(c) = &mut self.cx {
            c.fin = true;
            if c.tid.is_none() && !id.is_empty() {
                c.tid = Some(id);
            }
        }
        let resp = sv(r, "response");
        if !shown && !resp.trim().is_empty() {
            self.bs.push(B::T(resp.trim().to_string()));
        }
        let status = sv(r, "status");
        if !status.is_empty() && status != "SUCCESS" {
            let err = sv(r, "error");
            self.bs.push(B::T(format!(
                "**agy:** {}",
                one(if err.is_empty() { &status } else { &err }, 200)
            )));
        }
        let tok = r["usage"]["total_tokens"].as_u64().unwrap_or(0);
        let t0 = self.cx.as_ref().map(|c| c.t0).unwrap_or(self.t);
        let secs = r["duration_seconds"]
            .as_f64()
            .filter(|s| *s > 0.0)
            .map(|s| s as f32)
            .unwrap_or((self.t - t0) as f32 * 0.033);
        if tok > 0 {
            self.bs.push(B::Rc(secs_tok(secs, tok), self.t.max(1)));
        }
    }

    fn agstep(&mut self, s: &Value) {
        let id = sv(s, "conversation_id");
        if let Some(c) = &mut self.cx {
            if c.tid.is_none() && !id.is_empty() {
                c.tid = Some(id);
            }
        }
        let key = format!("s{}", s["step_index"].as_u64().unwrap_or(u64::MAX));
        let done = sv(s, "state") == "DONE";
        match sv(s, "step_type").as_str() {
            "agent_response" => {
                let d = sv(s, "text_delta");
                let ex = self.cx.as_ref().and_then(|c| c.im.get(&key).copied());
                match ex {
                    Some(i) => {
                        if let Some(B::T(t)) = self.bs.get_mut(i) {
                            // a closing event may repeat the whole text instead of adding the last piece
                            if done && d.len() >= t.len() && d.starts_with(t.as_str()) {
                                *t = d;
                            } else {
                                t.push_str(&d);
                            }
                        }
                    }
                    None if !d.trim().is_empty() => {
                        self.bs.push(B::T(d.trim_start().to_string()));
                        let i = self.bs.len() - 1;
                        if let Some(c) = &mut self.cx {
                            c.im.insert(key, i);
                        }
                    }
                    None => {}
                }
            }
            "tool" => self.agtool(s, &key, done),
            _ => {}
        }
    }

    fn agtool(&mut self, s: &Value, key: &str, done: bool) {
        let ti = &s["tool_info"];
        let nm = {
            let n = sv(s, "tool_name");
            if n.is_empty() { sv(ti, "name") } else { n }
        };
        let p = &ti["parameters"];
        let lc = nm.to_lowercase();
        let (k, a) = if lc == "run_command" || lc == "send_command_input" {
            (
                Tk::Bash,
                one(&pick(p, &["CommandLine", "Command", "Input"]), 140),
            )
        } else if lc.contains("replace") || lc.contains("write") || lc.contains("edit") {
            (
                Tk::Edit,
                base(&pick(
                    p,
                    &["TargetFile", "AbsolutePath", "FilePath", "Path", "File"],
                )),
            )
        } else if lc.starts_with("view") || lc.starts_with("list") || lc.starts_with("read_file") {
            (
                Tk::Read,
                base(&pick(
                    p,
                    &["AbsolutePath", "DirectoryPath", "FilePath", "Path", "File"],
                )),
            )
        } else if lc.contains("grep")
            || lc.contains("find")
            || lc.contains("search_code")
            || lc.contains("codebase")
        {
            (
                Tk::Grep,
                one(
                    &pick(p, &["Query", "Pattern", "SearchPattern", "SearchPath"]),
                    100,
                ),
            )
        } else if lc.contains("url") || lc.contains("web") || lc.contains("browser") {
            (
                Tk::Web,
                one(&pick(p, &["Url", "URL", "Query", "TaskName", "Task"]), 120),
            )
        } else if lc.contains("subagent") {
            (
                Tk::Task,
                one(
                    &pick(p, &["TaskName", "Task", "Description", "Prompt"]),
                    120,
                ),
            )
        } else {
            (Tk::Oth, one(&p.to_string(), 80))
        };
        let err = &ti["error"];
        let emsg = if err.is_null() {
            String::new()
        } else {
            err["message"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| err.to_string())
        };
        let st = if !err.is_null() {
            St::Err
        } else if done {
            St::Ok
        } else {
            St::Run
        };
        let out = match &ti["output"] {
            Value::String(o) => o.clone(),
            Value::Null => String::new(),
            o => o.to_string(),
        };
        let out = if st == St::Err { emsg } else { out };
        let ls: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        let low = out.to_lowercase();
        let sum = match (k, st) {
            (_, St::Run) => String::new(),
            (_, St::Err)
                if low.contains("permission")
                    || low.contains("denied")
                    || low.contains("approval") =>
            {
                "not allowed here, use auto or bypass mode".into()
            }
            (_, St::Err) => ls
                .first()
                .map(|l| one(l, 48))
                .unwrap_or_else(|| "failed".into()),
            (Tk::Bash, _) => ls.last().map(|l| one(l, 48)).unwrap_or_else(|| "ok".into()),
            _ => "done".into(),
        };
        let body: Vec<Ln> = if k == Tk::Bash {
            ls.iter()
                .rev()
                .take(12)
                .rev()
                .map(|l| Ln::P(one(l, 200)))
                .collect()
        } else {
            vec![]
        };
        let now = self.t.max(1);
        match self.cx.as_ref().and_then(|c| c.im.get(key).copied()) {
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
                    c.im.insert(key.to_string(), i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(c: &Cx, mode: usize, x: &str) -> Vec<String> {
        build(c, mode, &[], 0, x)
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect()
    }

    fn app() -> App {
        App::agy(vec![]).unwrap()
    }

    fn cx(a: &App) -> &Cx {
        a.cx.as_ref().unwrap()
    }

    fn step(i: u64, ty: &str, st: &str, extra: Value) -> Value {
        let mut s =
            json!({"conversation_id": "c-1", "step_index": i, "step_type": ty, "state": st});
        for (k, v) in extra.as_object().unwrap() {
            s[k] = v.clone();
        }
        json!({"event": "step_update", "step_update": s})
    }

    fn texts(a: &App) -> Vec<String> {
        a.bs.iter()
            .filter_map(|b| {
                if let B::T(t) = b {
                    Some(t.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn builds_a_print_run() {
        let a = app();
        let v = args(cx(&a), 0, "hello there");
        assert_eq!(v, ["--output-format", "stream-json", "-p", "hello there"]);
    }

    #[test]
    fn modes_map_to_agy_flags() {
        let a = app();
        assert!(
            args(cx(&a), 1, "x")
                .windows(2)
                .any(|w| w == ["--mode", "plan"])
        );
        assert!(
            args(cx(&a), 2, "x")
                .windows(2)
                .any(|w| w == ["--mode", "accept-edits"])
        );
        assert!(args(cx(&a), 3, "x").contains(&"--dangerously-skip-permissions".to_string()));
        let d = args(cx(&a), 0, "x");
        assert!(
            !d.contains(&"--mode".to_string())
                && !d.contains(&"--dangerously-skip-permissions".to_string())
        );
    }

    #[test]
    fn resume_model_and_slash_commands() {
        let mut a = App::agy(vec![
            "-c".into(),
            "--model".into(),
            "gemini-x".into(),
            "--effort".into(),
            "high".into(),
        ])
        .unwrap();
        let v = args(cx(&a), 0, "/researcher rust");
        assert!(v.contains(&"-c".to_string()));
        assert!(
            v.windows(2).any(|w| w == ["--model", "gemini-x"]) && !v.contains(&"-m".to_string())
        );
        assert!(v.windows(2).any(|w| w == ["--effort", "high"]));
        // a slash command goes through as the prompt: print mode expands it
        assert_eq!(v.last().map(String::as_str), Some("/researcher rust"));
        assert_eq!(a.mname, "gemini-x");
        a.cx.as_mut().unwrap().tid = Some("abc".into());
        let v = args(cx(&a), 0, "x");
        assert!(
            v.windows(2).any(|w| w == ["--conversation", "abc"]) && !v.contains(&"-c".to_string())
        );
    }

    #[test]
    fn streams_text_and_joins_deltas() {
        let mut a = app();
        a.agev(json!({"event": "init", "init": {"cwd": "/x", "model": "gemini-3", "permission_mode": "request-review"}}));
        assert_eq!(a.mname, "gemini-3");
        a.agev(step(0, "user_input", "DONE", json!({"text_delta": "hi"})));
        a.agev(step(
            1,
            "agent_response",
            "ACTIVE",
            json!({"text_delta": "Hel"}),
        ));
        a.agev(step(
            1,
            "agent_response",
            "ACTIVE",
            json!({"text_delta": "lo "}),
        ));
        a.agev(step(
            1,
            "agent_response",
            "DONE",
            json!({"text_delta": "world"}),
        ));
        assert_eq!(texts(&a), ["Hello world"]);
        assert_eq!(cx(&a).tid.as_deref(), Some("c-1"));
    }

    #[test]
    fn a_closing_event_with_the_whole_text_does_not_double_it() {
        let mut a = app();
        a.agev(step(
            1,
            "agent_response",
            "ACTIVE",
            json!({"text_delta": "Hello "}),
        ));
        a.agev(step(
            1,
            "agent_response",
            "DONE",
            json!({"text_delta": "Hello world"}),
        ));
        assert_eq!(texts(&a), ["Hello world"]);
        let mut a = app();
        a.agev(step(
            1,
            "agent_response",
            "DONE",
            json!({"text_delta": "one piece"}),
        ));
        assert_eq!(texts(&a), ["one piece"]);
    }

    #[test]
    fn a_shell_tool_goes_from_running_to_done() {
        let mut a = app();
        let ti = |out: Value| json!({"tool_name": "run_command", "tool_info": {"name": "run_command", "parameters": {"CommandLine": "echo hi"}, "output": out}});
        a.agev(step(2, "tool", "ACTIVE", ti(Value::Null)));
        assert!(
            matches!(&a.bs[0], B::Tl(t) if t.k == Tk::Bash && t.st == St::Run && t.a == "echo hi")
        );
        a.agev(step(2, "tool", "DONE", ti(json!("hi\n"))));
        assert_eq!(a.bs.len(), 1);
        assert!(matches!(&a.bs[0], B::Tl(t) if t.st == St::Ok && t.sum == "hi"));
    }

    #[test]
    fn a_failed_tool_is_an_error_and_a_denied_one_says_how_to_allow_it() {
        let mut a = app();
        a.agev(step(0, "tool", "DONE", json!({"tool_name": "view_file", "tool_info": {"parameters": {"AbsolutePath": "/a/b.txt"}, "error": {"type": "NOT_FOUND", "message": "no such file"}}})));
        assert!(
            matches!(&a.bs[0], B::Tl(t) if t.k == Tk::Read && t.a == "b.txt" && t.st == St::Err && t.sum == "no such file")
        );
        a.agev(step(1, "tool", "DONE", json!({"tool_name": "run_command", "tool_info": {"parameters": {"CommandLine": "rm x"}, "error": {"type": "PERMISSION", "message": "permission denied by policy"}}})));
        assert!(
            matches!(&a.bs[1], B::Tl(t) if t.st == St::Err && t.sum.contains("auto or bypass"))
        );
    }

    #[test]
    fn tool_kinds_follow_the_tool_name() {
        let mut a = app();
        for (i, (n, p)) in [
            ("replace_file_content", json!({"TargetFile": "/s/m.rs"})),
            ("grep_search", json!({"Query": "foo"})),
            ("read_url_content", json!({"Url": "https://x.y"})),
            ("mystery", json!({})),
        ]
        .into_iter()
        .enumerate()
        {
            a.agev(step(
                i as u64,
                "tool",
                "DONE",
                json!({"tool_name": n, "tool_info": {"parameters": p}}),
            ));
        }
        let k: Vec<Tk> =
            a.bs.iter()
                .filter_map(|b| if let B::Tl(t) = b { Some(t.k) } else { None })
                .collect();
        assert_eq!(k, [Tk::Edit, Tk::Grep, Tk::Web, Tk::Oth]);
    }

    #[test]
    fn result_shows_time_tokens_and_the_fallback_answer() {
        let mut a = app();
        a.agev(json!({"event": "result", "result": {"conversation_id": "c-9", "status": "SUCCESS", "response": "all done", "duration_seconds": 2.5, "usage": {"total_tokens": 1234}}}));
        assert_eq!(texts(&a), ["all done"]);
        assert!(matches!(a.bs.last(), Some(B::Rc(s, _)) if s == "2.5s  ·  1.2k tok"));
        assert_eq!(cx(&a).tid.as_deref(), Some("c-9"));
        assert!(cx(&a).fin);
        // text that already streamed is not shown a second time
        let mut a = app();
        a.agev(step(
            1,
            "agent_response",
            "DONE",
            json!({"text_delta": "hey"}),
        ));
        a.agev(json!({"event": "result", "result": {"status": "SUCCESS", "response": "hey"}}));
        assert_eq!(texts(&a), ["hey"]);
    }

    #[test]
    fn a_failed_run_shows_its_error() {
        let mut a = app();
        a.agev(json!({"event": "result", "result": {"status": "ERROR", "error": "unknown model", "conversation_id": ""}}));
        assert!(texts(&a).iter().any(|t| t == "**agy:** unknown model"));
        assert_eq!(cx(&a).tid, None);
    }

    #[test]
    fn no_result_event_shows_stderr_at_the_end() {
        let mut a = app();
        a.busy = true;
        a.agev(json!({"event": "_stderr", "text": "authentication required"}));
        a.agev(json!({"type": "_eof"}));
        assert!(
            texts(&a)
                .iter()
                .any(|t| t.contains("authentication required"))
        );
        assert!(!a.busy);
        // nothing on stderr: say what to check
        let mut a = app();
        a.agev(json!({"type": "_eof"}));
        assert!(texts(&a).iter().any(|t| t.contains("signed in")));
    }

    #[test]
    fn a_soft_denial_on_stderr_is_shown_after_a_good_run() {
        let mut a = app();
        a.agev(json!({"event": "_stderr", "text": "progress: 3 steps"}));
        a.agev(json!({"event": "_stderr", "text": "permission needed for command(git): add it under permissions.allow"}));
        a.agev(json!({"event": "result", "result": {"status": "SUCCESS", "response": "ok"}}));
        a.agev(json!({"type": "_eof"}));
        assert!(
            a.bs.iter()
                .any(|b| matches!(b, B::Sys(s) if s.contains("permission needed")))
        );
        assert!(
            !a.bs
                .iter()
                .any(|b| matches!(b, B::Sys(s) if s.contains("progress")))
        );
    }
}
