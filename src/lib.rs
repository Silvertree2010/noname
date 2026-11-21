//! noname: a quiet terminal UI for Claude Code (and Codex, Antigravity and opencode).
//!
//! The binary in `main.rs` draws the interface; this library holds the application state ([`App`]),
//! the engines that talk to the CLIs, and the platform helpers.

mod agy;
pub mod bg;
mod cm;
mod codex;
pub mod free;
pub mod img;
mod live;
mod opencode;
pub mod sys;
pub mod upd;

/// One entry of the model list: value, display name, description and effort levels.
pub type ModelEntry = (String, String, String, Vec<String>);
/// One rewind choice as shown in the rewind picker.
pub type RewindItem = (String, String, String);

use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers as Km, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub use ratatui::crossterm::event::{KeyCode as K, KeyEvent as Ke, KeyModifiers as M};

/// Colour themes: the built-in ones, user `.theme` files, and the colour accessors the UI draws with.
#[allow(non_snake_case)]
pub mod pal {
    use ratatui::style::Color;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One colour theme: a name, a short description and twelve colours.
    #[derive(Clone, Copy)]
    pub struct Theme {
        /// Theme name as typed in `/theme` and saved in the config.
        pub name: &'static str,
        /// One-line description shown next to the name in the theme picker.
        pub desc: &'static str,
        c: [u32; 12],
    }

    const BUILTIN: [Theme; 4] = [
        Theme {
            name: "default",
            desc: "white, gray, light purple",
            c: [
                0xe4e4e8, 0xffffff, 0xb4b4bc, 0x85858f, 0xc3b0ff, 0xd8caff, 0xcdc4e8, 0xa9d6e5,
                0xa6e3a1, 0xf38ba8, 0xe6b3ff, 0x161618,
            ],
        },
        Theme {
            name: "dracula",
            desc: "purple, pink, neon",
            c: [
                0xf8f8f2, 0xffffff, 0xc3c6dd, 0x7b88b8, 0xbd93f9, 0xff92df, 0xcaa9fa, 0x8be9fd,
                0x50fa7b, 0xff5555, 0xff79c6, 0x282a36,
            ],
        },
        Theme {
            name: "tokyo-night",
            desc: "night blue, city lights",
            c: [
                0xc0caf5, 0xe0e6ff, 0xa9b1d6, 0x737aa2, 0x7aa2f7, 0xbb9af7, 0xb4c2f0, 0x7dcfff,
                0x9ece6a, 0xf7768e, 0xff9e64, 0x1a1b26,
            ],
        },
        Theme {
            name: "mocha",
            desc: "soft pastels on dark roast",
            c: [
                0xcdd6f4, 0xeef1fc, 0xbac2de, 0x9399b2, 0xcba6f7, 0xb4befe, 0xf5c2e7, 0x89dceb,
                0xa6e3a1, 0xf38ba8, 0xf5c2e7, 0x1e1e2e,
            ],
        },
    ];

    static CUR: AtomicUsize = AtomicUsize::new(0);
    static ALL: std::sync::OnceLock<Vec<Theme>> = std::sync::OnceLock::new();

    const KEYS: [&str; 12] = [
        "fg", "bright", "muted", "faint", "accent", "accent2", "soft", "cyan", "green", "red",
        "pink", "base",
    ];

    /// Folder that holds the user's `.theme` files.
    pub fn dir() -> String {
        crate::sys::cfgfile("themes")
    }

    fn hx(v: &str) -> Option<u32> {
        let v = v.trim().trim_start_matches('#');
        let v = if v.len() == 3 {
            v.chars().flat_map(|c| [c, c]).collect()
        } else {
            v.to_string()
        };
        if v.len() != 6 {
            return None;
        }
        u32::from_str_radix(&v, 16).ok()
    }

    /// Builds a theme called `name` from `key = value` lines in `src`.
    /// Missing keys keep the default theme's colour; `desc = ...` sets the description.
    pub fn parse(name: &str, src: &str) -> Theme {
        let mut c = BUILTIN[0].c;
        let mut desc = String::from("custom");
        for l in src.lines() {
            let l = l.trim();
            let Some((k, v)) = l.split_once('=') else {
                continue;
            };
            let k = k.trim().to_lowercase();
            if k.starts_with('#') {
                continue;
            }
            if k == "desc" {
                desc = v.trim().to_string();
            } else if let (Some(i), Some(x)) = (KEYS.iter().position(|n| *n == k), hx(v)) {
                c[i] = x;
            }
        }
        Theme {
            name: Box::leak(name.to_string().into_boxed_str()),
            desc: Box::leak(desc.into_boxed_str()),
            c,
        }
    }

    fn user() -> Vec<Theme> {
        let Ok(rd) = std::fs::read_dir(dir()) else {
            return Vec::new();
        };
        let mut v: Vec<(String, String)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()?.to_str()? != "theme" {
                    return None;
                }
                let n = p.file_stem()?.to_str()?.to_lowercase();
                Some((n, std::fs::read_to_string(&p).ok()?))
            })
            .collect();
        v.sort();
        v.iter().map(|(n, s)| parse(n, s)).collect()
    }

    /// All themes: the built-in ones plus the user's, where a user theme with the same name replaces a built-in one.
    pub fn all() -> &'static [Theme] {
        ALL.get_or_init(|| {
            let mut v = BUILTIN.to_vec();
            for t in user() {
                match v.iter().position(|b| b.name == t.name) {
                    Some(i) => v[i] = t,
                    None => v.push(t),
                }
            }
            v
        })
    }

    /// Index of the active theme in `all`.
    pub fn cur() -> usize {
        CUR.load(Ordering::Relaxed)
    }
    /// Makes theme `i` the active one (clamped to the last theme).
    pub fn set(i: usize) {
        CUR.store(i.min(all().len() - 1), Ordering::Relaxed);
    }
    /// Index of the theme called `n`, ignoring case and punctuation; `catppuccin` and `tokyo` are aliases.
    pub fn find(n: &str) -> Option<usize> {
        let k = |x: &str| {
            x.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        };
        let n = k(n);
        all().iter().position(|t| {
            k(t.name) == n
                || (n == "catppuccin" && t.name == "mocha")
                || (n == "tokyo" && t.name == "tokyo-night")
        })
    }
    /// Path of the file that stores the selected theme name.
    pub fn file() -> String {
        crate::sys::cfgfile("theme")
    }
    /// Selects the theme named in `NONAME_THEME` or in the saved theme file, if there is one.
    pub fn load() {
        if let Some(i) = std::env::var("NONAME_THEME")
            .ok()
            .or_else(|| std::fs::read_to_string(file()).ok())
            .and_then(|x| find(&x))
        {
            set(i);
        }
    }
    /// Saves the active theme's name to the theme file.
    pub fn save() {
        let f = file();
        if let Some(d) = std::path::Path::new(&f).parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&f, all()[cur()].name);
    }
    fn c(i: usize) -> Color {
        let v = all()[cur()].c[i];
        Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
    /// Normal text colour.
    pub fn FG() -> Color {
        c(0)
    }
    /// Bright (white) text colour, for headings and emphasis.
    pub fn WH() -> Color {
        c(1)
    }
    /// Muted text colour.
    pub fn MU() -> Color {
        c(2)
    }
    /// Faint text colour, for hints and borders.
    pub fn FA() -> Color {
        c(3)
    }
    /// Main accent colour (light purple in the default theme).
    pub fn LI() -> Color {
        c(4)
    }
    /// Second accent colour.
    pub fn L2() -> Color {
        c(5)
    }
    /// Soft accent colour, used for code identifiers and quiet highlights.
    pub fn PE() -> Color {
        c(6)
    }
    /// Cyan, used for running state and info.
    pub fn CY() -> Color {
        c(7)
    }
    /// Green, used for success and added lines.
    pub fn GR() -> Color {
        c(8)
    }
    /// Red, used for errors and removed lines.
    pub fn RD() -> Color {
        c(9)
    }
    /// Pink, used for numbers and task or question highlights.
    pub fn PK() -> Color {
        c(10)
    }
    /// Base (background) colour of the theme.
    pub fn BASE() -> Color {
        c(11)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_theme_file() {
            let t = parse(
                "mine",
                "# comment\ndesc = black and red\naccent = #e0362b\nfg=ececec\nred = #f00\nbogus = #123456\npink = nope\n",
            );
            assert_eq!(t.name, "mine");
            assert_eq!(t.desc, "black and red");
            assert_eq!(t.c[4], 0xe0362b);
            assert_eq!(t.c[0], 0xececec);
            assert_eq!(t.c[9], 0xff0000);
            assert_eq!(t.c[10], BUILTIN[0].c[10]);
            assert_eq!(t.c[11], BUILTIN[0].c[11]);
        }
    }
}

/// Model names the mock engine and `/model` cycle through, in the same order as `MID_ALL`.
pub const MODELS: [&str; 13] = [
    "opus 5.5",
    "sonnet 5.5",
    "haiku 5.5",
    "fable 5.2",
    "fable 5.1",
    "opus 5",
    "sonnet 5",
    "fable 5",
    "opus 4.8",
    "opus 4.7",
    "opus 4.6",
    "sonnet 4.6",
    "haiku 4.5",
];
/// Window opacity steps in the /noname menu, in percent.
pub const OPAC: [u8; 8] = [100, 90, 80, 70, 60, 50, 40, 30];

/// Opacity when nothing is set in the /noname menu.
pub const OPAC_DEFAULT: u8 = 100;

/// noname's window opacity setting in percent (100 = leave the terminal alone).
pub fn opac() -> u8 {
    std::fs::read_to_string(sys::cfgfile("opacity"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|v| OPAC.contains(v))
        .unwrap_or(OPAC_DEFAULT)
}

/// Effort levels, lowest to highest.
pub const EFF: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Built-in model list as (model id, display name), in the same order as `MODELS`.
pub const MID_ALL: [(&str, &str); 13] = [
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
    ("claude-fable-5-2", "Fable 5.2"),
    ("claude-fable-5-1", "Fable 5.1"),
    ("claude-opus-5", "Opus 5"),
    ("claude-sonnet-5", "Sonnet 5"),
    ("claude-fable-5", "Fable 5"),
    ("claude-opus-4-8", "Opus 4.8"),
    ("claude-opus-4-7", "Opus 4.7"),
    ("claude-opus-4-6", "Opus 4.6"),
    ("claude-sonnet-4-6", "Sonnet 4.6"),
    ("claude-haiku-4-5", "Haiku 4.5"),
];
/// Permission modes in the order Shift+Tab cycles them: default, plan, auto, bypass.
pub const MODES: [&str; 4] = ["default", "plan", "auto", "bypass"];
/// Slash commands of the mock engine as (name, description), for the command menu.
pub const CMDS: [(&str, &str); 9] = [
    ("btw", "side question, main chat keeps going"),
    ("clear", "new session, empty context"),
    ("compact", "summarize to free context"),
    ("model", "switch model"),
    ("effort", "switch effort level"),
    ("context", "show context usage"),
    ("resume", "pick an older session"),
    ("usage", "plan limits and stats"),
    ("help", "keys and commands"),
];

/// Kind of a tool call, which decides its symbol and colour.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tk {
    /// File read.
    Read,
    /// File edit or write.
    Edit,
    /// Shell command.
    Bash,
    /// Search over files (grep or glob).
    Grep,
    /// Web search or fetch.
    Web,
    /// Subagent task.
    Task,
    /// Any other tool.
    Oth,
}
impl Tk {
    /// Short lowercase name of the kind, such as `read` or `bash`.
    pub fn nm(&self) -> &'static str {
        match self {
            Tk::Read => "read",
            Tk::Edit => "edit",
            Tk::Bash => "bash",
            Tk::Grep => "grep",
            Tk::Web => "web",
            Tk::Task => "task",
            Tk::Oth => "tool",
        }
    }
}

/// State of a tool call.
#[derive(Clone, Copy, PartialEq)]
pub enum St {
    /// Still running.
    Run,
    /// Finished successfully.
    Ok,
    /// Failed or was interrupted.
    Err,
}

/// One output line of a tool call.
#[derive(Clone)]
pub enum Ln {
    /// Plain output text.
    P(String),
    /// Added line of a diff: line number and text.
    A(usize, String),
    /// Removed line of a diff: line number and text.
    D(usize, String),
    /// Unchanged context line of a diff: line number and text.
    C(usize, String),
}

/// One tool call shown in the chat.
#[derive(Clone)]
pub struct Tool {
    /// Kind of tool.
    pub k: Tk,
    /// Display name of the tool.
    pub nm: String,
    /// Main argument shown after the name, such as a file path or a command.
    pub a: String,
    /// One-line result summary, such as `212 lines` or `+3 -2`.
    pub sum: String,
    /// Output lines shown when the call is unfolded.
    pub body: Vec<Ln>,
    /// Current state of the call.
    pub st: St,
    /// Tick at which the call started.
    pub t0: u64,
    /// Tick at which the call finished (0 while it runs).
    pub t1: u64,
}

/// A permission question for a command, shown in the chat.
#[derive(Clone)]
pub struct Ask {
    /// The command that needs permission.
    pub cmd: String,
    /// The answer given: `y` yes, `n` no, `a` yes and always; `None` while waiting.
    pub ans: Option<char>,
}

/// One question of a question dialog.
#[derive(Clone)]
pub struct Qq {
    /// The question text.
    pub q: String,
    /// Short header (tag) shown with the question.
    pub h: String,
    /// Answer options as (label, description).
    pub opts: Vec<(String, String)>,
    /// Whether several options can be picked.
    pub multi: bool,
}

/// State of an open question dialog (a question set from Claude or a plan approval).
pub struct Qa {
    /// Id of the request to answer when the dialog closes.
    pub id: String,
    /// The original tool input, sent back with the answer.
    pub inp: serde_json::Value,
    /// The questions of the dialog.
    pub qs: Vec<Qq>,
    /// Index of the question being asked.
    pub i: usize,
    /// Highlighted option; one past the last option means the free-text answer.
    pub sel: usize,
    /// Ticked options of the current question, for multi-select questions.
    pub pick: Vec<bool>,
    /// Answers so far as (question, answer).
    pub ans: Vec<(String, String)>,
    /// Whether this is a plan approval dialog instead of a question set.
    pub plan: bool,
}

/// One block of the chat.
pub enum B {
    /// An answered question: the question and the chosen answer.
    Qa(String, String),
    /// A user message and the clock time it was sent at.
    U(String, String),
    /// Assistant reply text.
    T(String),
    /// Assistant thinking text.
    Th(String),
    /// A tool call.
    Tl(Tool),
    /// A permission question for a command.
    Ask(Ask),
    /// A system line, such as `interrupted` or a command result.
    Sys(String),
    /// End-of-turn recap line (time, tokens, cost) and the tick it was added at.
    Rc(String, u64),
}

enum Sp {
    Th(String),
    Tx(String),
    Tl(Tool, u32),
    Fin,
    Ask(String),
    Todo(Vec<(u8, String)>),
    Rc(String),
}

/// A `/btw` side question and its answer, which is typed out letter by letter.
pub struct Btw {
    /// The side question.
    pub q: String,
    /// The part of the answer shown so far.
    pub a: String,
    full: String,
    fin: bool,
}
impl Btw {
    /// Whether the whole answer has been typed out.
    pub fn done(&self) -> bool {
        self.fin && self.a.len() >= self.full.len()
    }
}

/// All state of the interface: chat blocks, input box, pickers, timers and settings.
/// Every engine (mock, Claude, Codex, ...) fills it and `main.rs` draws it.
pub struct App {
    /// Chat blocks in display order.
    pub bs: Vec<B>,
    /// Text in the input box.
    pub inp: String,
    /// Cursor position in the input box, counted in characters.
    pub cur: usize,
    /// Chat scroll offset in lines from the bottom (0 = showing the newest).
    pub scr: usize,
    /// Whether tool output is shown unfolded (Ctrl+O).
    pub open: bool,
    q: VecDeque<Sp>,
    /// Whether a turn is running.
    pub busy: bool,
    /// Tick counter, about 30 per second; the other tick fields below are values of this counter.
    pub t: u64,
    wait: u32,
    /// Index of the current model in `MODELS` and `MID_ALL`.
    pub model: usize,
    /// Index of the current effort level in `EFF`.
    pub eff: usize,
    /// Index of the current permission mode in `MODES`.
    pub mode: usize,
    /// Context window used in percent (negative when not known yet).
    pub ctx: f32,
    /// Five-hour usage limit used in percent (negative when not known yet).
    pub h5: f32,
    /// Seven-day (week) usage limit used in percent (negative when not known yet).
    pub d7: f32,
    /// Working directory shown in the footer and the terminal title.
    pub cwd: String,
    /// Git branch of the working directory (empty when there is none).
    pub br: String,
    /// The `/btw` side question being shown, if any.
    pub btw: Option<Btw>,
    /// Todo list as (status, text); status 0 is pending, 1 in progress, 2 done.
    pub todo: Vec<(u8, String)>,
    /// Whether the todo list is shown (Ctrl+T).
    pub stodo: bool,
    /// Index in `bs` of the permission question waiting for an answer, if any.
    pub ask: Option<usize>,
    ti: Option<usize>,
    sb: Option<usize>,
    /// Number of prompts the mock engine has answered; it picks the next scripted reply.
    pub n: usize,
    /// Submitted prompts and commands, for Up/Down recall.
    pub hist: Vec<String>,
    hi: Option<usize>,
    /// Set to leave the main loop and exit.
    pub quit: bool,
    /// Short footer message and the tick at which it disappears.
    pub toast: Option<(String, u64)>,
    /// Tick at which the current turn started.
    pub t0: u64,
    /// Index in `bs` of the latest started tool block; the engines set it and nothing reads it.
    pub sel: Option<usize>,
    /// Session names of the mock engine; set in `App::new` and not read anywhere in `src`.
    pub ses: Vec<&'static str>,
    /// Index into `ses`; set in `App::new` and not read anywhere in `src`.
    pub si: usize,
    /// Where the text cursor was placed in the last drawn frame, if anywhere.
    pub cp: Cell<Option<(u16, u16)>>,
    /// Largest valid value of `scr`, set by the drawing code from the chat height.
    pub smax: Cell<usize>,
    /// Tick of the last `/clear`, which drives the logo animation.
    pub tc: u64,
    /// Tick of the last submitted prompt, which drives the send animation.
    pub ts: u64,
    /// Tick at which details were last toggled (Ctrl+O), which drives the unfold animation.
    pub to: u64,
    /// Tick of the last permission mode change, which drives the mode glyph animation.
    pub tm: u64,
    tf: bool,
    /// Display name of the current model, as reported by the engine.
    pub mname: String,
    /// Current effort level as text.
    pub effs: String,
    lv: Option<live::Lv>,
    cx: Option<codex::Cx>,
    /// Name of the active animated background, if any.
    pub bgw: Option<&'static str>,
    /// Whether the work account (token from the keyring) is in use.
    pub work: bool,
    /// `Some` when a work token exists in the keyring, so the work account can be switched to.
    pub wk: Option<()>,
    /// Rewind picker: selected user message and the chosen option (0 code and conversation, 1 conversation, 2 code), if open.
    pub rw: Option<(usize, Option<usize>)>,
    /// Suggested next prompt, shown as ghost text in an empty input box.
    pub sug: Option<String>,
    /// Highlighted row of the `/` command menu or the `@` file menu.
    pub ms: usize,
    /// Resume picker: the session list and the selected row, if open.
    pub rs: Option<(Vec<RewindItem>, usize)>,
    /// Filter text typed in the resume picker.
    pub rq: String,
    /// Background picker: selected row and the background to restore on Esc, if open.
    pub bp: Option<(usize, Option<&'static str>)>,
    /// Theme picker: selected theme and the theme to restore on Esc, if open.
    pub tp: Option<(usize, usize)>,
    /// Default permission picker: selected mode, if open.
    pub pp: Option<usize>,
    /// `/noname` settings menu: selected row, if open.
    pub ns: Option<usize>,
    /// Open question dialog, if any.
    pub qa: Option<Qa>,
    /// Receiver for the result of the running update check, if one is running.
    pub upr: Option<std::sync::mpsc::Receiver<upd::Up>>,
    /// Update status text shown in the settings menu.
    pub ups: String,
    /// Whether the running update check was started by the user, so the result is shown as a toast.
    pub upf: bool,
    /// Pending read of the free-token total, the last total, and when it was asked.
    pub frr: Option<std::sync::mpsc::Receiver<Option<u64>>>,
    /// Last free-token total.
    pub frv: Option<u64>,
    frat: Option<Instant>,
    /// The same three for the Claude token total.
    pub clr: Option<std::sync::mpsc::Receiver<Option<u64>>>,
    /// Last Claude token total.
    pub clv: Option<u64>,
    clat: Option<Instant>,
    le: u64,
    /// Tick of the last Ctrl+C press (0 = none): a second press within CC_WIN ticks exits.
    cc: u64,
    /// Attached images as (media type, bytes); `[image #N]` in the input refers to entry N.
    pub imgs: Vec<(&'static str, Vec<u8>)>,
    /// Pasted blocks folded into `[pasted #N]` tokens in the input; entry N holds the full text.
    pub pst: Vec<String>,
    /// Find bar (Ctrl+F): the search text and the index of the current match, if open.
    pub fs: Option<(String, usize)>,
    /// Chat size of the last frame as (total lines, visible lines).
    pub bv: Cell<(usize, usize)>,
    /// Whether the last turn has finished, shown as a check mark in the terminal title until the next key.
    pub dn: bool,
    und: Vec<(String, usize)>,
    red: Vec<(String, usize)>,
    uk: bool,
    unre: bool,
    drf: Option<String>,
    /// Input box geometry of the last frame as (top row, height, text width).
    pub ig: Cell<(u16, u16, usize)>,
    /// Background tasks (shells, monitors, agents, workflows).
    pub tasks: Vec<Task>,
    /// last text copied with a mouse selection; right-click pastes it when no clipboard tool is there
    pub clip: String,
    /// Task list view: selected task and whether its detail page is open, if the view is open.
    pub tk: Option<(usize, bool)>,
    tkp: u64,
    /// Open picker (model, effort, MCP servers or diff), if any.
    pub pk: Option<Pk>,
    /// File suggestions for the current `@` token.
    pub fsug: Vec<String>,
    /// The `@` query that `fsug` was built for.
    pub fq: Option<String>,
    fqt: u64,
    /// The `@` query whose menu was dismissed with Esc, so it stays hidden.
    pub fhide: Option<String>,
    /// Set when Ctrl+G asks to edit the input in an external editor.
    pub edit: bool,
    /// API retry notice and the tick until which it is shown, if a request is being retried.
    pub retry: Option<(String, u64)>,
    /// Running hooks as (id, event name, tick it started at).
    pub hooks: Vec<(String, String, u64)>,
    /// Session title.
    pub stitle: String,
    /// Models offered by the engine.
    pub models: Vec<ModelEntry>,
    /// Clickable regions of the last frame as (row, first column, end column, target), where the target is a URL or a file path.
    pub links: std::cell::RefCell<Vec<(u16, u16, u16, String)>>,
    /// Workspace diff as JSON from the engine, for the diff picker.
    pub diff: Option<serde_json::Value>,
    /// Mouse selection on the screen as (start column, start row, end column, end row).
    pub msel: Option<(u16, u16, u16, u16)>,
    /// Whether a mouse drag selection is in progress.
    pub seld: bool,
    /// Copy of the last drawn screen, used to read the selected text.
    pub txt: std::cell::RefCell<Option<ratatui::buffer::Buffer>>,
    /// Selection inside the chat text as two (line, column) points, anchor first.
    pub csel: Option<((usize, u16), (usize, u16))>,
    /// Chat text geometry of the last frame as (top row, visible lines, first visible line, left column).
    pub cg: Cell<(u16, usize, usize, u16)>,
    /// Plain text of every chat line of the last frame, for selecting and finding text.
    pub btx: std::cell::RefCell<Vec<String>>,
    /// Last mouse position as (column, row).
    pub my: (u16, u16),
    /// Last left click as (time, column, row, click count), to detect double and triple clicks.
    pub lc: (Instant, u16, u16, u8),
}

/// State of a list picker.
#[derive(Clone, Default)]
pub struct Pk {
    /// Picker kind: 0 model, 1 effort, 2 MCP servers, 3 diff file list, 4 diff text of one file.
    pub kind: u8,
    /// Title of the picker.
    pub title: String,
    /// Rows as (label, description, key).
    pub items: Vec<(String, String, String)>,
    /// Selected row.
    pub sel: usize,
    /// Scroll offset in the diff text view.
    pub off: usize,
    /// Diff text lines as (marker character, text).
    pub lines: Vec<(char, String)>,
}

/// A background task: shell, monitor, agent or workflow.
#[derive(Clone, Default)]
pub struct Task {
    /// Task id.
    pub id: String,
    /// Kind: 0 shell, 1 monitor, 2 agent, 3 workflow, 4 code agent; an index into `TKIND`.
    pub kind: u8,
    /// Description.
    pub desc: String,
    /// State: 0 running, 1 done, 2 failed, 3 stopped.
    pub st: u8,
    /// Tick at which the task started.
    pub t0: u64,
    /// Tick at which the task ended (0 while running).
    pub t1: u64,
    /// Id of the tool call that started the task.
    pub tool: String,
    /// Latest activity line.
    pub last: String,
    /// Number of tool calls the task has made.
    pub tools: u64,
    /// Number of tokens the task has used.
    pub toks: u64,
    /// Output text of the task.
    pub out: String,
    /// Recent activity lines, oldest first.
    pub acts: Vec<String>,
    /// workflow agents seen in workflow_progress: (index, 0 running/queued, 1 done, 2 error)
    pub wf: Vec<(u64, u8)>,
    /// phase of the newest running workflow agent
    pub phase: String,
    /// workflow name (deep-research, review-changes, ...), shown instead of the description
    pub name: String,
}

/// Colour of each task kind in the footer and the list under the prompt.
pub fn tcol(k: u8) -> ratatui::style::Color {
    [pal::CY(), pal::GR(), pal::LI(), pal::PK(), pal::LI()][k.min(4) as usize]
}

/// Task kinds as (symbol, singular name, plural name), indexed by `Task::kind`.
pub const TKIND: [(&str, &str, &str); 5] = [
    ("$", "shell", "shells"),
    ("~", "monitor", "monitors"),
    ("&", "agent", "agents"),
    ("%", "workflow", "workflows"),
    ("{", "code agent", "code agents"),
];

fn tl(k: Tk, a: &str, sum: &str, body: Vec<Ln>) -> Tool {
    Tool {
        k,
        nm: k.nm().into(),
        a: a.into(),
        sum: sum.into(),
        body,
        st: St::Ok,
        t0: 0,
        t1: 0,
    }
}
fn p(s: &str) -> Ln {
    Ln::P(s.into())
}

/// Sample Rust source that the mock engine shows when it reads `feed.rs`.
pub const FEED_RS: &str = "use std::time::Instant;

pub struct Feed {
    pub off: f32,
    pub tgt: f32,
    items: Vec<Card>,
    last: Instant,
}

impl Feed {
    #[must_use]
    pub fn new(items: Vec<Card>) -> Self {
        Self { off: 0.0, tgt: 0.0, items, last: Instant::now() }
    }

    pub fn next(&mut self) {
        self.tgt = (self.tgt + 1.0).min(self.items.len() as f32 - 1.0);
    }

    pub fn tick(&mut self, dt: f32) {
        let k = 1.0 - (-14.0 * dt).exp();
        self.off += (self.tgt - self.off) * k;
        self.clamp();
    }

    fn clamp(&mut self) {
        self.off = self.off.clamp(0.0, self.items.len() as f32);
    }
}";

fn edit1() -> Vec<Ln> {
    vec![
        Ln::C(40, "".into()),
        Ln::D(41, "    pub fn tick(&mut self) {".into()),
        Ln::D(42, "        self.off += 0.18;".into()),
        Ln::A(41, "    pub fn tick(&mut self, dt: f32) {".into()),
        Ln::A(42, "        let k = 1.0 - (-14.0 * dt).exp();".into()),
        Ln::A(43, "        self.off += (self.tgt - self.off) * k;".into()),
        Ln::C(44, "        self.clamp();".into()),
        Ln::C(45, "    }".into()),
    ]
}

fn hist0() -> Vec<B> {
    vec![
        B::U("scrolling in the feed stutters at 144hz, fix it".into(), "14:02".into()),
        B::Th("The feed probably moves by a fixed step per frame. At 144 Hz that gets faster and uneven because poll does not tick exactly. Checking feed.rs and the main loop.".into()),
        B::Tl(Tool { t1: 9, ..tl(Tk::Read, "src/feed.rs", "212 lines", vec![]) }),
        B::Tl(Tool { t1: 4, ..tl(Tk::Grep, "fn tick", "3 matches", vec![p("src/feed.rs:41    pub fn tick(&mut self) {"), p("src/app.rs:88        self.feed.tick();"), p("src/swipe.rs:17    pub fn tick(&mut self) {")]) }),
        B::T("Found it. `Feed::tick` moves the offset by a fixed amount per frame:\n\n```rust\nself.off += 0.18;\n```\n\nAt 144 Hz that runs 2.4x faster than at 60 Hz, and since `poll` returns irregularly, it jumps. Switching to **delta time**.".into()),
        B::Tl(Tool { t1: 12, ..tl(Tk::Edit, "src/feed.rs", "+3 -2", edit1()) }),
        B::Tl(Tool { t1: 254, ..tl(Tk::Bash, "cargo build --release", "ok", vec![p("   Compiling rusttok v0.4.1 (/home/you/projects/rusttok)"), p("    Finished `release` profile [optimized] target(s) in 8.42s")]) }),
        B::T("Done. Scrolling is now framerate independent and eases out instead of stopping hard. Try it with `cargo run --release`.".into()),
        B::Rc(rcfmt(12.4, 18_200, 0, 0.08, 0.0), 0),
    ]
}

fn script(n: usize) -> Vec<Sp> {
    match n % 3 {
        0 => vec![
            Sp::Th("The snap at the end of a swipe still uses the old linear motion. Reusing the scroll curve so both feel the same.".into()),
            Sp::Tx("Updating the snap in `swipe.rs`.".into()),
            Sp::Tl(tl(Tk::Read, "src/swipe.rs", "96 lines", vec![]), 22),
            Sp::Tl(tl(Tk::Edit, "src/swipe.rs", "+4 -3", vec![
                Ln::C(16, "".into()),
                Ln::D(17, "    pub fn tick(&mut self) {".into()),
                Ln::D(18, "        self.x += self.v;".into()),
                Ln::D(19, "        self.v *= 0.9;".into()),
                Ln::A(17, "    pub fn tick(&mut self, dt: f32) {".into()),
                Ln::A(18, "        let k = 1.0 - (-14.0 * dt).exp();".into()),
                Ln::A(19, "        self.x += (self.snap() - self.x) * k;".into()),
                Ln::A(20, "        self.v = 0.0;".into()),
                Ln::C(21, "    }".into()),
            ]), 34),
            Sp::Tl(Tool { st: St::Err, ..tl(Tk::Bash, "cargo test", "1 failed", vec![p("running 24 tests"), p("test swipe::snap_back ... FAILED"), p("  assertion failed: x == 0.0  (left: 0.004)"), p("test result: FAILED. 23 passed; 1 failed")]) }, 60),
            Sp::Tx("A test expects the snap to land exactly on 0. The curve only approaches it, so I add a threshold at the end.".into()),
            Sp::Tl(tl(Tk::Edit, "src/swipe.rs", "+3 -0", vec![
                Ln::C(20, "        self.v = 0.0;".into()),
                Ln::A(21, "        if (self.snap() - self.x).abs() < 0.01 {".into()),
                Ln::A(22, "            self.x = self.snap();".into()),
                Ln::A(23, "        }".into()),
            ]), 26),
            Sp::Tl(tl(Tk::Bash, "cargo test", "24 passed", vec![p("running 24 tests"), p("test feed::smooth ... ok"), p("test swipe::snap_back ... ok"), p("test result: ok. 24 passed; 0 failed; finished in 0.31s")]), 70),
            Sp::Todo(vec![(2, "feed.rs to delta time".into()), (2, "check build".into()), (2, "easing for swipe snap".into())]),
            Sp::Tx("Done. The snap now eases out with the same curve as scrolling, and all 24 tests pass.".into()),
            Sp::Rc(rcfmt(19.6, 31_500, 0, 0.14, 1.0)),
        ],
        1 => vec![
            Sp::Th("Committing both files, asking before the push since it leaves this machine.".into()),
            Sp::Tl(tl(Tk::Bash, "git add -A && git commit -m \"smooth scroll + snap\"", "1 commit", vec![p("[main 3f9c2e1] smooth scroll + snap"), p(" 2 files changed, 7 insertions(+), 5 deletions(-)")]), 26),
            Sp::Ask("git push origin main".into()),
        ],
        _ => vec![
            Sp::Th("There are several ways to make scrolling smoother. A subagent checks the render path while I look for approaches.".into()),
            Sp::Tl(tl(Tk::Task, "explore: where is the feed rendered", "4 files", vec![p("read src/ui.rs"), p("read src/feed.rs"), p("grep \"render_card\"  ->  2 matches"), p("read src/card.rs"), p("result: render in ui.rs:120, rows are whole cells")]), 60),
            Sp::Tl(tl(Tk::Web, "ratatui smooth scrolling sub-cell", "5 results", vec![p("ratatui.rs  ·  Rendering with half blocks"), p("github.com  ·  tachyonfx motion effects"), p("blog  ·  frame pacing in terminal apps")]), 30),
            Sp::Tx("Three ways to make scrolling even smoother:\n\n- **Sub-row rendering** with half blocks, twice as fine steps\n- **Prediction** of the next frame from swipe velocity\n- **Frame pacing** tied to the monitor refresh rate\n\nI would start with the first one, it makes the biggest visible difference.".into()),
            Sp::Rc(rcfmt(9.2, 22_700, 0, 0.09, 0.0)),
        ],
    }
}

const BTW: &str = "`dt` is the time since the last frame in seconds. About 0.0069 at 144 Hz and 0.0167 at 60 Hz, so motion per second stays the same no matter how often it draws.";

fn take(s: &mut String, n: usize) -> String {
    let i = s.char_indices().nth(n).map(|x| x.0).unwrap_or(s.len());
    let r = s[..i].to_string();
    s.drain(..i);
    r
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates the mock app: demo chat, scripted replies and default settings, used for demos and `--dump` screenshots.
    #[must_use]
    pub fn new() -> Self {
        App {
            bs: hist0(),
            inp: String::new(),
            cur: 0,
            scr: 0,
            open: false,
            q: VecDeque::new(),
            busy: false,
            t: 0,
            wait: 0,
            model: 0,
            eff: 2,
            mode: 0,
            ctx: 34.0,
            h5: 12.0,
            d7: 4.0,
            cwd: "~/projects/rusttok".into(),
            br: "main".into(),
            btw: None,
            todo: vec![
                (2, "feed.rs to delta time".into()),
                (2, "check build".into()),
                (0, "easing for swipe snap".into()),
            ],
            stodo: false,
            ask: None,
            ti: None,
            sb: None,
            n: 0,
            hist: vec![],
            hi: None,
            quit: false,
            toast: None,
            t0: 0,
            sel: None,
            ses: vec!["rusttok", "wargame", "hivemind"],
            si: 0,
            cp: Cell::new(None),
            smax: Cell::new(usize::MAX),
            tc: 0,
            ts: 0,
            to: 0,
            tm: 0,
            mname: MODELS[0].into(),
            effs: EFF[2].into(),
            lv: None,
            cx: None,
            bgw: None,
            work: false,
            wk: None,
            rw: None,
            sug: None,
            ms: 0,
            rs: None,
            rq: String::new(),
            bp: None,
            tp: None,
            pp: None,
            ns: None,
            qa: None,
            upr: None,
            ups: String::new(),
            upf: false,
            frr: None,
            frv: None,
            frat: None,
            clr: None,
            clv: None,
            clat: None,
            le: 0,
            cc: 0,
            imgs: vec![],
            pst: vec![],
            fs: None,
            bv: Cell::new((0, 0)),
            dn: false,
            und: vec![],
            red: vec![],
            uk: false,
            unre: false,
            drf: None,
            ig: Cell::new((0, 0, 0)),
            tasks: vec![],
            clip: String::new(),
            tk: None,
            tkp: 0,
            pk: None,
            fsug: vec![],
            fq: None,
            fqt: 0,
            fhide: None,
            edit: false,
            retry: None,
            hooks: vec![],
            stitle: String::new(),
            models: MID_ALL
                .iter()
                .map(|(v, d)| {
                    (
                        v.to_string(),
                        d.to_string(),
                        String::new(),
                        EFF.iter().map(|x| x.to_string()).collect(),
                    )
                })
                .collect(),
            links: std::cell::RefCell::new(vec![]),
            diff: None,
            msel: None,
            seld: false,
            txt: std::cell::RefCell::new(None),
            csel: None,
            cg: Cell::new((0, 0, 0, 0)),
            btx: std::cell::RefCell::new(vec![]),
            my: (0, 0),
            lc: (Instant::now(), u16::MAX, u16::MAX, 0),
            tf: false,
        }
    }

    /// Mock wall-clock time as `HH:MM`, derived from the tick counter.
    pub fn clock(&self) -> String {
        let m = 2 + 60 * 14 + (self.t / 1800) as u32;
        format!("{:02}:{:02}", m / 60, m % 60)
    }

    /// Seconds the tool `t` has run, or ran if it is finished, counted from ticks.
    pub fn secs(&self, t: &Tool) -> f32 {
        let e = if t.st == St::Run {
            self.t
        } else {
            t.t1.max(t.t0)
        };
        (e - t.t0) as f32 * 0.033
    }

    /// Seconds since the current turn started.
    pub fn turn_secs(&self) -> f32 {
        (self.t - self.t0) as f32 * 0.033
    }

    /// Index in `bs` of the block that is currently being streamed, if any.
    pub fn streaming(&self) -> Option<usize> {
        self.sb
    }

    /// Shows `s` in the footer for about two seconds.
    pub fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), self.t + 60));
    }

    /// Advances one tick: runs background polls, timers and update checks, then plays the next scripted step or pumps the live engine.
    pub fn step(&mut self) {
        self.t += 1;
        self.fpoll();
        if let Some((s, true)) = self.tk {
            if self.t >= self.tkp + 30 {
                self.tkp = self.t;
                self.tpoll(s);
            }
        }
        self.frpoll();
        self.clpoll();
        if let Some(r) = &self.upr {
            if let Ok(u) = r.try_recv() {
                self.upr = None;
                let f = self.upf;
                self.upf = false;
                match u {
                    upd::Up::Current => {
                        self.ups = "up to date".into();
                        if f {
                            self.toast("noname is up to date");
                        }
                    }
                    upd::Up::Ready(r) => {
                        self.ups = format!("{r} installed, restart to use");
                        self.bs.push(B::Sys(format!(
                            "noname updated to {r}  ·  restart to use it"
                        )));
                    }
                    upd::Up::Skip(m) | upd::Up::Fail(m) => {
                        if f {
                            self.toast(format!("update  {m}"));
                        }
                        self.ups = m;
                    }
                }
            }
        }
        if let Some(b) = &mut self.btw {
            if !b.done() {
                let mut r = b.full[b.a.len()..].to_string();
                let c = take(&mut r, 3);
                b.a.push_str(&c);
            }
        }
        if let Some((_, t)) = &self.toast {
            if self.t > *t {
                self.toast = None;
            }
        }
        if self.lv.is_some() {
            self.pump();
            return;
        }
        if self.cx.is_some() {
            self.cpump();
            return;
        }
        if self.ask.is_some() {
            return;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        let Some(sp) = self.q.pop_front() else {
            self.busy = false;
            return;
        };
        match sp {
            Sp::Th(mut s) => {
                let c = take(&mut s, 5);
                self.stream(true, c);
                if s.is_empty() {
                    self.sb = None;
                    self.wait = 8;
                } else {
                    self.q.push_front(Sp::Th(s));
                }
            }
            Sp::Tx(mut s) => {
                let c = take(&mut s, 3);
                self.stream(false, c);
                if s.is_empty() {
                    self.sb = None;
                    self.wait = 5;
                } else {
                    self.q.push_front(Sp::Tx(s));
                }
            }
            Sp::Tl(mut t, d) => {
                self.tf = t.st == St::Err;
                t.t0 = self.t;
                t.st = St::Run;
                self.bs.push(B::Tl(t));
                self.ti = Some(self.bs.len() - 1);
                self.sel = self.ti;
                self.wait = d;
                self.q.push_front(Sp::Fin);
            }
            Sp::Fin => {
                if let Some(i) = self.ti.take() {
                    if let B::Tl(t) = &mut self.bs[i] {
                        t.st = if self.tf { St::Err } else { St::Ok };
                        t.t1 = self.t;
                    }
                }
                self.tf = false;
                self.wait = 6;
                self.ctx = (self.ctx + 1.5).min(99.0);
            }
            Sp::Ask(c) => {
                self.bs.push(B::Ask(Ask { cmd: c, ans: None }));
                self.ask = Some(self.bs.len() - 1);
            }
            Sp::Todo(v) => self.todo = v,
            Sp::Rc(x) => {
                self.bs.push(B::Rc(x, self.t.max(1)));
                if self.q.is_empty() && self.inp.is_empty() {
                    self.sug = Some(
                        [
                            "commit and push this",
                            "can it get smoother?",
                            "run the tests again",
                        ][self.n % 3]
                            .into(),
                    );
                }
            }
        }
    }

    fn stream(&mut self, th: bool, c: String) {
        match self.sb {
            Some(i) => {
                if let B::Th(s) | B::T(s) = &mut self.bs[i] {
                    s.push_str(&c)
                }
            }
            None => {
                self.bs.push(if th { B::Th(c) } else { B::T(c) });
                self.sb = Some(self.bs.len() - 1);
            }
        }
        self.ctx = (self.ctx + 0.02).min(99.0);
        self.h5 = (self.h5 + 0.005).min(99.0);
    }

    /// Answers the pending permission question with `y` (yes), `n` (no) or `a` (yes and always).
    pub fn answer(&mut self, c: char) {
        if self.lv.is_some() {
            self.lanswer(c);
            return;
        }
        let Some(i) = self.ask.take() else { return };
        if let B::Ask(a) = &mut self.bs[i] {
            a.ans = Some(c);
        }
        if c == 'n' {
            self.q.push_front(Sp::Rc(rcfmt(4.1, 9_800, 0, 0.03, 0.0)));
            self.q.push_front(Sp::Tx(
                "Okay, not pushed. The commit stays local on `main`.".into(),
            ));
        } else {
            self.q.push_front(Sp::Rc(rcfmt(6.8, 12_400, 0, 0.04, 0.0)));
            self.q.push_front(Sp::Tx("Pushed to `origin/main`.".into()));
            self.q.push_front(Sp::Tl(
                tl(
                    Tk::Bash,
                    "git push origin main",
                    "pushed",
                    vec![
                        p("To github.com:netter/rusttok.git"),
                        p("   a81d0c4..3f9c2e1  main -> main"),
                    ],
                ),
                40,
            ));
        }
    }

    /// Interrupts the running turn and clears what was queued.
    pub fn stop(&mut self) {
        if self.cx.is_some() {
            self.cstop();
            return;
        }
        if self.lv.is_some() {
            self.lstop();
            return;
        }
        self.q.clear();
        self.wait = 0;
        if let Some(i) = self.ti.take() {
            if let B::Tl(t) = &mut self.bs[i] {
                t.st = St::Err;
                t.t1 = self.t;
                t.sum = "interrupted".into();
            }
        }
        if let Some(i) = self.ask.take() {
            if let B::Ask(a) = &mut self.bs[i] {
                a.ans = Some('n');
            }
        }
        self.sb = None;
        self.busy = false;
        self.bs.push(B::Sys("interrupted".into()));
    }

    /// Sends the input box: a background name, a `/` command or a prompt for the current engine.
    pub fn submit(&mut self) {
        let s = self.inp.trim().to_string();
        if s.is_empty() {
            return;
        }
        self.inp.clear();
        self.cur = 0;
        self.scr = 0;
        self.sug = None;
        self.ts = self.t.max(1);
        self.hist.push(s.clone());
        self.hi = None;
        if let Some(k) = bg::canon(&s) {
            self.setbg(Some(k));
            return;
        }
        if let Some(r) = s.strip_prefix('/') {
            self.cmd(r);
            return;
        }
        if self.lv.is_some() {
            self.lsubmit(s);
            return;
        }
        if self.cx.is_some() {
            self.csubmit(s);
            return;
        }
        if !self.busy {
            self.t0 = self.t;
        }
        self.bs.push(B::U(s, self.clock()));
        self.busy = true;
        self.sb = None;
        let sc = script(self.n);
        self.n += 1;
        self.q.extend(sc);
    }

    /// Selects the animated background `k` (`None` turns it off) and saves the choice.
    /// Says so in the chat if the background is not installed.
    pub fn setbg(&mut self, k: Option<&'static str>) {
        match k {
            Some(k) if !bg::exists(k) => {
                self.bs
                    .push(B::Sys(format!("{k} is not installed  ·  {}", bg::hint(k))));
                return;
            }
            Some(k) => self.toast(format!("background  {k}")),
            None => self.toast("background  off"),
        }
        self.bgw = k;
        if self.lv.is_some() || self.cx.is_some() {
            let f = bgfile();
            if let Some(d) = std::path::Path::new(&f).parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&f, k.unwrap_or("off"));
        }
    }

    fn qkey(&mut self, k: KeyEvent) -> bool {
        let Some(q) = &mut self.qa else { return false };
        let cur = &q.qs[q.i];
        let n = cur.opts.len();
        let other = q.sel == n;
        match k.code {
            KeyCode::Esc => {
                self.qdone(false);
                return true;
            }
            KeyCode::Up => q.sel = q.sel.saturating_sub(1),
            KeyCode::Down => q.sel = (q.sel + 1).min(n),
            KeyCode::Char(c)
                if !other && c.is_ascii_digit() && c != '0' && (c as usize - '1' as usize) < n =>
            {
                let j = c as usize - '1' as usize;
                q.sel = j;
                if cur.multi {
                    q.pick[j] = !q.pick[j];
                } else {
                    self.qnext(cur_label(&self.qa, j));
                }
            }
            KeyCode::Char(' ') if !other && cur.multi => q.pick[q.sel] = !q.pick[q.sel],
            KeyCode::Enter if other => {
                let t = self.inp.trim().to_string();
                if t.is_empty() {
                    return true;
                }
                self.inp.clear();
                self.cur = 0;
                let mut v: Vec<String> = if cur.multi {
                    cur.opts
                        .iter()
                        .zip(&q.pick)
                        .filter(|x| *x.1)
                        .map(|x| x.0.0.clone())
                        .collect()
                } else {
                    vec![]
                };
                v.push(t);
                self.qnext(v.join(", "));
            }
            KeyCode::Enter => {
                let a = if cur.multi {
                    let v: Vec<String> = cur
                        .opts
                        .iter()
                        .zip(&q.pick)
                        .filter(|x| *x.1)
                        .map(|x| x.0.0.clone())
                        .collect();
                    if v.is_empty() {
                        cur.opts[q.sel].0.clone()
                    } else {
                        v.join(", ")
                    }
                } else {
                    cur.opts[q.sel].0.clone()
                };
                self.qnext(a);
            }
            _ => return !other,
        }
        true
    }

    fn qnext(&mut self, a: String) {
        let Some(q) = &mut self.qa else { return };
        let qt = q.qs[q.i].q.clone();
        q.ans.push((qt, a));
        q.i += 1;
        if q.i >= q.qs.len() {
            self.qdone(true);
            return;
        }
        q.sel = 0;
        q.pick = vec![false; q.qs[q.i].opts.len()];
    }

    fn qdone(&mut self, ok: bool) {
        let Some(q) = self.qa.take() else { return };
        if ok {
            for (a, b) in &q.ans {
                self.bs.push(B::Qa(a.clone(), b.clone()));
            }
        } else {
            self.bs.push(B::Sys("question dismissed".into()));
        }
        self.lqa(q, ok);
    }

    /// Keeps the free-token total fresh while the option is on; one background read at a time.
    fn frpoll(&mut self) {
        if let Some(r) = &self.frr {
            if let Ok(v) = r.try_recv() {
                self.frr = None;
                if v.is_some() {
                    self.frv = v;
                }
            }
            return;
        }
        if !free::enabled() {
            self.frv = None;
            self.frat = None;
            return;
        }
        if self.frat.is_none_or(|t| t.elapsed() >= free::EVERY) {
            self.frat = Some(Instant::now());
            self.frr = Some(free::fetch());
        }
    }

    /// Same for the Claude token total, read once a minute while its option is on.
    fn clpoll(&mut self) {
        if let Some(r) = &self.clr {
            if let Ok(v) = r.try_recv() {
                self.clr = None;
                if v.is_some() {
                    self.clv = v;
                }
            }
            return;
        }
        if !free::claude_enabled() {
            self.clv = None;
            self.clat = None;
            return;
        }
        if self.clat.is_none_or(|t| t.elapsed() >= free::CLAUDE_EVERY) {
            self.clat = Some(Instant::now());
            self.clr = Some(free::fetch_claude());
        }
    }

    /// Starts an update check in the background unless one is running; `force` means the user asked for it.
    pub fn upcheck(&mut self, force: bool) {
        if self.upr.is_some() {
            return;
        }
        self.upf = force;
        self.ups = "checking".into();
        self.upr = Some(upd::check(force));
    }

    /// Rows of the `/noname` settings menu as (label, current value).
    pub fn nrows(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "auto-update",
                match upd::managed() {
                    Some(m) => format!("via {m}"),
                    None => if upd::enabled() { "on" } else { "off" }.into(),
                },
            ),
            ("theme", pal::all()[pal::cur()].name.into()),
            ("default permission", MODES[defmode().unwrap_or(0)].into()),
            ("default effort", defeff().unwrap_or("claude").into()),
            ("background", self.bgw.unwrap_or("off").into()),
            ("context bar", if bars()[0] { "on" } else { "off" }.into()),
            ("5h bar", if bars()[1] { "on" } else { "off" }.into()),
            ("week bar", if bars()[2] { "on" } else { "off" }.into()),
            ("input", LAYOUT[0].1[lay(0)].into()),
            ("usage", LAYOUT[1].1[lay(1)].into()),
            ("model", LAYOUT[2].1[lay(2)].into()),
            ("title", LAYOUT[3].1[lay(3)].into()),
            ("title visibility", LAYOUT[4].1[lay(4)].into()),
            ("location", LAYOUT[5].1[lay(5)].into()),
            (
                "right-click paste",
                if rclick() { "on" } else { "off" }.into(),
            ),
            ("opacity", format!("{}%", opac())),
            (
                "free token count",
                if free::enabled() {
                    "on, middle of the footer"
                } else {
                    "off"
                }
                .into(),
            ),
            (
                "claude token count",
                if free::claude_enabled() {
                    "on, after the free total"
                } else {
                    "off"
                }
                .into(),
            ),
            (
                "check for updates",
                if self.ups.is_empty() {
                    format!("{}  {}", upd::short(), upd::DATE)
                } else {
                    format!("{}  ·  {}", upd::short(), self.ups)
                },
            ),
        ]
    }

    /// Steps the default effort setting to the next (`up`) or previous level; the first step means Claude's own default.
    pub fn cycleff(&mut self, up: bool) {
        let n = EFF.len() + 1;
        let i = defeff()
            .and_then(|e| EFF.iter().position(|x| *x == e))
            .map_or(0, |i| i + 1);
        let j = if up { (i + 1) % n } else { (i + n - 1) % n };
        setdefeff(j.checked_sub(1).map(|k| EFF[k]));
        self.toast(format!(
            "default effort  {}  (next session)",
            defeff().unwrap_or("claude")
        ));
    }

    /// Sets the default permission mode to index `i` of `MODES`, saves it and applies it to the session.
    pub fn setdefmode(&mut self, i: usize) {
        if self.lv.is_some() || self.cx.is_some() {
            let f = modefile();
            if let Some(d) = std::path::Path::new(&f).parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&f, MODES[i]);
        }
        if self.mode != i {
            self.mode = i;
            self.tm = self.t.max(1);
            self.lmode();
        }
        self.toast(format!("default permission  {}", MODES[i]));
    }

    /// One step more see-through (`less`) or more solid; 100% gives the terminal its own value back.
    pub fn setopac(&mut self, less: bool) {
        self.opacstep(less, true);
    }

    /// One opacity step; `wrap` goes from 30% round to 100% (menu), without it the ends hold (alt + wheel).
    pub fn opacstep(&mut self, less: bool, wrap: bool) {
        let n = OPAC.len();
        let cur = OPAC.iter().position(|x| *x == opac()).unwrap_or(0);
        let i = match (less, wrap) {
            (true, true) => (cur + 1) % n,
            (false, true) => (cur + n - 1) % n,
            (true, false) => (cur + 1).min(n - 1),
            (false, false) => cur.saturating_sub(1),
        };
        if i == cur {
            self.toast(format!("opacity  {}%", OPAC[i]));
            return;
        }
        let v = OPAC[i];
        let ok = if std::env::var("NONAME_MOCK").is_ok() {
            true
        } else if v == 100 {
            sys::opacity_restore();
            true
        } else {
            sys::opacity_apply(v)
        };
        if !ok {
            self.toast("opacity works in kitty, Alacritty, Ptyxis and GNOME Terminal; set it in your terminal's own settings");
            return;
        }
        let f = sys::cfgfile("opacity");
        if let Some(d) = std::path::Path::new(&f).parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&f, v.to_string());
        let sc = sys::opacity_scope();
        self.toast(if v == 100 || sc.is_empty() {
            format!("opacity  {v}%")
        } else {
            format!("opacity  {v}%  ({sc})")
        });
    }

    /// Activates theme `i` and saves the choice when a real engine is running.
    pub fn settheme(&mut self, i: usize) {
        pal::set(i);
        self.toast(format!("theme  {}", pal::all()[pal::cur()].name));
        if self.lv.is_some() || self.cx.is_some() {
            pal::save();
        }
    }

    /// Runs the slash command `r`, given without the leading `/`.
    pub fn cmd(&mut self, r: &str) {
        let (c0, a0) = r.split_once(' ').unwrap_or((r, ""));
        if c0 == "noname" {
            self.ns = Some(0);
            return;
        }
        if (c0 == "model" || c0 == "effort")
            && a0.trim().is_empty()
            && !self.is_codex()
            && self.lv.is_none()
        {
            if c0 == "model" {
                self.modelpicker()
            } else {
                self.effortpicker()
            }
            return;
        }
        if c0 == "tasks" || c0 == "bashes" {
            if self.tasks.is_empty() {
                self.toast("no tasks");
            } else {
                self.tk = Some((self.tasks.len() - 1, false));
            }
            return;
        }
        if c0 == "default-permission" {
            let a0 = a0.trim();
            if a0.is_empty() {
                self.pp = Some(defmode().unwrap_or(0));
            } else if let Some(i) = modeidx(a0) {
                self.setdefmode(i);
            } else {
                self.toast(format!("unknown mode  {a0}"));
            }
            return;
        }
        if c0 == "theme" {
            let a0 = a0.trim();
            if a0.is_empty() {
                self.tp = Some((pal::cur(), pal::cur()));
            } else if let Some(i) = pal::find(a0) {
                self.settheme(i);
            } else {
                self.toast(format!("unknown theme  {a0}"));
            }
            return;
        }
        if c0 == "background" || c0 == "bg" {
            let a0 = a0.trim();
            if a0.is_empty() {
                let v = bg::avail();
                let i = v.iter().position(|k| Some(*k) == self.bgw).unwrap_or(0);
                self.bp = Some((i, self.bgw));
            } else if a0 == "off" {
                self.setbg(None);
            } else if let Some(k) = bg::canon(a0) {
                self.setbg(Some(k));
            } else {
                self.toast(format!("unknown background  {a0}"));
            }
            return;
        }
        if c0 == "copy" {
            self.copylast(a0.trim() == "code");
            return;
        }
        if self.lv.is_some() {
            self.lcmd(r);
            return;
        }
        if self.cx.is_some() {
            self.ccmd(r);
            return;
        }
        let (c, arg) = r.split_once(' ').unwrap_or((r, ""));
        match c {
            "btw" => {
                let q = if arg.is_empty() { "what is dt exactly?" } else { arg };
                self.btw = Some(Btw { q: q.into(), a: String::new(), full: BTW.into(), fin: true })
            }
            "clear" => {
                self.tc = self.t;
                self.bs.clear();
                self.ctx = 3.0;
                self.todo.clear();
                self.sel = None;
            }
            "model" => {
                self.model = (self.model + 1) % MODELS.len();
                self.toast(format!("model  {}", MODELS[self.model]))
            }
            "effort" => {
                self.eff = (self.eff + 1) % EFF.len();
                self.toast(format!("effort  {}", EFF[self.eff]))
            }
            "compact" => {
                let o = self.ctx;
                self.ctx = 9.0;
                self.bs.push(B::Sys(format!("compacted  ctx {:.0}% -> 9%", o)))
            }
            "context" => self.bs.push(B::Sys(format!("context  {:.0}% of 1M used  ·  system 8%  ·  tools 6%  ·  messages {:.0}%", self.ctx, self.ctx - 14.0))),
            "usage" => self.bs.push(B::Sys(format!("usage  5h {:.0}%  ·  7d {:.0}%  ·  opus 7d 2%", self.h5, self.d7))),
            "resume" => self.bs.push(B::Sys("resume  (mock) 12 older sessions in ~/projects/rusttok".into())),
            "help" => self.bs.push(B::Sys("enter send  shift+enter newline  esc stop  ctrl+o details or unfold paste  ctrl+z undo  ctrl+y redo  ctrl+f find  alt+up/down jump between prompts  ctrl+t todos  shift+tab mode  /copy [code]".into())),
            _ => self.bs.push(B::Sys(format!("unknown command  /{c}"))),
        }
    }

    fn bi(&self) -> usize {
        self.inp
            .char_indices()
            .nth(self.cur)
            .map(|x| x.0)
            .unwrap_or(self.inp.len())
    }

    /// Inserts `c` at the cursor.
    pub fn ins(&mut self, c: char) {
        self.sug = None;
        self.ms = 0;
        let i = self.bi();
        self.inp.insert(i, c);
        self.cur += 1;
    }

    fn nch(&self) -> usize {
        self.inp.chars().count()
    }

    fn tokl(&self) -> Option<usize> {
        let cs: Vec<char> = self.inp.chars().take(self.cur).collect();
        tokend(&cs)
    }

    fn wl(&self) -> usize {
        if let Some(o) = self.tokl() {
            return o;
        }
        let cs: Vec<char> = self.inp.chars().collect();
        let mut j = self.cur;
        while j > 0 && cs[j - 1].is_whitespace() {
            j -= 1
        }
        if let Some(o) = tokend(&cs[..j]) {
            return o;
        }
        let w = |c: char| c.is_alphanumeric() || c == '_';
        if j > 0 && w(cs[j - 1]) {
            while j > 0 && w(cs[j - 1]) {
                j -= 1
            }
        } else {
            while j > 0 && !w(cs[j - 1]) && !cs[j - 1].is_whitespace() {
                j -= 1
            }
        }
        j
    }

    fn wr(&self) -> usize {
        let cs: Vec<char> = self.inp.chars().collect();
        let n = cs.len();
        let mut j = self.cur;
        while j < n && cs[j].is_whitespace() {
            j += 1
        }
        let rest: String = cs[j..].iter().collect();
        if let Some((_, l)) = atok(&rest) {
            return j + l;
        }
        let w = |c: char| c.is_alphanumeric() || c == '_';
        if j < n && w(cs[j]) {
            while j < n && w(cs[j]) {
                j += 1
            }
        } else {
            while j < n && !w(cs[j]) && !cs[j].is_whitespace() {
                j += 1
            }
        }
        j
    }

    fn cut(&mut self, a: usize, b: usize) {
        let (a, b) = (a.min(b), a.max(b));
        let cs: Vec<char> = self.inp.chars().collect();
        self.inp = cs[..a].iter().chain(cs[b..].iter()).collect();
        self.cur = a;
        self.sug = None;
        self.ms = 0;
    }

    /// Inserts the text `t` at the cursor.
    pub fn put(&mut self, t: &str) {
        for c in t.chars() {
            self.ins(c)
        }
    }

    /// The `@` token that ends at the cursor, as (character index of the `@`, text after it).
    pub fn attok(&self) -> Option<(usize, String)> {
        let cs: Vec<char> = self.inp.chars().collect();
        let mut j = self.cur.min(cs.len());
        while j > 0 && !cs[j - 1].is_whitespace() {
            j -= 1;
        }
        if cs.get(j) != Some(&'@') {
            return None;
        }
        let end = (j..cs.len())
            .find(|&i| cs[i].is_whitespace())
            .unwrap_or(cs.len());
        if end != self.cur {
            return None;
        }
        Some((j, cs[j + 1..end].iter().collect()))
    }

    fn fpoll(&mut self) {
        let Some(q) = self.attok().map(|x| x.1) else {
            if self.fq.is_some() {
                self.fq = None;
                self.fsug.clear();
            }
            self.fhide = None;
            return;
        };
        if self.fq.as_ref() == Some(&q) || self.t < self.fqt + 2 {
            return;
        }
        self.fqt = self.t;
        self.fq = Some(q.clone());
        if !self.fsuggest(&q) {
            self.fsug = localfiles(&q);
            self.ms = 0;
        }
    }

    /// File suggestions for the `@` token at the cursor, or none when the menu is hidden.
    pub fn atmenu(&self) -> Vec<String> {
        match self.attok() {
            Some((_, q)) if self.fhide.as_deref() != Some(q.as_str()) => self.fsug.clone(),
            _ => vec![],
        }
    }

    /// Replaces the `@` token at the cursor with the path `p`.
    pub fn atpick(&mut self, p: &str) {
        let Some((j, _)) = self.attok() else { return };
        let cs: Vec<char> = self.inp.chars().collect();
        let dir = p.ends_with('/') || p.ends_with('\\');
        let ins = format!(
            "@{}{}",
            if p.contains(' ') {
                format!("\"{p}\"")
            } else {
                p.to_string()
            },
            if dir { "" } else { " " }
        );
        let n = ins.chars().count();
        self.inp =
            cs[..j].iter().collect::<String>() + &ins + &cs[self.cur..].iter().collect::<String>();
        self.cur = j + n;
        self.ms = 0;
        self.fq = None;
    }

    /// Maps a screen position to (chat line, column) when it lies on the chat text.
    pub fn cpos(&self, x: u16, y: u16) -> Option<(usize, u16)> {
        let (yt, n, s0, x0) = self.cg.get();
        (n > 0 && y >= yt && ((y - yt) as usize) < n)
            .then(|| (s0 + (y - yt) as usize, x.saturating_sub(x0)))
    }

    /// Like `cpos`, but a position outside the chat text snaps to its nearest edge.
    pub fn cclamp(&self, x: u16, y: u16) -> (usize, u16) {
        let (yt, n, s0, _) = self.cg.get();
        if y < yt {
            (s0, 0)
        } else if (y - yt) as usize >= n {
            (s0 + n.saturating_sub(1), u16::MAX)
        } else {
            self.cpos(x, y).unwrap_or((s0, 0))
        }
    }

    /// Start and end (line, column) of the word at chat position `p`.
    pub fn word(&self, p: (usize, u16)) -> Option<((usize, u16), (usize, u16))> {
        let bt = self.btx.borrow();
        let l = bt.get(p.0)?;
        let mut cs: Vec<(u16, u16, char)> = vec![];
        let mut x = 0u16;
        for c in l.chars() {
            let w = c.width().unwrap_or(0) as u16;
            cs.push((x, x + w.max(1) - 1, c));
            x += w;
        }
        let i = cs.iter().position(|c| p.1 >= c.0 && p.1 <= c.1)?;
        let wc = |c: char| !c.is_whitespace() && !"()[]{}<>\"'`,;|".contains(c);
        if !wc(cs[i].2) {
            return Some(((p.0, cs[i].0), (p.0, cs[i].1)));
        }
        let mut a = i;
        while a > 0 && wc(cs[a - 1].2) {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < cs.len() && wc(cs[b + 1].2) {
            b += 1;
        }
        Some(((p.0, cs[a].0), (p.0, cs[b].1)))
    }

    /// Screen rectangle (x0, y0, x1, y1) of the chat selection, cut to the visible lines.
    pub fn cvis(&self) -> Option<(u16, u16, u16, u16)> {
        let (a, b) = self.csel?;
        let (p0, p1) = if a <= b { (a, b) } else { (b, a) };
        let (yt, n, s0, x0) = self.cg.get();
        if n == 0 || p1.0 < s0 || p0.0 >= s0 + n {
            return None;
        }
        let st = if p0.0 < s0 {
            (0, yt)
        } else {
            (x0.saturating_add(p0.1), yt + (p0.0 - s0) as u16)
        };
        let en = if p1.0 >= s0 + n {
            (u16::MAX, yt + n as u16 - 1)
        } else {
            (x0.saturating_add(p1.1), yt + (p1.0 - s0) as u16)
        };
        Some((st.0, st.1, en.0, en.1))
    }

    /// Text of the chat selection, one entry per line.
    pub fn ctext(&self) -> String {
        let Some((a, b)) = self.csel else {
            return String::new();
        };
        let (p0, p1) = if a <= b { (a, b) } else { (b, a) };
        let bt = self.btx.borrow();
        let mut ls = vec![];
        for i in p0.0..=p1.0.min(bt.len().saturating_sub(1)) {
            let (xa, xb) = (
                if i == p0.0 { p0.1 } else { 0 },
                if i == p1.0 { p1.1 } else { u16::MAX },
            );
            let mut l = String::new();
            let mut x = 0u16;
            for c in bt[i].chars() {
                let w = c.width().unwrap_or(0) as u16;
                if x >= xa && x <= xb {
                    l.push(c);
                }
                x += w;
            }
            ls.push(l.trim_end().to_string());
        }
        tidy(ls)
    }

    /// Handles a click at a screen cell: opens the link there as a URL, file or folder.
    pub fn click(&mut self, x: u16, y: u16) {
        let hit = self
            .links
            .borrow()
            .iter()
            .find(|l| l.0 == y && x >= l.1 && x < l.2)
            .map(|l| l.3.clone());
        if let Some(p) = hit {
            if p.starts_with("https://") || p.starts_with("http://") {
                sys::open_url(&p);
                self.toast("open link");
                return;
            }
            let (path, line) = sys::split_line(&p);
            let abs = sys::resolve(path);
            let name = path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|x| !x.is_empty())
                .unwrap_or(path)
                .to_string();
            if abs.is_dir() {
                sys::open_dir(&abs.to_string_lossy());
                self.toast(format!("folder  {name}"));
            } else {
                sys::open(&abs.to_string_lossy(), line);
                self.toast(format!("open  {name}"));
            }
        }
    }

    fn pkkey(&mut self, k: KeyEvent) {
        let Some(p) = &mut self.pk else { return };
        let n = p.items.len();
        if p.kind == 4 {
            let h = p.lines.len();
            match k.code {
                KeyCode::Esc | KeyCode::Left => {
                    p.kind = 3;
                    p.lines.clear();
                    p.off = 0;
                }
                KeyCode::Up => p.off = p.off.saturating_sub(1),
                KeyCode::Down => p.off = (p.off + 1).min(h.saturating_sub(1)),
                KeyCode::PageUp => p.off = p.off.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    p.off = (p.off + 10).min(h.saturating_sub(1))
                }
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Esc => self.pk = None,
            KeyCode::Up => p.sel = p.sel.saturating_sub(1),
            KeyCode::Down => p.sel = (p.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right if n > 0 => {
                let (kind, key) = (p.kind, p.items[p.sel].2.clone());
                self.pkpick(kind, key, false);
            }
            KeyCode::Char(' ') | KeyCode::Char('d') if n > 0 && p.kind == 2 => {
                let key = p.items[p.sel].2.clone();
                self.pkpick(2, key, true);
            }
            _ => {}
        }
    }

    fn pkpick(&mut self, kind: u8, key: String, alt: bool) {
        match kind {
            0 => {
                self.pk = None;
                self.setmodel(&key);
            }
            1 => {
                self.pk = None;
                self.seteffort(&key);
            }
            2 => self.mcpact(&key, alt),
            3 => {
                let lines = difflines(self.diff.as_ref(), &key);
                if let Some(p) = &mut self.pk {
                    p.kind = 4;
                    p.lines = lines;
                    p.off = 0;
                }
            }
            _ => {}
        }
    }

    /// Opens the model picker.
    pub fn modelpicker(&mut self) {
        let cur = MID_ALL.get(self.model).map(|x| x.0).unwrap_or("");
        let items: Vec<(String, String, String)> = self
            .models
            .iter()
            .map(|(v, d, ds, _)| (d.clone(), one_line(ds, 60), v.clone()))
            .collect();
        let sel = self
            .models
            .iter()
            .position(|m| m.0 == cur || m.0.starts_with(cur) || cur.starts_with(&m.0))
            .unwrap_or(0);
        self.pk = Some(Pk {
            kind: 0,
            title: "model".into(),
            items,
            sel,
            ..Default::default()
        });
    }

    /// Opens the effort picker.
    pub fn effortpicker(&mut self) {
        let cur = MID_ALL.get(self.model).map(|x| x.0).unwrap_or("");
        let lv: Vec<String> = self
            .models
            .iter()
            .find(|m| m.0 == cur)
            .map(|m| m.3.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| EFF.iter().map(|x| x.to_string()).collect());
        let sel = lv.iter().position(|x| *x == self.effs).unwrap_or(0);
        let items = lv
            .iter()
            .map(|x| (x.clone(), String::new(), x.clone()))
            .collect();
        self.pk = Some(Pk {
            kind: 1,
            title: "effort".into(),
            items,
            sel,
            ..Default::default()
        });
    }

    /// Attaches image bytes `b` and inserts an `[image #N]` token; returns `false` with a toast when they are not an image or are too large.
    pub fn attach(&mut self, b: Vec<u8>) -> bool {
        match img::shrink(b) {
            Ok((b, m)) => {
                self.imgs.push((m, b));
                let pre = self
                    .inp
                    .chars()
                    .nth(self.cur.wrapping_sub(1))
                    .is_some_and(|c| !c.is_whitespace());
                let t = format!(
                    "{}[image #{}] ",
                    if pre { " " } else { "" },
                    self.imgs.len()
                );
                self.put(&t);
                true
            }
            Err(0) => {
                self.toast("not an image");
                false
            }
            Err(n) => {
                self.toast(format!("image too large  {:.1} MB", n as f32 / 1e6));
                false
            }
        }
    }

    fn files(&mut self, v: Vec<std::path::PathBuf>) {
        for p in v {
            match std::fs::read(&p) {
                Ok(b) => {
                    self.attach(b);
                }
                Err(_) => self.toast(format!("cannot read {}", p.display())),
            }
        }
    }

    /// Handles pasted text: image files are attached, long text is folded into a `[pasted #N]` token, short text is typed.
    pub fn paste(&mut self, s: String) {
        if self.rs.is_none() {
            self.snap((self.inp.clone(), self.cur));
            self.red.clear();
            self.uk = false;
        }
        let v = img::paths(&s);
        if !v.is_empty() {
            self.files(v);
            return;
        }
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.rs.is_some() {
            self.rq.push_str(s.lines().next().unwrap_or(""));
            return;
        }
        let n = s.trim_end().lines().count();
        let c = s.chars().count();
        if n > 4 || c > 800 {
            self.pst.push(s);
            let pre = self
                .inp
                .chars()
                .nth(self.cur.wrapping_sub(1))
                .is_some_and(|c| !c.is_whitespace());
            let d = if n > 1 {
                format!("+{n} lines")
            } else {
                format!("{c} chars")
            };
            let t = format!(
                "{}[pasted #{} {d}] ",
                if pre { " " } else { "" },
                self.pst.len()
            );
            self.put(&t);
            return;
        }
        self.put(&s);
    }

    fn clipv(&mut self) {
        self.clipped(img::clip())
    }

    /// Right-click paste: the clipboard including text on every system.
    fn clipv_any(&mut self) {
        self.clipped(img::clip_any())
    }

    fn clipped(&mut self, c: img::Clip) {
        match c {
            img::Clip::Img(b) => {
                self.attach(b);
            }
            img::Clip::Files(v) => self.files(v),
            img::Clip::Text(t) => self.paste(t),
            img::Clip::Empty if !self.clip.is_empty() => self.paste(self.clip.clone()),
            img::Clip::Empty => self.toast("clipboard is empty"),
        }
    }

    /// Replaces every `[pasted #N]` token in `s` with the text it stands for.
    pub fn expand(&self, s: &str) -> String {
        let mut o = String::new();
        let mut r = s;
        while let Some(i) = r.find("[pasted #") {
            o.push_str(&r[..i]);
            let t = &r[i..];
            match ptok(t) {
                Some((k, l)) if k >= 1 && k <= self.pst.len() => {
                    o.push_str(&self.pst[k - 1]);
                    r = &t[t.chars().take(l).map(char::len_utf8).sum::<usize>()..];
                }
                _ => {
                    o.push('[');
                    r = &t[1..];
                }
            }
        }
        o.push_str(r);
        o
    }

    fn vmove(&mut self, up: bool) -> bool {
        let w = self.ig.get().2;
        if w == 0 {
            return false;
        }
        let (ls, (cx, cy)) = inwrap(&self.inp, self.cur, w);
        let (st, _) = inlay(&self.inp, w);
        let ty = if up {
            if cy == 0 {
                return false;
            }
            cy - 1
        } else {
            if cy + 1 >= ls.len() || cy + 1 >= st.len() {
                return false;
            }
            cy + 1
        };
        let mut j = st[ty];
        let mut col = 0;
        for c in ls[ty].chars() {
            let x = c.width().unwrap_or(0);
            if col + x > cx {
                break;
            }
            col += x;
            j += 1;
        }
        self.cur = j.min(self.nch());
        true
    }

    /// Images referenced by `[image #N]` tokens in `s` as (media type, bytes), or `None` when there are none.
    pub fn content(&self, s: &str) -> Option<Vec<(&'static str, &[u8])>> {
        let mut seen = vec![];
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '[' {
                let t: String = cs[i..].iter().collect();
                if let Some((k, l)) = imgtok(&t) {
                    if k >= 1 && k <= self.imgs.len() && !seen.contains(&k) {
                        seen.push(k);
                    }
                    i += l;
                    continue;
                }
            }
            i += 1;
        }
        if seen.is_empty() {
            return None;
        }
        Some(
            seen.into_iter()
                .map(|k| (self.imgs[k - 1].0, self.imgs[k - 1].1.as_slice()))
                .collect(),
        )
    }

    /// Resume picker rows that match the filter in `rq`.
    pub fn rhits(&self) -> Vec<(String, String, String)> {
        let Some((v, _)) = &self.rs else {
            return vec![];
        };
        let q = self.rq.to_lowercase();
        v.iter()
            .filter(|x| {
                q.is_empty() || x.1.to_lowercase().contains(&q) || x.2.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    /// Indices in `bs` of the user messages.
    pub fn users(&self) -> Vec<usize> {
        self.bs
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b, B::U(..)))
            .map(|(i, _)| i)
            .collect()
    }

    fn rwkey(&mut self, k: KeyEvent) {
        let Some((sel, opt)) = self.rw else { return };
        let us = self.users();
        match (k.code, opt) {
            (KeyCode::Esc, Some(_)) => self.rw = Some((sel, None)),
            (KeyCode::Esc, None) => self.rw = None,
            (KeyCode::Up, None) => self.rw = Some((sel.saturating_sub(1), None)),
            (KeyCode::Down, None) => {
                self.rw = Some(((sel + 1).min(us.len().saturating_sub(1)), None))
            }
            (KeyCode::Enter, None) => self.rw = Some((sel, Some(0))),
            (KeyCode::Left, Some(o)) => self.rw = Some((sel, Some(o.saturating_sub(1)))),
            (KeyCode::Right | KeyCode::Tab, Some(o)) => self.rw = Some((sel, Some((o + 1).min(2)))),
            (KeyCode::Char(c @ '1'..='3'), Some(_)) => {
                self.rw = Some((sel, Some(c as usize - '1' as usize)))
            }
            (KeyCode::Enter, Some(o)) => {
                self.rw = None;
                if let Some(&i) = us.get(sel) {
                    self.rewind(i, o);
                }
            }
            _ => {}
        }
    }

    /// Rewinds the chat to user message `i`: `mode` 0 is code and conversation, 1 conversation only, 2 code only.
    pub fn rewind(&mut self, i: usize, mode: usize) {
        if self.lv.is_some() {
            self.lrewind(i, mode);
            return;
        }
        if mode != 2 {
            if let Some(B::U(t, _)) = self.bs.get(i) {
                self.inp = t.clone();
                self.cur = self.inp.chars().count();
            }
            self.bs.truncate(i);
            self.q.clear();
            self.busy = false;
        }
        self.toast("rewound");
    }

    /// Handles one key press, and records an undo step when the input text changes.
    pub fn key(&mut self, k: KeyEvent) {
        let b = (self.inp.clone(), self.cur);
        let typ = matches!(k.code, KeyCode::Char(c) if !k.modifiers.intersects(Km::CONTROL | Km::ALT) && !c.is_whitespace());
        self.key0(k);
        if std::mem::take(&mut self.unre) || matches!(k.code, KeyCode::Up | KeyCode::Down) {
            self.uk = false;
        } else if self.inp != b.0 {
            if !(typ && self.uk) {
                self.snap(b);
            }
            self.red.clear();
            self.uk = typ;
        } else if !typ {
            self.uk = false;
        }
    }

    fn snap(&mut self, b: (String, usize)) {
        self.und.push(b);
        if self.und.len() > 300 {
            self.und.remove(0);
        }
    }

    fn undo(&mut self, re: bool) {
        let (f, t) = if re {
            (&mut self.red, &mut self.und)
        } else {
            (&mut self.und, &mut self.red)
        };
        if let Some((s, c)) = f.pop() {
            t.push((std::mem::take(&mut self.inp), self.cur));
            self.inp = s;
            self.cur = c.min(self.inp.chars().count());
        }
        self.unre = true;
        self.sug = None;
        self.ms = 0;
    }

    fn tokr(&self) -> Option<usize> {
        let rest: String = self.inp.chars().skip(self.cur).take(48).collect();
        atok(&rest).map(|(_, l)| self.cur + l)
    }

    fn unfold(&mut self) -> bool {
        let cs: Vec<char> = self.inp.chars().collect();
        let (a, b) = match (self.tokl(), self.tokr()) {
            (_, Some(e)) => (self.cur, e),
            (Some(o), None) => (o, self.cur),
            _ => return false,
        };
        let t: String = cs[a..b].iter().collect();
        let Some((k, _)) = ptok(&t).filter(|x| x.0 >= 1 && x.0 <= self.pst.len()) else {
            return false;
        };
        let x = self.pst[k - 1].clone();
        let n = x.chars().count();
        self.inp = cs[..a].iter().collect::<String>() + &x + &cs[b..].iter().collect::<String>();
        self.cur = a + n;
        true
    }

    fn vline(&self) -> Option<(usize, usize)> {
        let w = self.ig.get().2;
        if w == 0 {
            return None;
        }
        let (st, en) = inlay(&self.inp, w);
        let cy = st.iter().rposition(|&a| a <= self.cur)?;
        let cs: Vec<char> = self.inp.chars().collect();
        let mut e = en[cy];
        if cy + 1 < st.len() && st[cy + 1] == e && e > st[cy] && cs.get(e - 1) == Some(&' ') {
            e -= 1;
        }
        Some((st[cy], e))
    }

    fn copylast(&mut self, code: bool) {
        let lu = self
            .bs
            .iter()
            .rposition(|b| matches!(b, B::U(..)))
            .map(|i| i + 1)
            .unwrap_or(0);
        let t: Vec<&str> = self.bs[lu..]
            .iter()
            .filter_map(|b| {
                if let B::T(x) = b {
                    Some(x.as_str())
                } else {
                    None
                }
            })
            .collect();
        let t = if t.is_empty() {
            self.bs
                .iter()
                .rev()
                .find_map(|b| {
                    if let B::T(x) = b {
                        Some(x.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        } else {
            t.join("\n\n")
        };
        let x = if code {
            let mut bl: Vec<String> = vec![];
            let mut cur: Option<String> = None;
            for l in t.lines() {
                if l.trim_start().starts_with("```") {
                    match cur.take() {
                        Some(c) => bl.push(c),
                        None => cur = Some(String::new()),
                    }
                } else if let Some(c) = &mut cur {
                    if !c.is_empty() {
                        c.push('\n');
                    }
                    c.push_str(l);
                }
            }
            bl.pop().unwrap_or_default()
        } else {
            t.trim().to_string()
        };
        if x.is_empty() {
            self.toast(if code {
                "no code block"
            } else {
                "nothing to copy"
            });
            return;
        }
        copy(&x);
        self.toast(format!("copied {} lines", x.lines().count()));
    }

    /// Matches of the find bar in the chat text as (line, column, width).
    pub fn fhits(&self) -> Vec<(usize, usize, usize)> {
        let Some((q, _)) = &self.fs else {
            return vec![];
        };
        let q: Vec<char> = q.chars().flat_map(char::to_lowercase).collect();
        if q.is_empty() {
            return vec![];
        }
        let mut o = vec![];
        for (i, l) in self.btx.borrow().iter().enumerate() {
            let cs: Vec<char> = l.chars().collect();
            let lc: Vec<char> = cs
                .iter()
                .map(|c| c.to_lowercase().next().unwrap_or(*c))
                .collect();
            let mut j = 0;
            while j + q.len() <= lc.len() {
                if lc[j..j + q.len()] == q[..] {
                    let x: usize = cs[..j].iter().map(|c| c.width().unwrap_or(0)).sum();
                    let w: usize = cs[j..j + q.len()]
                        .iter()
                        .map(|c| c.width().unwrap_or(0))
                        .sum();
                    o.push((i, x, w));
                    j += q.len();
                } else {
                    j += 1;
                }
            }
        }
        o
    }

    fn showline(&mut self, i: usize, top: bool) {
        let (nb, bh) = self.bv.get();
        if nb <= bh {
            return;
        }
        let s0 = if top {
            i.saturating_sub(1)
        } else {
            i.saturating_sub(bh / 2)
        };
        self.scr = (nb - bh).saturating_sub(s0).min(self.smax.get());
    }

    fn fgo(&mut self, d: i32) {
        let h = self.fhits();
        let Some((_, ix)) = &mut self.fs else { return };
        if h.is_empty() {
            *ix = 0;
            return;
        }
        let n = h.len() as i32;
        *ix = match d {
            0 => h.len() - 1,
            _ => ((*ix as i32 + d).rem_euclid(n)) as usize,
        };
        let l = h[*ix].0;
        self.showline(l, false);
    }

    fn fskey(&mut self, k: KeyEvent) {
        let Some((q, _)) = &mut self.fs else { return };
        match k.code {
            KeyCode::Esc => self.fs = None,
            KeyCode::Enter if k.modifiers.contains(Km::SHIFT) => self.fgo(1),
            KeyCode::Enter | KeyCode::Up => self.fgo(-1),
            KeyCode::Down => self.fgo(1),
            KeyCode::Backspace => {
                q.pop();
                self.fgo(0);
            }
            KeyCode::Char('u') if k.modifiers.contains(Km::CONTROL) => {
                q.clear();
                self.fgo(0);
            }
            KeyCode::Char(c) if !k.modifiers.contains(Km::CONTROL) => {
                q.push(c);
                self.fgo(0);
            }
            _ => {}
        }
    }

    fn jump(&mut self, up: bool) {
        let (nb, bh) = self.bv.get();
        if nb <= bh {
            return;
        }
        let s0 = (nb - bh).saturating_sub(self.scr.min(self.smax.get())) + 1;
        let ps: Vec<usize> = self
            .btx
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("> "))
            .map(|(i, _)| i)
            .collect();
        let t = if up {
            ps.iter().rev().find(|&&p| p < s0).copied()
        } else {
            ps.iter().find(|&&p| p > s0).copied()
        };
        match t {
            Some(p) => self.showline(p, true),
            None if up => self.scr = self.smax.get(),
            None => self.scr = 0,
        }
    }

    /// Clears the input box and keeps what was there in the history.
    fn clearinp(&mut self) {
        let s = std::mem::take(&mut self.inp);
        if self.hist.last() != Some(&s) {
            self.hist.push(s);
        }
        self.hi = None;
        self.drf = None;
        self.cur = 0
    }

    /// Ctrl+C as in Claude Code: the first press interrupts a running turn or clears the
    /// input and shows a hint; a second press within CC_WIN ticks (~2 s) exits.
    fn ctrlc(&mut self) {
        if self.cc > 0 && self.t.saturating_sub(self.cc) <= CC_WIN {
            self.quit = true;
            return;
        }
        self.cc = self.t.max(1);
        let menu = self.fs.is_some()
            || self.rw.is_some()
            || self.qa.is_some()
            || self.pk.is_some()
            || self.tk.is_some()
            || self.ns.is_some()
            || self.pp.is_some()
            || self.tp.is_some()
            || self.bp.is_some()
            || self.rs.is_some();
        if !menu {
            if self.busy {
                self.stop()
            } else if !self.inp.is_empty() {
                self.clearinp()
            }
        }
        self.toast("press ctrl+c again to exit");
    }

    fn key0(&mut self, k: KeyEvent) {
        let ct = k.modifiers.contains(Km::CONTROL);
        if ct && k.code == KeyCode::Char('c') {
            self.ctrlc();
            return;
        }
        if self.fs.is_some() {
            self.fskey(k);
            return;
        }
        if self.rw.is_some() {
            self.rwkey(k);
            return;
        }
        if self.qa.is_some() && self.qkey(k) {
            return;
        }
        if self.pk.is_some() {
            self.pkkey(k);
            return;
        }
        if let Some((sel, det)) = self.tk {
            let n = self.tasks.len();
            if n == 0 {
                self.tk = None;
                return;
            }
            let sel = sel.min(n - 1);
            match k.code {
                KeyCode::Esc | KeyCode::Left if det => self.tk = Some((sel, false)),
                KeyCode::Esc => self.tk = None,
                KeyCode::Up if !det => self.tk = Some((sel.saturating_sub(1), false)),
                KeyCode::Down if !det => self.tk = Some(((sel + 1).min(n - 1), false)),
                KeyCode::Enter | KeyCode::Right if !det => {
                    self.tk = Some((sel, true));
                    self.tkp = self.t;
                    self.tpoll(sel);
                }
                KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace
                    if self.tasks[sel].st == 0 =>
                {
                    self.tstop(sel);
                }
                _ => {}
            }
            return;
        }
        if let Some(sel) = self.ns {
            let n = self.nrows().len();
            match k.code {
                KeyCode::Esc => self.ns = None,
                KeyCode::Up => self.ns = Some(sel.saturating_sub(1)),
                KeyCode::Down => self.ns = Some((sel + 1).min(n - 1)),
                KeyCode::Left if sel == 3 => self.cycleff(false),
                KeyCode::Left if (8..=13).contains(&sel) => {
                    let j = sel - 8;
                    let n = LAYOUT[j].1.len();
                    setlay(j, (lay(j) + n - 1) % n);
                }
                KeyCode::Left if sel == 15 => self.setopac(false),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => match sel {
                    0 if upd::managed().is_some() => {}
                    0 => {
                        let on = !upd::enabled();
                        upd::set_enabled(on);
                        self.toast(format!("auto-update  {}", if on { "on" } else { "off" }));
                    }
                    1 => {
                        self.ns = None;
                        self.cmd("theme");
                    }
                    2 => {
                        self.ns = None;
                        self.cmd("default-permission");
                    }
                    3 => self.cycleff(true),
                    4 => {
                        self.ns = None;
                        self.cmd("background");
                    }
                    5..=7 => {
                        let j = sel - 5;
                        let on = !bars()[j];
                        setbar(j, on);
                        self.toast(format!("{}  {}", BARS[j], if on { "on" } else { "off" }));
                    }
                    8..=13 => {
                        let j = sel - 8;
                        let n = LAYOUT[j].1.len();
                        setlay(j, (lay(j) + 1) % n);
                    }
                    14 => {
                        let on = !rclick();
                        setrclick(on);
                        self.toast(format!(
                            "right-click paste  {}",
                            if on { "on" } else { "off" }
                        ));
                    }
                    15 => self.setopac(true),
                    16 => {
                        let on = !free::enabled();
                        free::set_enabled(on);
                        self.frat = None;
                        self.toast(format!(
                            "free token count  {}",
                            if on { "on" } else { "off" }
                        ));
                    }
                    17 => {
                        let on = !free::claude_enabled();
                        free::set_claude_enabled(on);
                        self.clat = None;
                        self.toast(format!(
                            "claude token count  {}",
                            if on { "on" } else { "off" }
                        ));
                    }
                    _ => self.upcheck(true),
                },
                _ => {}
            }
            return;
        }
        if let Some(sel) = self.pp {
            match k.code {
                KeyCode::Esc => self.pp = None,
                KeyCode::Up => self.pp = Some(sel.saturating_sub(1)),
                KeyCode::Down => self.pp = Some((sel + 1).min(MODES.len() - 1)),
                KeyCode::Enter => {
                    self.pp = None;
                    self.setdefmode(sel);
                }
                _ => {}
            }
            return;
        }
        if let Some((sel, prev)) = self.tp {
            let n = pal::all().len();
            match k.code {
                KeyCode::Esc => {
                    pal::set(prev);
                    self.tp = None;
                }
                KeyCode::Up | KeyCode::Down => {
                    let j = if k.code == KeyCode::Up {
                        sel.saturating_sub(1)
                    } else {
                        (sel + 1).min(n - 1)
                    };
                    self.tp = Some((j, prev));
                    pal::set(j);
                }
                KeyCode::Enter => {
                    self.tp = None;
                    self.settheme(sel);
                }
                _ => {}
            }
            return;
        }
        if let Some((sel, prev)) = self.bp {
            let v = bg::avail();
            let pv = |i: usize| if v[i] == "off" { None } else { Some(v[i]) };
            match k.code {
                KeyCode::Esc => {
                    self.bgw = prev;
                    self.bp = None;
                }
                KeyCode::Up | KeyCode::Down => {
                    let n = if k.code == KeyCode::Up {
                        sel.saturating_sub(1)
                    } else {
                        (sel + 1).min(v.len() - 1)
                    };
                    self.bp = Some((n, prev));
                    self.bgw = pv(n);
                }
                KeyCode::Enter => {
                    self.bp = None;
                    self.setbg(pv(sel));
                }
                _ => {}
            }
            return;
        }
        if self.rs.is_some() {
            let hits = self.rhits();
            let Some((_, sel)) = &mut self.rs else { return };
            match k.code {
                KeyCode::Esc if !self.rq.is_empty() => {
                    self.rq.clear();
                    *sel = 0
                }
                KeyCode::Esc => self.rs = None,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(hits.len().saturating_sub(1)),
                KeyCode::Backspace => {
                    self.rq.pop();
                    *sel = 0
                }
                KeyCode::Char(c) => {
                    self.rq.push(c);
                    *sel = 0
                }
                KeyCode::Enter => {
                    let id = hits.get(*sel).map(|x| x.0.clone());
                    self.rs = None;
                    self.rq.clear();
                    if let Some(id) = id {
                        self.lresume(&id);
                    }
                }
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Up if k.modifiers.contains(Km::ALT) => self.jump(true),
            KeyCode::Down if k.modifiers.contains(Km::ALT) => self.jump(false),
            KeyCode::Char('f') if ct => self.fs = Some((String::new(), 0)),
            KeyCode::Char('d') if ct && self.inp.is_empty() => self.quit = true,
            KeyCode::Esc => {
                let dbl = self.t.saturating_sub(self.le) < 18;
                self.le = self.t;
                if self.btw.is_some() {
                    self.btw = None
                } else if self.busy {
                    self.stop()
                } else if !self.inp.is_empty() {
                    self.clearinp()
                } else if dbl && !self.users().is_empty() {
                    self.rw = Some((self.users().len() - 1, None));
                    self.le = 0;
                }
            }
            KeyCode::Enter if k.modifiers.intersects(Km::SHIFT | Km::ALT) => self.ins('\n'),
            KeyCode::Enter => {
                if self.ask.is_some() && self.inp.is_empty() {
                    self.answer('y')
                } else {
                    self.submit()
                }
            }
            KeyCode::Char(c)
                if !ct
                    && self.ask.is_some()
                    && self.inp.is_empty()
                    && matches!(c, 'y' | 'n' | 'a') =>
            {
                self.answer(c)
            }
            KeyCode::Char('o') if ct && self.unfold() => {}
            KeyCode::Char('z' | 'Z')
                if ct && (k.modifiers.contains(Km::SHIFT) || k.code == KeyCode::Char('Z')) =>
            {
                self.undo(true)
            }
            KeyCode::Char('z') if ct => self.undo(false),
            KeyCode::Char('y') if ct => self.undo(true),
            KeyCode::Char('o') if ct => {
                self.open = !self.open;
                self.to = self.t
            }
            KeyCode::Char('t') if ct => self.stodo = !self.stodo,
            KeyCode::Char('a') if ct => self.cur = 0,
            KeyCode::Char('e') if ct => self.cur = self.nch(),
            KeyCode::Char('u') if ct => {
                let i = self.bi();
                self.inp.drain(..i);
                self.cur = 0
            }
            KeyCode::Char('v') if ct => self.clipv(),
            KeyCode::Char('g') if ct => self.edit = true,
            KeyCode::Char('b') if ct => self.tbg(),
            KeyCode::Down if self.inp.is_empty() && self.hi.is_none() && !self.tasks.is_empty() => {
                self.tk = Some((self.tasks.len() - 1, false))
            }
            KeyCode::Backspace if k.modifiers.intersects(Km::CONTROL | Km::ALT) => {
                let j = self.wl();
                self.cut(j, self.cur)
            }
            KeyCode::Char('h') if ct => {
                let j = self.wl();
                self.cut(j, self.cur)
            }
            KeyCode::Delete if k.modifiers.intersects(Km::CONTROL | Km::ALT) => {
                let j = self.wr();
                self.cut(self.cur, j)
            }
            KeyCode::Char('d') if k.modifiers.contains(Km::ALT) => {
                let j = self.wr();
                self.cut(self.cur, j)
            }
            KeyCode::Char('k') if ct => {
                let n = self.nch();
                self.cut(self.cur, n)
            }
            KeyCode::Left if k.modifiers.intersects(Km::CONTROL | Km::ALT) => self.cur = self.wl(),
            KeyCode::Right if k.modifiers.intersects(Km::CONTROL | Km::ALT) => self.cur = self.wr(),
            KeyCode::Char('b') if k.modifiers.contains(Km::ALT) => self.cur = self.wl(),
            KeyCode::Char('f') if k.modifiers.contains(Km::ALT) => self.cur = self.wr(),
            KeyCode::Char('w') if ct => {
                let cs: Vec<char> = self.inp.chars().collect();
                let mut j = self.cur;
                while j > 0 && cs[j - 1] == ' ' {
                    j -= 1
                }
                while j > 0 && cs[j - 1] != ' ' {
                    j -= 1
                }
                let s: String = cs[..j].iter().chain(cs[self.cur..].iter()).collect();
                self.inp = s;
                self.cur = j
            }
            KeyCode::BackTab => {
                self.mode = (self.mode + 1) % MODES.len();
                self.tm = self.t.max(1);
                self.lmode();
                self.toast(format!("mode  {}", MODES[self.mode]))
            }
            KeyCode::Char(c) if !ct => self.ins(c),
            KeyCode::Backspace if self.tokl().is_some() => {
                let j = self.tokl().unwrap_or(self.cur);
                self.cut(j, self.cur)
            }
            KeyCode::Backspace => {
                self.ms = 0;
                if self.cur > 0 {
                    self.cur -= 1;
                    let i = self.bi();
                    self.inp.remove(i);
                }
            }
            KeyCode::Delete if self.tokr().is_some() => {
                let e = self.tokr().unwrap_or(self.cur);
                self.cut(self.cur, e)
            }
            KeyCode::Delete => {
                if self.cur < self.nch() {
                    let i = self.bi();
                    self.inp.remove(i);
                }
            }
            KeyCode::Left if self.tokl().is_some() => self.cur = self.tokl().unwrap_or(self.cur),
            KeyCode::Right if self.tokr().is_some() => self.cur = self.tokr().unwrap_or(self.cur),
            KeyCode::Left => self.cur = self.cur.saturating_sub(1),
            KeyCode::Right => self.cur = (self.cur + 1).min(self.nch()),
            KeyCode::Home => {
                self.cur = self
                    .vline()
                    .map(|x| x.0)
                    .filter(|&a| a != self.cur)
                    .unwrap_or(0)
            }
            KeyCode::End => {
                self.cur = self
                    .vline()
                    .map(|x| x.1)
                    .filter(|&e| e != self.cur)
                    .unwrap_or(self.nch())
            }
            KeyCode::Up if self.vmove(true) => {}
            KeyCode::Down if self.vmove(false) => {}
            KeyCode::Up => {
                if self.hist.is_empty() {
                    return;
                }
                let i = match self.hi {
                    None => {
                        self.drf = Some(self.inp.clone());
                        self.hist.len() - 1
                    }
                    Some(i) => i.saturating_sub(1),
                };
                self.hi = Some(i);
                self.inp = self.hist[i].clone();
                self.cur = self.nch()
            }
            KeyCode::Down => {
                if let Some(i) = self.hi {
                    if i + 1 < self.hist.len() {
                        self.hi = Some(i + 1);
                        self.inp = self.hist[i + 1].clone();
                    } else {
                        self.hi = None;
                        self.inp = self.drf.take().unwrap_or_default();
                    }
                    self.cur = self.nch()
                }
            }
            KeyCode::PageUp => self.scr = (self.scr + 10).min(self.smax.get()),
            KeyCode::PageDown => self.scr = self.scr.saturating_sub(10),
            _ => {}
        }
    }

    /// Number of successful edit tool calls since the last user message.
    pub fn tedits(&self) -> usize {
        let lu = self
            .bs
            .iter()
            .rposition(|b| matches!(b, B::U(..)))
            .unwrap_or(0);
        self.bs[lu..]
            .iter()
            .filter(|b| matches!(b, B::Tl(t) if t.k == Tk::Edit && t.st == St::Ok))
            .count()
    }

    /// Indices in `bs` of the tool calls.
    pub fn tools(&self) -> Vec<usize> {
        self.bs
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b, B::Tl(_)))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Windows reports AltGr as Ctrl+Alt, so AltGr+A on a Polish layout comes in as Ctrl+Alt+'ą'
/// and was dropped as a shortcut. A Ctrl+Alt key that carries a printable character other than
/// an ASCII letter or digit (those are real Ctrl+Alt shortcuts) is typed text: drop Ctrl and Alt.
pub fn altgr(mut k: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(c) = k.code {
        if k.modifiers.contains(Km::CONTROL | Km::ALT)
            && !c.is_control()
            && !c.is_whitespace()
            && !c.is_ascii_alphanumeric()
        {
            k.modifiers.remove(Km::CONTROL | Km::ALT);
        }
    }
    k
}

fn one_line(s: &str, n: usize) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    if l.chars().count() > n {
        l.chars().take(n - 1).collect::<String>() + "…"
    } else {
        l.to_string()
    }
}

/// Up to 12 files and folders under the current directory that fuzzy-match `q`, best first; hidden entries and build folders are skipped.
pub fn localfiles(q: &str) -> Vec<String> {
    let root = std::env::current_dir().unwrap_or_default();
    let mut all = vec![];
    let mut st = vec![root.clone()];
    while let Some(d) = st.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.')
                || matches!(
                    n.as_str(),
                    "target" | "node_modules" | "dist" | "build" | "__pycache__"
                )
            {
                continue;
            }
            let rel = p
                .strip_prefix(&root)
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_default();
            if p.is_dir() {
                all.push(format!("{rel}/"));
                if all.len() < 20000 {
                    st.push(p);
                }
            } else {
                all.push(rel);
            }
        }
        if all.len() > 20000 {
            break;
        }
    }
    let ql = q.to_lowercase();
    let mut hits: Vec<(i64, String)> = all
        .into_iter()
        .filter_map(|p| {
            let pl = p.to_lowercase();
            let mut it = pl.chars();
            if !ql.chars().all(|c| it.any(|x| x == c)) {
                return None;
            }
            let base = pl
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();
            let sc = if base.starts_with(&ql) {
                0
            } else if pl.contains(&ql) {
                1000
            } else {
                3000
            } + p.len() as i64;
            Some((sc, p))
        })
        .collect();
    hits.sort();
    hits.into_iter().take(12).map(|x| x.1).collect()
}

/// Diff text of `path` from the workspace diff `d` as (marker character, text), hunk headers included.
pub fn difflines(d: Option<&serde_json::Value>, path: &str) -> Vec<(char, String)> {
    let mut o = vec![];
    let Some(d) = d else { return o };
    for f in d["hunks"].as_array().cloned().unwrap_or_default() {
        if f["path"].as_str() != Some(path) {
            continue;
        }
        for h in f["hunks"].as_array().cloned().unwrap_or_default() {
            o.push((
                '@',
                format!(
                    "@@ -{},{} +{},{} @@",
                    h["oldStart"], h["oldLines"], h["newStart"], h["newLines"]
                ),
            ));
            for l in h["lines"].as_array().cloned().unwrap_or_default() {
                let l = l.as_str().unwrap_or("").to_string();
                let c = l.chars().next().unwrap_or(' ');
                o.push((c, l.chars().skip(1).collect()));
            }
        }
    }
    if o.is_empty() {
        o.push((' ', "no diff text".into()));
    }
    o
}

fn tokend(cs: &[char]) -> Option<usize> {
    if cs.last() != Some(&']') {
        return None;
    }
    let o = cs.iter().rposition(|&c| c == '[')?;
    let t: String = cs[o..].iter().collect();
    atok(&t).filter(|&(_, l)| l == t.chars().count()).map(|_| o)
}

/// Reads a `[pasted #N]` token at the start of `s` as (N, length in characters).
pub fn ptok(s: &str) -> Option<(usize, usize)> {
    let r = s.strip_prefix("[pasted #")?;
    let d: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d.is_empty() {
        return None;
    }
    let e = r[d.len()..]
        .chars()
        .take(32)
        .position(|c| c == ']' || c == '[' || c == '\n')?;
    if r[d.len()..].chars().nth(e) != Some(']') {
        return None;
    }
    Some((d.parse().ok()?, 9 + d.len() + e + 1))
}

/// Reads an image or pasted token at the start of `s` as (N, length in characters).
pub fn atok(s: &str) -> Option<(usize, usize)> {
    imgtok(s).or_else(|| ptok(s))
}

/// Reads an `[image #N]` token at the start of `s` as (N, length in characters).
pub fn imgtok(s: &str) -> Option<(usize, usize)> {
    let r = s.strip_prefix("[image #")?;
    let d: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d.is_empty() || !r[d.len()..].starts_with(']') {
        return None;
    }
    Some((d.parse().ok()?, 9 + d.len()))
}

/// Splits `s` into spans, using `hi` for image and pasted tokens and `base` for the rest.
pub fn imgspans(s: &str, base: Style, hi: Style) -> Vec<Span<'static>> {
    let mut v = vec![];
    let mut cur = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some((_, l)) = atok(rest) {
            if !cur.is_empty() {
                v.push(Span::styled(std::mem::take(&mut cur), base));
            }
            let b: usize = rest.chars().take(l).map(char::len_utf8).sum();
            v.push(Span::styled(rest[..b].to_string(), hi));
            rest = &rest[b..];
        } else {
            let Some(c) = rest.chars().next() else { break };
            cur.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if !cur.is_empty() {
        v.push(Span::styled(cur, base));
    }
    v
}

/// Word-wraps `s` to width `w`, keeping existing line breaks.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut o = vec![];
    for para in s.split('\n') {
        for l in wsegs(&[(para.to_string(), Style::default())], w) {
            o.push(l.iter().map(|x| x.content.to_string()).collect::<String>());
        }
    }
    o
}

fn words(s: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    let mut sp = None;
    for c in s.chars() {
        let isp = c == ' ';
        match v.last_mut() {
            Some(last) if sp == Some(isp) => last.push(c),
            _ => {
                v.push(c.to_string());
                sp = Some(isp);
            }
        }
    }
    v
}

/// Word-wraps styled text segments to width `w` into lines of spans.
pub fn wsegs(segs: &[(String, Style)], w: usize) -> Vec<Vec<Span<'static>>> {
    wsegs2(segs, w, w)
}

/// Word-wraps styled text segments into lines of spans: the first line `w0` wide, the
/// others `w`. Text that touches across segments (no space between, as in a highlighted
/// `https://...`) stays one word, so a line never breaks inside it except at the edge.
pub fn wsegs2(segs: &[(String, Style)], w0: usize, w: usize) -> Vec<Vec<Span<'static>>> {
    let (w0, w) = (w0.max(4), w.max(4));
    // words across segments: (is space, pieces with their style)
    let mut wds: Vec<(bool, Vec<(String, Style)>)> = vec![];
    for (t, st) in segs {
        for wd in words(t) {
            let sp = wd.starts_with(' ');
            match wds.last_mut() {
                Some((s, p)) if *s == sp => p.push((wd, *st)),
                _ => wds.push((sp, vec![(wd, *st)])),
            }
        }
    }
    let lim = |ls: &Vec<Vec<Span<'static>>>| if ls.is_empty() { w0 } else { w };
    let mut ls = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut cw = 0;
    for (sp, ps) in wds {
        let ww: usize = ps.iter().map(|p| p.0.width()).sum();
        if sp {
            if cw == 0 {
                continue;
            }
            if cw + ww > lim(&ls) {
                ls.push(std::mem::take(&mut cur));
                cw = 0;
                continue;
            }
            cur.extend(ps.into_iter().map(|(t, s)| Span::styled(t, s)));
            cw += ww;
            continue;
        }
        if cw + ww > lim(&ls) && cw > 0 {
            while let Some(l) = cur.last() {
                if l.content.trim().is_empty() {
                    cur.pop();
                } else {
                    break;
                }
            }
            ls.push(std::mem::take(&mut cur));
            cw = 0;
        }
        if ww > lim(&ls) {
            for (t, st) in ps {
                let mut b = String::new();
                for c in t.chars() {
                    let x = c.width().unwrap_or(0);
                    if cw + x > lim(&ls) {
                        if !b.is_empty() {
                            cur.push(Span::styled(std::mem::take(&mut b), st));
                        }
                        ls.push(std::mem::take(&mut cur));
                        cw = 0;
                    }
                    b.push(c);
                    cw += x;
                }
                if !b.is_empty() {
                    cur.push(Span::styled(b, st));
                }
            }
            continue;
        }
        cur.extend(ps.into_iter().map(|(t, s)| Span::styled(t, s)));
        cw += ww;
    }
    if !cur.is_empty() || ls.is_empty() {
        ls.push(cur);
    }
    ls
}

/// Styles for rendering markdown and code in the chat.
pub struct Md {
    /// Normal text.
    pub t: Style,
    /// Bold text and table headings.
    pub b: Style,
    /// Inline code.
    pub c: Style,
    /// Identifiers in code blocks.
    pub fe: Style,
    /// Keywords in code blocks.
    pub kw: Style,
    /// String literals in code blocks.
    pub st: Style,
    /// Comments in code blocks.
    pub cm: Style,
    /// Numbers in code blocks.
    pub nu: Style,
    /// Bullet prefix of list items.
    pub bul: &'static str,
    /// Style of the bullet prefix.
    pub bs: Style,
    /// Prefix of each line in a code block.
    pub fpre: &'static str,
    /// Style of the code block prefix and of table cells after the first column.
    pub fps: Style,
}

fn inl(s: &str, base: Style, m: &Md) -> Vec<(String, Style)> {
    let mut v = vec![];
    let mut cur = String::new();
    let (mut code, mut bold) = (false, false);
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let sty = |c: bool, b: bool| {
        if c {
            m.c
        } else if b {
            m.b
        } else {
            base
        }
    };
    while i < cs.len() {
        if cs[i] == '`' {
            v.push((std::mem::take(&mut cur), sty(code, bold)));
            code = !code;
            i += 1;
            continue;
        }
        if !code && cs[i] == '*' && i + 1 < cs.len() && cs[i + 1] == '*' {
            v.push((std::mem::take(&mut cur), sty(code, bold)));
            bold = !bold;
            i += 2;
            continue;
        }
        cur.push(cs[i]);
        i += 1;
    }
    v.push((cur, sty(code, bold)));
    v.retain(|x| !x.0.is_empty());
    v
}

const KW: [&str; 16] = [
    "fn", "let", "mut", "pub", "self", "Self", "impl", "struct", "use", "for", "in", "if", "else",
    "match", "return", "as",
];

/// Syntax-highlights one line of code into spans.
pub fn hl(s: &str, m: &Md) -> Vec<Span<'static>> {
    let mut v = vec![];
    let (code, cmt) = match s.find("//") {
        Some(i) => (&s[..i], Some(&s[i..])),
        None => (s, None),
    };
    let mut w = String::new();
    let mut ins = false;
    let flush = |w: &mut String, v: &mut Vec<Span<'static>>| {
        if w.is_empty() {
            return;
        }
        let t = std::mem::take(w);
        let st = if KW.contains(&t.as_str()) {
            m.kw
        } else if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            m.nu
        } else {
            m.fe
        };
        v.push(Span::styled(t, st));
    };
    for c in code.chars() {
        if ins {
            w.push(c);
            if c == '"' {
                v.push(Span::styled(std::mem::take(&mut w), m.st));
                ins = false;
            }
            continue;
        }
        if c == '"' {
            flush(&mut w, &mut v);
            ins = true;
            w.push(c);
            continue;
        }
        if c.is_alphanumeric()
            || c == '_'
            || (c == '.' && w.chars().next().is_some_and(|x| x.is_ascii_digit()))
        {
            w.push(c);
        } else {
            flush(&mut w, &mut v);
            v.push(Span::styled(c.to_string(), m.fe));
        }
    }
    if ins {
        v.push(Span::styled(w, m.st));
    } else {
        flush(&mut w, &mut v);
    }
    if let Some(c) = cmt {
        v.push(Span::styled(c.to_string(), m.cm));
    }
    v
}

fn tcols(nat: &[usize], av: usize) -> Vec<usize> {
    let n = nat.len();
    if nat.iter().sum::<usize>() <= av {
        return nat.to_vec();
    }
    let mut cw = vec![0; n];
    let mut left: Vec<usize> = (0..n).collect();
    let mut rem = av;
    loop {
        let sh = rem / left.len().max(1);
        let (fit, big): (Vec<usize>, Vec<usize>) = left.iter().partition(|&&j| nat[j] <= sh);
        if fit.is_empty() {
            for (k, &j) in big.iter().enumerate() {
                cw[j] = (sh + usize::from(k < rem % big.len().max(1))).max(4);
            }
            return cw;
        }
        for &j in &fit {
            cw[j] = nat[j].max(1);
            rem = rem.saturating_sub(cw[j]);
        }
        left = big;
        if left.is_empty() {
            return cw;
        }
    }
}

fn tbl(rows: &[&str], w: usize, m: &Md, o: &mut Vec<Line<'static>>) {
    let issep =
        |r: &str| r.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && r.contains('-');
    let al: Vec<bool> = rows
        .iter()
        .find(|r| issep(r))
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|x| {
                    let x = x.trim();
                    x.ends_with(':') && !x.starts_with(':')
                })
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<&str> = rows.iter().copied().filter(|r| !issep(r)).collect();
    let rows = &rows[..];
    let rs: Vec<Vec<Vec<(String, Style)>>> = rows
        .iter()
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|x| x.trim())
                .collect::<Vec<_>>()
        })
        .enumerate()
        .map(|(i, cs)| {
            cs.iter()
                .enumerate()
                .map(|(j, c)| {
                    inl(
                        c,
                        if i == 0 {
                            m.b
                        } else if j == 0 {
                            m.t
                        } else {
                            m.fps
                        },
                        m,
                    )
                })
                .collect()
        })
        .collect();
    let n = rs.iter().map(|r| r.len()).max().unwrap_or(0);
    if n == 0 {
        return;
    }
    let sep = 3;
    let cwid = |c: &Vec<(String, Style)>| c.iter().map(|x| x.0.width()).sum::<usize>();
    let nat: Vec<usize> = (0..n)
        .map(|j| {
            rs.iter()
                .filter_map(|r| r.get(j))
                .map(cwid)
                .max()
                .unwrap_or(0)
        })
        .collect();
    let av = w.saturating_sub(sep * (n - 1));
    if av < n * 6 {
        let hd: Vec<String> = rs[0]
            .iter()
            .map(|c| c.iter().map(|x| x.0.clone()).collect())
            .collect();
        for r in rs.iter().skip(1) {
            for (j, c) in r.iter().enumerate() {
                let mut sg = vec![];
                if let Some(h) = hd.get(j).filter(|h| !h.is_empty()) {
                    sg.push((format!("{h}: "), m.b));
                }
                sg.extend(c.iter().cloned());
                let pre = if j == 0 {
                    m.bul.to_string()
                } else {
                    " ".repeat(m.bul.width())
                };
                for (k, l) in wsegs(&sg, w.saturating_sub(pre.width()))
                    .into_iter()
                    .enumerate()
                {
                    let mut v = vec![Span::styled(
                        if k == 0 {
                            pre.clone()
                        } else {
                            " ".repeat(pre.width())
                        },
                        m.bs,
                    )];
                    v.extend(l);
                    o.push(Line::from(v));
                }
            }
        }
        return;
    }
    let cw = tcols(&nat, av);
    for (i, r) in rs.iter().enumerate() {
        let cells: Vec<Vec<Vec<Span<'static>>>> = (0..n)
            .map(|j| r.get(j).map(|c| wsegs(c, cw[j])).unwrap_or_default())
            .collect();
        let h = cells.iter().map(|c| c.len()).max().unwrap_or(1).max(1);
        for k in 0..h {
            let mut sp = vec![];
            for j in 0..n {
                if j > 0 {
                    sp.push(Span::styled(" │ ", m.fps));
                }
                let l = cells[j].get(k).cloned().unwrap_or_default();
                let lw = sw(&l);
                let gap = Span::raw(" ".repeat(cw[j].saturating_sub(lw)));
                if al.get(j).copied().unwrap_or(false) {
                    sp.push(gap);
                    sp.extend(l);
                } else {
                    sp.extend(l);
                    if j + 1 < n {
                        sp.push(gap);
                    }
                }
            }
            o.push(Line::from(sp));
        }
        if i == 0 && rs.len() > 1 {
            let ru: Vec<String> = cw.iter().map(|&x| "─".repeat(x)).collect();
            o.push(Line::from(Span::styled(ru.join("─┼─"), m.fps)));
        }
    }
}

/// One code line, word-wrapped to `w` behind the code prefix; wrapped parts keep the indent plus two spaces.
fn code(raw: &str, w: usize, m: &Md, o: &mut Vec<Line<'static>>) {
    let body = raw.trim_start_matches(' ');
    let ind = " ".repeat(raw.len() - body.len());
    let segs: Vec<(String, Style)> = hl(body, m)
        .into_iter()
        .map(|x| (x.content.into_owned(), x.style))
        .collect();
    let room = w.saturating_sub(m.fpre.width() + ind.len());
    // the first line uses the full room and the wrapped parts the room after their two
    // extra spaces, so a long word cut at the edge ends at the right edge on every line
    // and a link in it is joined back whole (see `links` in main.rs)
    let ls = wsegs2(&segs, room, room.saturating_sub(2).max(room / 2));
    if ls.is_empty() {
        o.push(Line::from(vec![
            Span::styled(m.fpre.to_string(), m.fps),
            Span::raw(ind),
        ]));
        return;
    }
    for (i, l) in ls.into_iter().enumerate() {
        let lead = if i == 0 {
            ind.clone()
        } else {
            format!("{ind}  ")
        };
        let mut sp = vec![Span::styled(m.fpre.to_string(), m.fps), Span::raw(lead)];
        sp.extend(l);
        o.push(Line::from(sp));
    }
}

/// Renders markdown text `s` into lines for width `w`.
pub fn md(s: &str, w: usize, m: &Md) -> Vec<Line<'static>> {
    let mut o = vec![];
    let mut inf = false;
    let mut tb: Vec<&str> = vec![];
    for raw in s.lines() {
        if !inf && raw.trim_start().starts_with('|') {
            tb.push(raw);
            continue;
        }
        if !tb.is_empty() {
            tbl(&tb, w, m, &mut o);
            tb.clear();
        }
        if raw.trim_start().starts_with("```") {
            inf = !inf;
            continue;
        }
        if inf {
            code(raw, w, m, &mut o);
            continue;
        }
        if raw.trim().is_empty() {
            o.push(Line::default());
            continue;
        }
        let (pre, body) = match raw.strip_prefix("- ") {
            Some(r) => (m.bul.to_string(), r),
            None => (String::new(), raw),
        };
        let pw = pre.width();
        let ind = " ".repeat(pw);
        for (i, l) in wsegs(&inl(body, m.t, m), w.saturating_sub(pw))
            .into_iter()
            .enumerate()
        {
            let mut v = vec![Span::styled(
                if i == 0 { pre.clone() } else { ind.clone() },
                m.bs,
            )];
            v.extend(l);
            o.push(Line::from(v));
        }
    }
    if !tb.is_empty() {
        tbl(&tb, w, m, &mut o);
    }
    let mut r: Vec<Line<'static>> = vec![];
    for l in o {
        let e = l.spans.iter().all(|x| x.content.trim().is_empty());
        if e && r
            .last()
            .is_none_or(|p: &Line| p.spans.iter().all(|x| x.content.trim().is_empty()))
        {
            continue;
        }
        r.push(l);
    }
    while r
        .last()
        .is_some_and(|l| l.spans.iter().all(|x| x.content.trim().is_empty()))
    {
        r.pop();
    }
    r
}

/// The last `h` lines of `v`, moved up by the scroll offset `scr`, which is clamped to what fits.
pub fn tail(v: Vec<Line<'static>>, h: usize, scr: &mut usize) -> Vec<Line<'static>> {
    let n = v.len();
    if n <= h {
        *scr = 0;
        return v;
    }
    *scr = (*scr).min(n - h);
    let s = n - h - *scr;
    v.into_iter().skip(s).take(h).collect()
}

/// Input box layout for width `w` as (start, end) character indices of each visual line.
pub fn inlay(s: &str, w: usize) -> (Vec<usize>, Vec<usize>) {
    let w = w.max(2);
    let cs: Vec<char> = s.chars().collect();
    let mut tk = vec![false; cs.len()];
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '[' {
            let t: String = cs[i..].iter().take(48).collect();
            if let Some((_, l)) = atok(&t) {
                for x in tk.iter_mut().take(i + l).skip(i + 1) {
                    *x = true;
                }
                i += l;
                continue;
            }
        }
        i += 1;
    }
    let cw = |c: char| c.width().unwrap_or(0);
    let mut st = vec![0];
    let mut en = vec![];
    let mut col = 0;
    let mut brk: Option<usize> = None;
    for i in 0..cs.len() {
        let c = cs[i];
        if c == '\n' {
            en.push(i);
            st.push(i + 1);
            col = 0;
            brk = None;
            continue;
        }
        let x = cw(c);
        if c == ' ' && !tk[i] {
            if col + x > w {
                en.push(i + 1);
                st.push(i + 1);
                col = 0;
                brk = None;
            } else {
                col += x;
                brk = Some(i + 1);
            }
            continue;
        }
        if col + x > w {
            let s0 = st.last().copied().unwrap_or(0);
            let b = brk.filter(|&b| b > s0 && b <= i).unwrap_or(i);
            en.push(b);
            st.push(b);
            col = cs[b..i].iter().map(|&c| cw(c)).sum();
            brk = None;
        }
        col += x;
    }
    en.push(cs.len());
    (st, en)
}

/// Wraps the input `s` to width `w`: the visual lines and the cursor as (column, line) for character index `cur`.
pub fn inwrap(s: &str, cur: usize, w: usize) -> (Vec<String>, (usize, usize)) {
    let w = w.max(2);
    let cs: Vec<char> = s.chars().collect();
    let (st, en) = inlay(s, w);
    let mut ls: Vec<String> = st
        .iter()
        .zip(&en)
        .map(|(&a, &b)| {
            cs[a..b]
                .iter()
                .collect::<String>()
                .trim_end_matches(' ')
                .to_string()
        })
        .collect();
    let cur = cur.min(cs.len());
    let mut cy = st.iter().rposition(|&a| a <= cur).unwrap_or(0);
    let mut cx: usize = cs[st[cy]..cur]
        .iter()
        .map(|&c| c.width().unwrap_or(0))
        .sum();
    if cx >= w {
        if cur == cs.len() {
            ls.push(String::new());
            cy = ls.len() - 1;
            cx = 0;
        } else {
            cx = w - 1;
        }
    }
    (ls, (cx, cy))
}

/// Splits a bar of `n` cells for `p` percent into the filled part (`on`) and the empty part (`off`).
pub fn bar(p: f32, n: usize, on: &str, off: &str) -> (String, String) {
    let k = ((p / 100.0) * n as f32).round() as usize;
    let k = k.min(n);
    (on.repeat(k), off.repeat(n - k))
}

/// One line of width `w` with `l` at the left and `r` at the right.
pub fn lr(l: Vec<Span<'static>>, r: Vec<Span<'static>>, w: usize) -> Line<'static> {
    let lw: usize = l.iter().map(|s| s.content.width()).sum();
    let rw: usize = r.iter().map(|s| s.content.width()).sum();
    let mut v = l;
    v.push(Span::raw(" ".repeat(w.saturating_sub(lw + rw).max(1))));
    v.extend(r);
    Line::from(v)
}

/// Display width of the spans `v` in terminal cells.
pub fn sw(v: &[Span]) -> usize {
    v.iter().map(|s| s.content.width()).sum()
}

/// Puts the terminal cursor at (`x`, `y`) and remembers it in `a`.
pub fn setcur(f: &mut Frame, a: &App, x: u16, y: u16) {
    f.set_cursor_position((x, y));
    a.cp.set(Some((x, y)));
}

/// Style with foreground colour `c`.
pub fn st(c: Color) -> Style {
    Style::new().fg(c)
}

/// A screen of the interface: it draws a frame from the app state and can handle keys.
pub trait V {
    /// Draws one frame from the app state `a`.
    fn draw(&mut self, f: &mut Frame, a: &App);
    /// Handles a key before the app does; returns `true` when it was used.
    fn key(&mut self, _k: &KeyEvent, _a: &mut App) -> bool {
        false
    }
    /// Sets up the demo scenario named `_s` in `_a`, for `--dump` screenshots.
    fn scen(&mut self, _s: &str, _a: &mut App) {}
}

/// Runs the interface with the screen `v` until the user quits.
///
/// Also handles `--version` and `--dump` on the command line.
///
/// # Errors
///
/// Returns any I/O error from setting up or restoring the terminal, drawing, reading events, or writing the `--dump` files.
pub fn run<T: V + Clone>(mut v: T) -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "--version" || args[1] == "-V") {
        println!("noname {} ({})", upd::VERSION, upd::short());
        return Ok(());
    }
    if args.len() > 1 && args[1] == "--dump" {
        return dump(&mut v, &args[2..]);
    }
    enable_raw_mode()?;
    TERM_ON.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let _ = write!(out, "\x1b[22;0t");
    let _ = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let _ = execute!(out, ratatui::crossterm::event::EnableBracketedPaste);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |p| {
        let _ = restore();
        sys::opacity_back();
        sys::log_panic_to(&sys::panic_log(), &p.to_string());
        eprintln!("noname: panic saved to {}", sys::panic_log().display());
        hook(p)
    }));
    if std::env::var("NONAME_MOCK").is_err() {
        if opac() < 100 {
            sys::opacity_apply(opac());
        } else {
            sys::opacity_back();
        }
    }
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    let r = lp(&mut term, &mut v);
    restore()?;
    sys::opacity_back();
    match r {
        Err(e) => {
            eprintln!("noname: {e}");
            std::process::exit(1);
        }
        Ok(sid) => {
            if !sid.is_empty() {
                println!("resume this session:  noname -r {sid}");
            }
            Ok(())
        }
    }
}

/// Runs the UI; returns the Claude session id (empty when there is none) so it can be shown after exit.
struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, b: &[u8]) {
        for c in b.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.write_u64(i as u64)
    }
    fn write_u16(&mut self, i: u16) {
        self.write_u64(i as u64)
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64)
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64)
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

fn lp<T: V>(term: &mut Terminal<CrosstermBackend<io::Stdout>>, v: &mut T) -> io::Result<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let oc = args.iter().any(|x| x == "--opencode")
        || std::env::var("NONAME_ENGINE").is_ok_and(|x| x == "opencode");
    let ag = !oc
        && (args.iter().any(|x| x == "--agy" || x == "--antigravity")
            || std::env::var("NONAME_ENGINE").is_ok_and(|x| x == "agy" || x == "antigravity"));
    let cx = oc
        || ag
        || args.iter().any(|x| x == "--codex")
        || std::env::var("NONAME_ENGINE").is_ok_and(|x| x == "codex");
    let args: Vec<String> = args
        .into_iter()
        .filter(|x| x != "--codex" && x != "--opencode" && x != "--agy" && x != "--antigravity")
        .collect();
    let mut a = if std::env::var("NONAME_MOCK").is_ok() {
        App::new()
    } else if oc {
        App::opencode(args)?
    } else if ag {
        App::agy(args)?
    } else if cx {
        App::codex(args)?
    } else {
        App::live(args)?
    };
    let mut lt = String::new();
    let mut pb = false;
    let mut pv: Option<u64> = None;
    let mut sk = 0u32;
    let shot = std::env::var("NONAME_SHOT").ok();
    sys::on_close(bye);
    sys::catch_ctrlc(true);
    upd::cleanup();
    if std::env::var("NONAME_MOCK").is_err()
        && upd::managed().is_none()
        && upd::enabled()
        && upd::due()
    {
        a.upcheck(false);
    }
    while !a.quit {
        a.cp.set(None);
        term.autoresize()?;
        {
            let mut f = term.get_frame();
            v.draw(&mut f, &a);
            let tx = a.txt.borrow();
            selhl(f.buffer_mut(), a.msel.or_else(|| a.cvis()), tx.as_ref());
        }
        let cp = a.cp.get();
        let hs = {
            use std::hash::{Hash, Hasher};
            let mut h = Fx(0);
            term.current_buffer_mut().hash(&mut h);
            cp.hash(&mut h);
            h.finish()
        };
        if pv == Some(hs) && shot.is_none() {
            term.current_buffer_mut().reset();
            sk += 1;
        } else {
            sk = 0;
            let _ = ratatui::crossterm::queue!(
                io::stdout(),
                ratatui::crossterm::terminal::BeginSynchronizedUpdate,
                ratatui::crossterm::cursor::Hide
            );
            if let Some(p) = &shot {
                let k = if p.contains("{}") { 3 } else { 15 };
                if a.t % k == 0 {
                    let f = p.replace("{}", &format!("{:05}", a.t / 3));
                    let _ = std::fs::write(f, html(term.current_buffer_mut(), cp));
                }
            }
            term.flush()?;
            match cp {
                Some(c) => {
                    term.show_cursor()?;
                    term.set_cursor_position(c)?;
                }
                None => term.hide_cursor()?,
            }
            pv = Some(hs);
            term.swap_buffers();
            ratatui::backend::Backend::flush(term.backend_mut())?;
            let _ = execute!(
                io::stdout(),
                ratatui::crossterm::terminal::EndSynchronizedUpdate
            );
        }
        if pb && !a.busy {
            a.dn = true;
        }
        pb = a.busy;
        let tt = ttl(&a);
        if tt != lt {
            let _ = execute!(io::stdout(), ratatui::crossterm::terminal::SetTitle(&tt));
            lt = tt;
        }
        if a.edit {
            a.edit = false;
            if let Some(t) = editor(&a.inp) {
                a.inp = t.trim_end_matches('\n').to_string();
                a.cur = a.inp.chars().count();
            }
            term.clear()?;
            pv = None;
        }
        if sys::ctrlc() {
            a.key(KeyEvent::new(KeyCode::Char('c'), Km::CONTROL));
        }
        let slow = sk >= 15 && !a.seld;
        let t0 = Instant::now();
        let dl = t0 + Duration::from_millis(if slow { 250 } else { 33 });
        while event::poll(dl.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let k = altgr(k);
                    a.dn = false;
                    if !a.seld {
                        a.msel = None;
                        a.csel = None;
                    }
                    if !v.key(&k, &mut a) {
                        a.key(k)
                    }
                }
                Event::Paste(s)
                    if a.rw.is_none()
                        && a.ns.is_none()
                        && a.pp.is_none()
                        && a.tp.is_none()
                        && a.bp.is_none() =>
                {
                    a.paste(s)
                }
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let (x, y) = (m.column, m.row);
                        let n = if a.lc.0.elapsed() < Duration::from_millis(400)
                            && (a.lc.1, a.lc.2) == (x, y)
                        {
                            a.lc.3 % 3 + 1
                        } else {
                            1
                        };
                        a.lc = (Instant::now(), x, y, n);
                        a.msel = None;
                        a.csel = None;
                        match a.cpos(x, y) {
                            Some(p) => {
                                a.csel = match n {
                                    1 => Some((p, p)),
                                    2 => a.word(p),
                                    _ => Some(((p.0, 0), (p.0, u16::MAX))),
                                }
                            }
                            None => a.msel = Some((x, y, x, y)),
                        }
                        a.seld = true;
                        a.my = (x, y);
                    }
                    MouseEventKind::Drag(MouseButton::Left) if a.seld => {
                        a.my = (m.column, m.row);
                        if a.csel.is_some() {
                            let h = a.cclamp(m.column, m.row);
                            if let Some(s) = &mut a.csel {
                                s.1 = h;
                            }
                        } else if let Some(s) = &mut a.msel {
                            s.2 = m.column;
                            s.3 = m.row;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) if a.seld => {
                        a.seld = false;
                        if let Some((p0, p1)) = a.csel {
                            if p0 == p1 && a.lc.3 == 1 {
                                a.csel = None;
                                a.click(m.column, m.row);
                            } else {
                                let t = a.ctext();
                                if t.trim().is_empty() {
                                    a.csel = None;
                                } else {
                                    let n = t.chars().count();
                                    let sys = copy(&t);
                                    a.clip = t;
                                    a.toast(if sys {
                                        format!("copied {n} chars")
                                    } else {
                                        format!(
                                            "copied {n} chars · no wl-copy/xclip{}",
                                            if rclick() {
                                                ", right-click pastes here"
                                            } else {
                                                ""
                                            }
                                        )
                                    });
                                }
                            }
                        } else {
                            match a.msel {
                                Some((x0, y0, x1, y1)) if (x0, y0) == (x1, y1) => {
                                    a.msel = None;
                                    a.click(m.column, m.row);
                                }
                                Some(s) => {
                                    let t = a
                                        .txt
                                        .borrow()
                                        .as_ref()
                                        .map(|b| seltext(b, s))
                                        .unwrap_or_default();
                                    if t.trim().is_empty() {
                                        a.msel = None;
                                    } else {
                                        copy(&t);
                                        a.toast("copied");
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                    // alt + wheel: up more solid, down more see-through (ctrl + wheel is the terminal's zoom)
                    MouseEventKind::ScrollUp
                        if m.modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::ALT) =>
                    {
                        a.opacstep(false, false)
                    }
                    MouseEventKind::ScrollDown
                        if m.modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::ALT) =>
                    {
                        a.opacstep(true, false)
                    }
                    MouseEventKind::Down(MouseButton::Right)
                        if rclick()
                            && a.rw.is_none()
                            && a.ns.is_none()
                            && a.pp.is_none()
                            && a.tp.is_none()
                            && a.bp.is_none() =>
                    {
                        a.msel = None;
                        // same as ctrl+v (images and copied files attach), with noname's own copy
                        // as fallback when there is no clipboard tool
                        a.clipv_any();
                    }
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        if {
                            let (y, h, _) = a.ig.get();
                            h > 1 && m.row >= y && m.row < y + h
                        } =>
                    {
                        let up = m.kind == MouseEventKind::ScrollUp;
                        a.vmove(up);
                    }
                    MouseEventKind::ScrollUp => {
                        a.msel = None;
                        a.scr = (a.scr + 3).min(a.smax.get())
                    }
                    MouseEventKind::ScrollDown => {
                        a.msel = None;
                        a.scr = a.scr.saturating_sub(3)
                    }
                    _ => {}
                },
                _ => {}
            }
            if a.quit || slow {
                break;
            }
        }
        if a.seld && a.csel.is_some() {
            let (yt, n, _, _) = a.cg.get();
            let y = a.my.1;
            let out = y < yt || (n > 0 && (y - yt) as usize >= n);
            if y < yt && a.scr < a.smax.get() {
                a.scr += 1;
            } else if out && y >= yt && a.scr > 0 {
                a.scr -= 1;
            }
            if out {
                let h = a.cclamp(a.my.0, a.my.1);
                if let Some(s) = &mut a.csel {
                    s.1 = h;
                }
            }
        }
        if let Ok(cy) = std::env::var("NONAME_BGCYCLE") {
            let v: Vec<&str> = cy.split(',').collect();
            if a.t % 30 == 0 && !v.is_empty() {
                a.bgw = bg::canon(v[(a.t / 30) as usize % v.len()]);
            }
        }
        for _ in 0..(t0.elapsed().as_millis() / 33).clamp(1, 10) {
            a.step();
        }
        if std::env::var("NONAME_MOCK").is_err() {
            let sid = a.session_id();
            if let Ok(mut s) = SID.lock() {
                if *s != sid {
                    *s = sid;
                }
            }
        }
    }
    // give the terminal back before stopping claude (that can take a second), so a
    // second ctrl+c or mouse movement in that time does not land in a half-restored screen
    sys::catch_ctrlc(false);
    let _ = restore();
    Ok(if std::env::var("NONAME_MOCK").is_ok() {
        String::new()
    } else {
        a.session_id()
    })
}

/// Session id for the exit message, also when noname is ended by a signal.
static SID: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Signal handler (ctrl+c as a real SIGINT, SIGTERM, terminal closed): restore the
/// terminal first, otherwise mouse tracking stays on and the shell fills with mouse codes.
fn bye() {
    // hold stdout until the process exits, so the drawing loop cannot paint over the shell
    let o = io::stdout();
    let _g = o.lock();
    let _ = restore();
    sys::opacity_back();
    if let Ok(s) = SID.lock() {
        if !s.is_empty() {
            let _ = writeln!(io::stdout(), "resume this session:  noname -r {s}");
        }
    }
    live::killall();
}

fn selnorm(s: (u16, u16, u16, u16)) -> ((u16, u16), (u16, u16)) {
    let (a, b) = ((s.1, s.0), (s.3, s.2));
    if a <= b { (a, b) } else { (b, a) }
}

fn selhl(
    b: &mut ratatui::buffer::Buffer,
    s: Option<(u16, u16, u16, u16)>,
    t: Option<&ratatui::buffer::Buffer>,
) {
    let Some(s) = s else { return };
    if (s.0, s.1) == (s.2, s.3) {
        return;
    }
    let ((y0, x0), (y1, x1)) = selnorm(s);
    let ar = b.area;
    for y in y0..=y1.min(ar.height.saturating_sub(1)) {
        let xa = if y == y0 { x0 } else { 0 };
        let xb = if y == y1 {
            x1
        } else {
            ar.width.saturating_sub(1)
        };
        for x in xa..=xb.min(ar.width.saturating_sub(1)) {
            let c = &mut b[(x, y)];
            if let Some(t) = t.filter(|t| t.area == ar) {
                *c = t[(x, y)].clone();
            }
            c.set_style(c.style().add_modifier(ratatui::style::Modifier::REVERSED));
        }
    }
}

fn seltext(b: &ratatui::buffer::Buffer, s: (u16, u16, u16, u16)) -> String {
    let ((y0, x0), (y1, x1)) = selnorm(s);
    let ar = b.area;
    let mut ls: Vec<String> = vec![];
    for y in y0..=y1.min(ar.height.saturating_sub(1)) {
        let xa = if y == y0 { x0 } else { 0 };
        let xb = if y == y1 {
            x1
        } else {
            ar.width.saturating_sub(1)
        };
        let mut l = String::new();
        let mut skip = 0;
        for x in 0..=xb.min(ar.width.saturating_sub(1)) {
            let sy = b[(x, y)].symbol();
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = unicode_width::UnicodeWidthStr::width(sy).saturating_sub(1);
            if x >= xa {
                l.push_str(sy);
            }
        }
        ls.push(l.trim_end().to_string());
    }
    tidy(ls)
}

fn tidy(ls: Vec<String>) -> String {
    let ind = ls
        .iter()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut o: Vec<String> = ls
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.trim_start().to_string()
            } else {
                l.get(ind.min(l.len())..).unwrap_or("").to_string()
            }
        })
        .collect();
    while o.last().is_some_and(|l| l.is_empty()) {
        o.pop();
    }
    o.join("\n")
}

fn copy(t: &str) -> bool {
    let _ = write!(io::stdout(), "\x1b]52;c;{}\x07", img::b64(t.as_bytes()));
    let _ = io::stdout().flush();
    sys::setclip(t)
}

fn cur_label(q: &Option<Qa>, j: usize) -> String {
    q.as_ref()
        .map(|q| q.qs[q.i].opts[j].0.clone())
        .unwrap_or_default()
}

/// Names of the three optional footer bars, in the order of `bars`.
pub const BARS: [&str; 3] = ["context bar", "5h bar", "week bar"];
const BKEY: [&str; 3] = ["context", "5h", "week"];
static BSET: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

/// Which footer bars are on (context, 5h, week), read from the config once.
pub fn bars() -> [bool; 3] {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = BSET.load(Relaxed);
    if m == u8::MAX {
        let hid = std::fs::read_to_string(sys::cfgfile("bars")).unwrap_or_default();
        m = (0..3)
            .filter(|&i| !hid.split([',', '\n', ' ']).any(|x| x.trim() == BKEY[i]))
            .fold(0, |a, i| a | 1 << i);
        BSET.store(m, Relaxed);
    }
    [m & 1 != 0, m & 2 != 0, m & 4 != 0]
}

/// Turns footer bar `i` on or off and saves the choice.
pub fn setbar(i: usize, on: bool) {
    let mut b = bars();
    b[i] = on;
    BSET.store(
        (0..3).filter(|&j| b[j]).fold(0, |a, j| a | 1 << j),
        std::sync::atomic::Ordering::Relaxed,
    );
    let hid: Vec<&str> = (0..3).filter(|&j| !b[j]).map(|j| BKEY[j]).collect();
    let f = sys::cfgfile("bars");
    if let Some(d) = std::path::Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&f, hid.join(","));
}

/// Layout settings as (name, choices), indexed as in `lay`; the first choice is the default.
pub const LAYOUT: [(&str, &[&str]); 6] = [
    ("input", &["bottom", "top"]),
    (
        "usage",
        &[
            "bottom left",
            "bottom right",
            "top left",
            "top right",
            "off",
        ],
    ),
    ("model", &["top", "bottom", "off"]),
    (
        "title",
        &[
            "middle",
            "top left",
            "top center",
            "top right",
            "bottom right",
        ],
    ),
    ("title visibility", &["on start", "always", "off"]),
    (
        "location",
        &[
            "top left",
            "top center",
            "top right",
            "bottom left",
            "bottom center",
            "bottom right",
            "off",
        ],
    ),
];
static LSET: [std::sync::atomic::AtomicU8; 6] =
    [const { std::sync::atomic::AtomicU8::new(u8::MAX) }; 6];

fn layload() -> Vec<(String, String)> {
    std::fs::read_to_string(sys::cfgfile("layout"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Index of the chosen option of layout setting `i` in `LAYOUT`.
pub fn lay(i: usize) -> usize {
    use std::sync::atomic::Ordering::Relaxed;
    let m = LSET[i].load(Relaxed);
    if m != u8::MAX {
        return m as usize;
    }
    let (k, opts) = LAYOUT[i];
    let v = layload()
        .into_iter()
        .find(|x| x.0 == k)
        .and_then(|x| opts.iter().position(|o| *o == x.1))
        .unwrap_or(0);
    LSET[i].store(v as u8, Relaxed);
    v
}

/// Chooses option `v` for layout setting `i` and saves all layout settings.
pub fn setlay(i: usize, v: usize) {
    LSET[i].store(v as u8, std::sync::atomic::Ordering::Relaxed);
    let body: String = (0..LAYOUT.len())
        .map(|j| format!("{}={}\n", LAYOUT[j].0, LAYOUT[j].1[lay(j)]))
        .collect();
    let f = sys::cfgfile("layout");
    if let Some(d) = std::path::Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&f, body);
}

/// Path of the file that stores the default permission mode.
pub fn modefile() -> String {
    sys::cfgfile("permission")
}

/// Index in `MODES` for a mode name or alias, or `None` if it is not one.
pub fn modeidx(s: &str) -> Option<usize> {
    match s.trim().to_lowercase().as_str() {
        "default" | "ask" | "normal" => Some(0),
        "plan" => Some(1),
        "auto" | "acceptedits" | "accept-edits" => Some(2),
        "bypass" | "bypasspermissions" | "yolo" => Some(3),
        _ => None,
    }
}

static RCLICK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

/// Whether right-click paste is on.
pub fn rclick() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = RCLICK.load(Relaxed);
    if m == u8::MAX {
        m = (std::fs::read_to_string(sys::cfgfile("rclick"))
            .unwrap_or_default()
            .trim()
            == "on") as u8;
        RCLICK.store(m, Relaxed);
    }
    m == 1
}

/// Turns right-click paste on or off and saves the choice.
pub fn setrclick(on: bool) {
    RCLICK.store(on as u8, std::sync::atomic::Ordering::Relaxed);
    let f = sys::cfgfile("rclick");
    if let Some(d) = std::path::Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&f, if on { "on" } else { "off" });
}

/// The saved default effort, or `None` to leave it to Claude.
pub fn defeff() -> Option<&'static str> {
    let v = std::fs::read_to_string(sys::cfgfile("effort")).unwrap_or_default();
    EFF.iter().copied().find(|x| *x == v.trim())
}

/// Saves `e` as the default effort; `None` removes the setting.
pub fn setdefeff(e: Option<&str>) {
    let f = sys::cfgfile("effort");
    match e {
        Some(e) => {
            if let Some(d) = std::path::Path::new(&f).parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&f, e);
        }
        None => {
            let _ = std::fs::remove_file(&f);
        }
    }
}

/// Index in `MODES` of the saved default permission mode, if one is set.
pub fn defmode() -> Option<usize> {
    std::fs::read_to_string(modefile())
        .ok()
        .and_then(|x| modeidx(&x))
}

/// Path of the file that stores the chosen animated background.
pub fn bgfile() -> String {
    sys::cfgfile("background")
}

fn ttl(a: &App) -> String {
    let d = if a.stitle.is_empty() {
        a.cwd.rsplit('/').next().unwrap_or("").to_string()
    } else {
        a.stitle.clone()
    };
    let st = if a.ask.is_some() || a.qa.is_some() {
        "? "
    } else if a.busy {
        "● "
    } else if a.dn {
        "✓ "
    } else {
        ""
    };
    format!("{st}noname · {d}")
}

fn editor(s: &str) -> Option<String> {
    let f = std::env::temp_dir().join(format!("noname-prompt-{}.md", std::process::id()));
    std::fs::write(&f, s).ok()?;
    let ed = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else if sys::which("nano").is_some() {
                "nano".into()
            } else {
                "vi".into()
            }
        });
    let _ = restore();
    let mut v = ed.split_whitespace();
    let ok = std::process::Command::new(v.next().unwrap_or("vi"))
        .args(v)
        .arg(&f)
        .status()
        .is_ok_and(|x| x.success());
    // a Ctrl+C pressed in the editor belongs to the editor, not to noname
    let _ = sys::ctrlc();
    let _ = enable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, EnterAlternateScreen, EnableMouseCapture);
    // push the title again too: restore() popped it, and pops it once more on exit
    let _ = write!(out, "\x1b[22;0t");
    let _ = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let _ = execute!(out, ratatui::crossterm::event::EnableBracketedPaste);
    TERM_ON.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = if ok {
        std::fs::read_to_string(&f).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&f);
    r
}

/// Ticks (~33 ms) in which a second Ctrl+C exits: as long as the "press again" toast shows.
const CC_WIN: u64 = 60;

#[cfg(test)]
mod ctrlc_tests {
    use super::*;

    fn cc(a: &mut App) {
        a.key(KeyEvent::new(KeyCode::Char('c'), Km::CONTROL));
    }

    #[test]
    fn second_press_exits() {
        let mut a = App::new();
        a.t = 100;
        cc(&mut a);
        assert!(!a.quit);
        assert!(
            a.toast
                .as_ref()
                .is_some_and(|t| t.0.contains("again to exit"))
        );
        a.t += CC_WIN;
        cc(&mut a);
        assert!(a.quit);
    }

    #[test]
    fn first_press_clears_input_and_late_press_starts_over() {
        let mut a = App::new();
        a.t = 100;
        a.inp = "half a prompt".into();
        a.cur = 4;
        cc(&mut a);
        assert!(!a.quit);
        assert_eq!((a.inp.as_str(), a.cur), ("", 0));
        assert_eq!(a.hist.last().map(String::as_str), Some("half a prompt"));
        a.t += CC_WIN + 1;
        cc(&mut a);
        assert!(!a.quit);
        a.t += 1;
        cc(&mut a);
        assert!(a.quit);
    }

    #[test]
    fn first_press_interrupts_a_turn() {
        let mut a = App::new();
        a.busy = true;
        a.inp = "next".into();
        cc(&mut a);
        assert!(!a.quit && !a.busy);
        assert_eq!(a.inp, "next");
    }

    #[test]
    fn open_menu_needs_two_presses_too() {
        let mut a = App::new();
        a.ns = Some(0);
        a.inp = "keep".into();
        cc(&mut a);
        assert!(!a.quit);
        assert_eq!(a.inp, "keep");
        cc(&mut a);
        assert!(a.quit);
    }
}

/// True while noname has the terminal (raw mode, alternate screen, pushed title and keys).
static TERM_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Gives the terminal back. Runs once per take-over: a second call (end of the loop, then
/// run(), a signal or a panic) does nothing, so the title and keyboard stacks of the
/// shell around noname are not popped twice.
fn restore() -> io::Result<()> {
    if !TERM_ON.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    let mut o = io::stdout();
    let _ = execute!(o, PopKeyboardEnhancementFlags);
    let _ = execute!(o, ratatui::crossterm::event::DisableBracketedPaste);
    let _ = write!(o, "\x1b[23;0t");
    disable_raw_mode()?;
    execute!(
        o,
        LeaveAlternateScreen,
        DisableMouseCapture,
        ratatui::crossterm::cursor::Show
    )?;
    Ok(())
}

fn setup<T: V>(v: &mut T, a: &mut App, s: &str) {
    if let Some((x, n)) = s.split_once('+') {
        setup(v, a, x);
        for _ in 0..n.parse::<u32>().unwrap_or(0) {
            a.step()
        }
        return;
    }
    match s {
        "empty" => a.cmd("clear"),
        "theme" => {
            a.inp = "/theme".into();
            a.submit();
        }
        "run" => {
            a.inp = "make the swipe snap that smooth too".into();
            a.submit();
            for _ in 0..150 {
                a.step()
            }
        }
        "ask" => {
            a.n = 1;
            a.inp = "commit and push this".into();
            a.submit();
            for _ in 0..1500 {
                a.step();
                if a.ask.is_some() {
                    break;
                }
            }
        }
        "btw" => {
            a.inp = "/btw what is dt exactly?".into();
            a.submit();
            for _ in 0..200 {
                a.step()
            }
        }
        "go" => {
            a.n = 2;
            a.inp = "can it get smoother?".into();
            a.submit();
        }
        "fail" => {
            a.inp = "make the swipe snap that smooth too".into();
            a.submit();
            for _ in 0..3000 {
                a.step();
                if a.bs
                    .iter()
                    .any(|b| matches!(b, B::Tl(t) if t.st == St::Err))
                {
                    break;
                }
            }
        }
        "open" => {
            a.open = true;
            a.to = a.t;
        }
        "type" => {
            a.inp = "can you give the snap\nsome easing and commit right after".into();
            a.cur = a.inp.chars().count();
        }
        "todo" => a.stodo = true,
        "find" => {
            a.bs.push(B::T("docs are at https://docs.rs/ratatui/latest/ratatui/ and (see https://example.com/a?b=1).".into()));
            a.fs = Some(("frame".into(), 1));
        }
        "wrap" => {
            a.inp = "can you please refactor the swipe handler so that the momentum calculation lives in its own module and the snapping uses a spring with configurable stiffness, then write tests for edge cases like very fast flicks and tiny drags".into();
            a.cur = a.inp.chars().count();
        }
        "paste" => {
            a.inp = "look at this log ".into();
            a.cur = a.inp.chars().count();
            a.paste(
                (0..40)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            a.put("and fix it\nalso\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine words that go on and on until the line wraps around nicely");
        }
        "done" => {
            a.n = 2;
            a.inp = "can it get smoother?".into();
            a.submit();
            for _ in 0..2500 {
                a.step();
                if !a.busy {
                    break;
                }
            }
        }
        x => v.scen(x, a),
    }
}

fn hex(c: Color) -> Option<String> {
    Some(match c {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#22192f".into(),
        Color::Red => "#f38ba8".into(),
        Color::Green => "#a6e3a1".into(),
        Color::Blue => "#b497ff".into(),
        Color::Magenta => "#d0a9ff".into(),
        Color::Cyan => "#89dceb".into(),
        Color::Gray => "#d9d0ee".into(),
        Color::DarkGray => "#3a2b52".into(),
        Color::White => "#f7f2ff".into(),
        _ => "#e9defa".into(),
    })
}

fn css(c: &ratatui::buffer::Cell) -> String {
    let mut s = String::new();
    let m = c.modifier;
    let (fg, bg) = if m.contains(Modifier::REVERSED) {
        (
            if c.bg == Color::Reset {
                pal::BASE()
            } else {
                c.bg
            },
            if c.fg == Color::Reset {
                pal::FG()
            } else {
                c.fg
            },
        )
    } else {
        (c.fg, c.bg)
    };
    if let Some(h) = hex(fg) {
        s += &format!("color:{h};");
    }
    if let Some(h) = hex(bg) {
        s += &format!("background:{h};");
    }
    if m.contains(Modifier::BOLD) {
        s += "font-weight:700;";
    }
    if m.contains(Modifier::DIM) {
        s += "opacity:.55;";
    }
    if m.contains(Modifier::ITALIC) {
        s += "font-style:italic;";
    }
    if m.contains(Modifier::UNDERLINED) {
        s += "text-decoration:underline;";
    }
    if m.contains(Modifier::CROSSED_OUT) {
        s += "text-decoration:line-through;";
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html(b: &Buffer, cur: Option<(u16, u16)>) -> String {
    let a = b.area;
    let pb = std::env::var("NONAME_SHOT_BG").ok();
    let mut s = String::from(
        "<!doctype html><html><head><meta charset=utf-8><style>body{margin:0;background:#0d0a14}#bg{position:relative;display:inline-block;padding:14px 18px;background:radial-gradient(circle at 20% 30%,#5a3d8a 0,transparent 45%),radial-gradient(circle at 80% 70%,#2a4a7a 0,transparent 50%),linear-gradient(135deg,#2b1f45,#1b2440 50%,#3a2040)}#bg:before{content:'';position:absolute;inset:0;background:rgba(26,20,38,.7)}#t{position:relative;font:15px/19px 'CaskaydiaCove Nerd Font Mono',monospace;white-space:pre;color:#e9defa}.k{background:#b497ff;color:#1a1426}</style></head><body><div id=bg><div id=t>",
    );
    s += &format!(
        "<style>.k{{background:{};color:{}}}#t{{color:{}}}</style>",
        hex(pal::LI()).unwrap_or_default(),
        hex(pal::BASE()).unwrap_or_default(),
        hex(pal::FG()).unwrap_or_default()
    );
    if let Some(c) = &pb {
        s += &format!(
            "<style>body,#bg{{background:{c} !important}}#bg:before{{display:none}}</style>"
        );
    }
    for y in 0..a.height {
        let mut x = 0;
        let mut run = String::new();
        let mut rs = String::new();
        while x < a.width {
            let c = &b[(x, y)];
            let k = cur == Some((x, y));
            let cs = if k { "K".to_string() } else { css(c) };
            if cs != rs && !run.is_empty() {
                s += &span(&rs, &run);
                run.clear();
            }
            rs = cs;
            let sym = c.symbol();
            run += &esc(if sym.is_empty() { "" } else { sym });
            x += 1;
        }
        if !run.is_empty() {
            s += &span(&rs, &run);
        }
        s.push('\n');
    }
    s += "</div></div></body></html>";
    s
}

fn span(cs: &str, t: &str) -> String {
    if cs == "K" {
        format!("<span class=k>{t}</span>")
    } else if cs.is_empty() {
        t.to_string()
    } else {
        format!("<span style=\"{cs}\">{t}</span>")
    }
}

fn dump<T: V + Clone>(v0: &mut T, args: &[String]) -> io::Result<()> {
    let dir = args.first().cloned().unwrap_or_else(|| ".".into());
    let name = args.get(1).cloned().unwrap_or_else(|| "v".into());
    let sizes: Vec<(u16, u16)> = vec![(270, 50), (104, 32)];
    let mut sc: Vec<String> = [
        "idle", "empty", "run", "ask", "btw", "open", "type", "todo", "done",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    sc.extend(args.iter().skip(2).cloned());
    for (w, h) in sizes {
        for s in &sc {
            let mut a = App::new();
            let mut vv = v0.clone();
            let v = &mut vv;
            setup(v, &mut a, s);
            let mut term =
                Terminal::new(TestBackend::new(w, h)).expect("the test backend cannot fail");
            a.cp.set(None);
            term.draw(|f| v.draw(f, &a))
                .expect("the test backend cannot fail");
            let b = term.backend().buffer().clone();
            let mut txt = String::new();
            for y in 0..h {
                for x in 0..w {
                    txt += b[(x, y)].symbol();
                }
                txt += "\n";
            }
            let base = format!("{dir}/{name}-{s}-{w}");
            std::fs::write(format!("{base}.txt"), txt)?;
            std::fs::write(format!("{base}.html"), html(&b, a.cp.get()))?;
        }
    }
    Ok(())
}

const NZ: &[u8] = b"0123456789abcdefABCDEF$%@#";

/// Formats the end-of-turn recap, such as `12.4s  ·  18.2k new  ·  2.59M cached  ·  $0.08`: `new` = input + cache-creation + output, `cached` = cache reads (left out at 0); the weekly-limit share is added when `dw` is at least 0.5.
pub fn rcfmt(secs: f32, new: u64, cached: u64, usd: f64, dw: f32) -> String {
    let mut s = format!("{secs:.1}s  ·  {} new", free::short(new));
    if cached > 0 {
        s += &format!("  ·  {} cached", free::short(cached));
    }
    s += &format!("  ·  ${usd:.2}");
    if dw >= 0.5 {
        s += &format!("  ·  +{dw:.0}% week");
    }
    s
}

/// Smooth value from 0 to 1 that repeats every `per` ticks, at tick `t`.
pub fn pulse(t: u64, per: f32) -> f32 {
    ((t as f32 / per * std::f32::consts::TAU).sin() + 1.0) / 2.0
}

/// Mixes two numbers into one pseudo-random number.
pub fn hsh(a: u64, b: u64) -> u64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^ (x >> 32)
}

/// Pseudo-random glyph for position `i` at tick `t`, used for the noise effect.
pub fn nz(t: u64, i: u64) -> char {
    NZ[(hsh(t / 2, i) % NZ.len() as u64) as usize] as char
}

fn rgb(c: Color) -> (f32, f32, f32) {
    match c {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        _ => (233.0, 222.0, 250.0),
    }
}

/// Colour at position `f` (0 to 1) along the gradient made of `cs`.
pub fn grad(cs: &[Color], f: f32) -> Color {
    let f = f.clamp(0.0, 1.0) * (cs.len() - 1) as f32;
    let i = (f.floor() as usize).min(cs.len() - 2);
    let k = f - i as f32;
    let (a, b) = (rgb(cs[i]), rgb(cs[i + 1]));
    let m = |x: f32, y: f32| (x + (y - x) * k) as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn wave(i: usize, t: u64) -> f32 {
    let f = (i as f32 * 0.13 + t as f32 * 0.06) % 2.0;
    if f > 1.0 { 2.0 - f } else { f }
}

/// `n` noise glyphs for tick `t` coloured along the gradient `cs`; `seed` makes separate noise strings differ.
pub fn noise(n: usize, t: u64, seed: u64, cs: &[Color]) -> Vec<Span<'static>> {
    (0..n)
        .map(|i| {
            Span::styled(
                nz(t, seed * 131 + i as u64).to_string(),
                Style::new().fg(grad(cs, wave(i, t))),
            )
        })
        .collect()
}

/// Text `s` shown as the noise effect that settles into the real characters from left to right as `el` grows; settled characters use `done`.
/// `t` is the tick, `seed` varies the noise and `cs` is the gradient of the unsettled glyphs.
pub fn decode(
    s: &str,
    el: u64,
    t: u64,
    seed: u64,
    done: Style,
    cs: &[Color],
) -> Vec<Span<'static>> {
    let mut v: Vec<Span<'static>> = vec![];
    let mut run = String::new();
    for (i, c) in s.chars().enumerate() {
        if c == ' ' || el >= 2 + i as u64 {
            run.push(c);
            continue;
        }
        if !run.is_empty() {
            v.push(Span::styled(std::mem::take(&mut run), done));
        }
        v.push(Span::styled(
            nz(t, seed * 131 + i as u64).to_string(),
            Style::new().fg(grad(cs, wave(i, t))),
        ));
    }
    if !run.is_empty() {
        v.push(Span::styled(run, done));
    }
    v
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn recap_splits_cached() {
        assert_eq!(
            rcfmt(188.4, 14_200, 2_595_400, 0.86, 0.0),
            "188.4s  ·  14.2k new  ·  2.60M cached  ·  $0.86"
        );
        assert_eq!(
            rcfmt(1.0, 950, 0, 0.01, 2.0),
            "1.0s  ·  950 new  ·  $0.01  ·  +2% week"
        );
    }

    fn ev(c: char, m: Km) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    #[test]
    fn altgr_letters_are_typed() {
        // Polish AltGr letters as crossterm reports them on Windows: Ctrl+Alt (+Shift for capitals)
        let mut a = App::new();
        for (c, m) in [
            ('ą', Km::CONTROL | Km::ALT),
            ('ż', Km::CONTROL | Km::ALT),
            ('Ł', Km::CONTROL | Km::ALT | Km::SHIFT),
            ('@', Km::CONTROL | Km::ALT),
            ('€', Km::CONTROL | Km::ALT),
        ] {
            a.key(altgr(ev(c, m)));
        }
        assert_eq!(a.inp, "ążŁ@€");
        assert_eq!(
            altgr(ev('Ł', Km::CONTROL | Km::ALT | Km::SHIFT)).modifiers,
            Km::SHIFT
        );
    }

    #[test]
    fn real_shortcuts_stay_shortcuts() {
        for k in [
            ev('a', Km::CONTROL | Km::ALT),
            ev('7', Km::CONTROL | Km::ALT),
            ev(' ', Km::CONTROL | Km::ALT),
            ev('c', Km::CONTROL),
            ev('ą', Km::CONTROL),
            ev('b', Km::ALT),
        ] {
            assert_eq!(altgr(k), k);
        }
        let mut a = App::new();
        a.key(altgr(ev('a', Km::CONTROL | Km::ALT)));
        assert_eq!(a.inp, "");
    }
}

#[cfg(test)]
mod md_tests {
    use super::*;

    fn mdt() -> Md {
        let s = Style::default();
        Md {
            t: s,
            b: s,
            c: s,
            fe: s,
            kw: s,
            st: s,
            cm: s,
            nu: s,
            bul: "- ",
            bs: s,
            fpre: "  | ",
            fps: s,
        }
    }

    fn text(l: &Line) -> String {
        l.spans.iter().map(|x| x.content.as_ref()).collect()
    }

    #[test]
    fn long_code_lines_wrap() {
        let s = "```\n    gamescope -W 3440 -H 1440 -w 1920 -h 1080 -r 175 -f -S stretch --force-grab-cursor -- %command%\n```";
        let ls = md(s, 40, &mdt());
        assert!(ls.len() > 1);
        for l in &ls {
            assert!(text(l).width() <= 40, "too wide: {:?}", text(l));
            assert!(text(l).starts_with("  |     "));
        }
        let joined: Vec<String> = ls
            .iter()
            .map(|l| text(l).trim_start_matches("  | ").trim().to_string())
            .collect();
        assert_eq!(
            joined.join(" "),
            "gamescope -W 3440 -H 1440 -w 1920 -h 1080 -r 175 -f -S stretch --force-grab-cursor -- %command%"
        );
    }

    #[test]
    fn touching_segments_wrap_as_one_word() {
        let s = Style::default();
        let segs = vec![
            ("ab ".to_string(), s),
            ("https".to_string(), s),
            ("://".to_string(), s),
            ("x.io/abc".to_string(), s),
        ];
        let t: Vec<String> = wsegs(&segs, 12)
            .iter()
            .map(|l| l.iter().map(|x| x.content.as_ref()).collect())
            .collect();
        assert_eq!(t, ["ab", "https://x.io", "/abc"]);
        // first line wider than the rest
        let t: Vec<String> = wsegs2(&[("aaaa bbbb cccc".to_string(), s)], 9, 4)
            .iter()
            .map(|l| l.iter().map(|x| x.content.as_ref()).collect())
            .collect();
        assert_eq!(t, ["aaaa bbbb", "cccc"]);
    }

    #[test]
    fn short_code_lines_stay_whole() {
        let ls = md("```\n  let x = 1;\n\n```", 80, &mdt());
        assert_eq!(text(&ls[0]), "  |   let x = 1;");
    }
}
