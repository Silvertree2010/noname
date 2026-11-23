//! Platform glue: home folder, binary lookup, process tracking and cleanup, clipboard access and
//! local time, with one implementation per operating system.

use std::path::PathBuf;
use std::process::{Child, Command};

/// Writes `b` to a new file `name` in the temp folder that only this user can read (the
/// shared /tmp would make it 0644). A file of that name that is in the way is removed first;
/// when that fails (someone else's file or link), nothing is written.
pub fn tmp_write(name: &str, b: &[u8]) -> Option<PathBuf> {
    use std::io::Write;
    let f = std::env::temp_dir().join(name);
    let _ = std::fs::remove_file(&f);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(&f).and_then(|mut x| x.write_all(b)).ok()?;
    Some(f)
}

/// The home folder from `HOME` or `USERPROFILE`, or an empty string when neither is set.
pub fn home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

/// noname's config folder: `%APPDATA%\noname` on Windows, otherwise `noname` under `XDG_CONFIG_HOME` or `~/.config`.
pub fn cfgdir() -> String {
    #[cfg(windows)]
    if let Ok(a) = std::env::var("APPDATA") {
        return format!("{a}\\noname");
    }
    let b = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home()));
    format!("{b}/noname")
}

/// Path of the file `n` inside the config folder, written with the separator of this platform.
pub fn cfgfile(n: &str) -> String {
    let d = cfgdir();
    if cfg!(windows) {
        format!("{d}\\{n}")
    } else {
        format!("{d}/{n}")
    }
}

/// Path of the panic log: `noname\\panic.log` under `LOCALAPPDATA` on Windows, otherwise `noname/panic.log` under `XDG_CACHE_HOME` or `~/.cache`.
pub fn panic_log() -> PathBuf {
    #[cfg(windows)]
    if let Ok(a) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(a).join("noname").join("panic.log");
    }
    let b = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", home()));
    PathBuf::from(b).join("noname").join("panic.log")
}

/// Appends one panic report (Unix time, version, `what` and a backtrace) to the file `path`. Errors are ignored: the
/// program is already panicking and has nowhere to report them. The file is never trimmed (a panic is rare).
pub fn log_panic_to(path: &std::path::Path, what: &str) {
    use std::io::Write;
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "[unix {t}] noname {}\n{what}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
    }
}

/// The program `b` as a full path when `which` finds it, otherwise `b` itself so the system searches its own path.
pub fn bin(b: &str) -> std::ffi::OsString {
    which(b)
        .map(|p| p.into_os_string())
        .unwrap_or_else(|| b.into())
}

/// Finds the executable `b`: searches `PATH` plus the usual user install folders (cargo, `~/.local/bin`, Homebrew, snap).
#[cfg(unix)]
pub fn which(b: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let h = home();
    let mut ds: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect();
    for d in [
        format!("{h}/.cargo/bin"),
        format!("{h}/.local/bin"),
        format!("{h}/bin"),
        "/usr/local/bin".into(),
        "/usr/games".into(),
        "/usr/local/games".into(),
        "/opt/homebrew/bin".into(),
        "/home/linuxbrew/.linuxbrew/bin".into(),
        "/snap/bin".into(),
    ] {
        if !ds.contains(&d) {
            ds.push(d);
        }
    }
    ds.into_iter()
        .map(|d| std::path::Path::new(&d).join(b))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Finds the program `b`: searches `PATH` plus a few user install folders, trying the `PATHEXT` extensions.
#[cfg(windows)]
pub fn which(b: &str) -> Option<PathBuf> {
    let h = home();
    let mut ds: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect();
    let mut extra = vec![format!("{h}\\.local\\bin"), format!("{h}\\.cargo\\bin")];
    if let Ok(a) = std::env::var("APPDATA") {
        extra.push(format!("{a}\\npm"));
    }
    for d in extra {
        if !ds.contains(&d) {
            ds.push(d);
        }
    }
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|x| !x.is_empty())
        .map(|x| x.to_lowercase())
        .collect();
    let has_ext = std::path::Path::new(b).extension().is_some();
    for d in ds {
        let base = std::path::Path::new(&d).join(b);
        if has_ext && base.is_file() {
            return Some(base);
        }
        for e in &exts {
            let p = std::path::Path::new(&d).join(format!("{b}{e}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Prepares `c` so the child ends when noname does (parent-death signal on Linux) and runs in its own process group.
#[cfg(unix)]
pub fn guard(c: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec, so it may only call
    // async-signal-safe functions. prctl is a plain syscall and takes no locks or memory.
    unsafe {
        c.pre_exec(|| {
            #[cfg(target_os = "linux")]
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    c.process_group(0)
}

/// Prepares `c` so the child gets no console window and its own process group.
#[cfg(windows)]
pub fn guard(c: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000 | 0x0000_0200)
}

/// Does nothing on Unix; `guard` already puts the child in its own process group.
#[cfg(unix)]
pub fn adopt(_: &Child) {}

/// Adds the child to noname's job object so Windows ends it when noname exits.
#[cfg(windows)]
pub fn adopt(c: &Child) {
    use std::os::windows::io::AsRawHandle;
    let j = job();
    if j != 0 {
        // SAFETY: `j` is a valid job handle kept for the whole process (see `job`) and the
        // child handle is valid while `c` is borrowed; a failed call only leaves the child out of the job.
        unsafe {
            win::AssignProcessToJobObject(j as _, c.as_raw_handle() as _);
        }
    }
}

/// Asks process `id` and its process group to stop (SIGTERM).
#[cfg(unix)]
pub fn term(id: i32) {
    // SAFETY: kill(2) has no memory-safety preconditions. A negative pid signals a whole
    // process group; callers pass the pid of a child they spawned (from `Child::id`), never 0 or 1.
    unsafe {
        libc::kill(-id, libc::SIGTERM);
        libc::kill(id, libc::SIGTERM);
    }
}

/// Forcibly ends process `id` and its process group (SIGKILL).
#[cfg(unix)]
pub fn kill(id: i32) {
    // SAFETY: same as `term`: kill(2) only sends a signal.
    unsafe {
        libc::kill(-id, libc::SIGKILL);
        libc::kill(id, libc::SIGKILL);
    }
}

/// Ends process `id` and its children; on Windows this is the same as `kill`.
#[cfg(windows)]
pub fn term(id: i32) {
    kill(id)
}

/// Forcibly ends process `id` and its children with `taskkill`.
#[cfg(windows)]
pub fn kill(id: i32) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/PID", &id.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}

/// Ends the process at once with status 0, without running destructors. It is safe to call from signal-handling threads.
pub fn exit_now() -> ! {
    #[cfg(unix)]
    // SAFETY: _exit ends the process at once without running destructors or atexit handlers,
    // which is the point: it is also called from signal-handling threads.
    unsafe {
        libc::_exit(0)
    }
    #[cfg(windows)]
    std::process::exit(0)
}

/// Installs a handler for SIGHUP, SIGTERM and SIGINT: it runs the cleanup `f`, then exits (after two seconds at most).
#[cfg(unix)]
pub fn on_close(f: fn()) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    if let Ok(mut sg) = signal_hook::iterator::Signals::new([SIGHUP, SIGTERM, SIGINT]) {
        std::thread::spawn(move || {
            if sg.forever().next().is_some() {
                std::thread::spawn(move || {
                    let _ = std::panic::catch_unwind(f);
                    exit_now();
                });
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    exit_now();
                });
                let _ = sg.forever().next();
                exit_now();
            }
        });
    }
}

/// Registers a console control handler that runs the cleanup `f` and then exits. Ctrl+C is only passed to the main loop while that is enabled.
#[cfg(windows)]
pub fn on_close(f: fn()) {
    let _ = job();
    let _ = HANDLER.set(f);
    // SAFETY (callers): Windows calls this with a control-event code on its own thread; it only
    // touches atomics, a OnceLock and `exit_now`, so it has no preconditions of its own.
    unsafe extern "system" fn h(ty: u32) -> i32 {
        // Ctrl+C does not close noname at once: the main loop picks it up through ctrlc()
        // and handles it like the key (first press interrupts, second press exits).
        if ty == win::CTRL_C_EVENT && CATCH.load(std::sync::atomic::Ordering::SeqCst) {
            CTRLC.store(true, std::sync::atomic::Ordering::SeqCst);
            return 1;
        }
        if let Some(f) = HANDLER.get() {
            f();
        }
        exit_now()
    }
    // SAFETY: `h` is a plain function that lives for the whole process, so the pointer stays valid.
    unsafe {
        win::SetConsoleCtrlHandler(Some(h), 1);
    }
}

#[cfg(windows)]
static HANDLER: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

#[cfg(windows)]
static CTRLC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
static CATCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While on (the main loop runs), a Ctrl+C signal is passed on to ctrlc() instead of
/// closing noname. Off again while noname shuts down, so Ctrl+C there still ends it.
pub fn catch_ctrlc(on: bool) {
    #[cfg(windows)]
    CATCH.store(on, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(windows))]
    let _ = on;
}

/// True once after the console sent a Ctrl+C signal (Windows). In raw mode Ctrl+C normally
/// comes in as a key; the signal only arrives when the console still processes it.
pub fn ctrlc() -> bool {
    #[cfg(windows)]
    return CTRLC.swap(false, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(windows))]
    false
}

#[cfg(windows)]
fn job() -> isize {
    static J: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
    // SAFETY: both calls get a zeroed, correctly sized struct and a handle checked for null; the
    // handle is never closed, so it lives as long as the process (that is what kills the children).
    *J.get_or_init(|| unsafe {
        let j = win::CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if j.is_null() {
            return 0;
        }
        let mut info: win::JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = win::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        win::SetInformationJobObject(
            j,
            win::JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        );
        j as isize
    })
}

#[cfg(windows)]
mod win {
    pub(super) use windows_sys::Win32::System::Console::{CTRL_C_EVENT, SetConsoleCtrlHandler};
    pub(super) use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
}

/// Opens `path` at `line` (may be empty) with the command in `NONAME_OPEN` (`{}` is the path, `{line}` the line) or else the system opener. The opener runs detached.
pub fn open(path: &str, line: &str) {
    let mut c = if let Ok(t) = std::env::var("NONAME_OPEN") {
        let mut v = open_cmd(&t, path, line).into_iter();
        let mut c = Command::new(v.next().unwrap_or_default());
        c.args(v);
        c
    } else if runs(std::path::Path::new(path)) {
        reveal(path);
        return;
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        c.arg(path.replace('/', "\\"));
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    detach(&mut c);
}

/// The `NONAME_OPEN` command for `path` at `line` as program + arguments. The template is
/// split at spaces first and `{}` / `{line}` are filled in after, so a path with spaces stays
/// one argument; with no `{}` the path is added as the last argument.
fn open_cmd(t: &str, path: &str, line: &str) -> Vec<String> {
    let ln = if line.is_empty() { "1" } else { line };
    let mut v: Vec<String> = t
        .split_whitespace()
        .map(|w| w.replace("{line}", ln).replace("{}", path))
        .collect();
    if !t.contains("{}") {
        v.push(path.to_string());
    }
    v
}

/// Opens a web link in the default browser (NONAME_OPEN is for files, so it is not used here).
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return;
    }
    let mut c = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // not `cmd /c start`: cmd would read & | ^ in the url as its own syntax
        // (and run what follows), while the link text comes from the model
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    detach(&mut c);
}

/// `path:line` or `path:line:col` → (path, line); a path with no number gives "".
pub fn split_line(s: &str) -> (&str, &str) {
    let num = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
    match s.rsplit_once(':') {
        Some((a, b)) if num(b) => match a.rsplit_once(':') {
            Some((p, l)) if num(l) => (p, l),
            _ => (a, b),
        },
        _ => (s, ""),
    }
}

/// A path as typed in text made absolute: `~` is the home folder, a relative path
/// starts in the working folder.
pub fn resolve(p: &str) -> PathBuf {
    if p == "~" {
        return PathBuf::from(home());
    }
    if let Some(r) = p.strip_prefix("~/") {
        return PathBuf::from(home()).join(r);
    }
    let q = std::path::Path::new(p);
    if q.is_absolute() {
        q.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(q)
    }
}

thread_local! {
    // path → (exists, when checked); the screen is drawn many times a second
    static SEEN: std::cell::RefCell<std::collections::HashMap<String, (bool, std::time::Instant)>> = std::cell::RefCell::new(Default::default());
}

/// Whether a path named in text exists. Answers are kept for 2 s, so a file the
/// model has just written turns into a link within that time.
pub fn exists(p: &str) -> bool {
    let now = std::time::Instant::now();
    SEEN.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(&(e, t)) = m.get(p) {
            if now.duration_since(t).as_secs() < 2 {
                return e;
            }
        }
        if m.len() > 4096 {
            m.clear();
        }
        let e = !remote(p) && stat(resolve(p)).unwrap_or(false);
        m.insert(p.to_string(), (e, now));
        e
    })
}

const AUTO: [&str; 6] = ["/net", "/network", "/afs", "/misc", "/smb", "/nfs"];

fn norm(p: &std::path::Path) -> String {
    let mut o = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                o.pop();
            }
            std::path::Component::CurDir => {}
            c => o.push(c),
        }
    }
    o.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn share(s: &str) -> Option<String> {
    let r = s.strip_prefix("//")?;
    Some(r.split('/').take(2).collect::<Vec<_>>().join("/"))
}

fn remote(p: &str) -> bool {
    let t = p.replace('\\', "/");
    if t.starts_with("//") {
        return true;
    }
    let s = norm(&resolve(p));
    if AUTO
        .iter()
        .any(|r| s == *r || s.strip_prefix(r).is_some_and(|x| x.starts_with('/')))
    {
        return true;
    }
    let Some(h) = share(&s) else { return false };
    let own = |b: PathBuf| share(&norm(&b)).as_ref() == Some(&h);
    !(own(std::env::current_dir().unwrap_or_default()) || own(PathBuf::from(home())))
}

type Rq = (PathBuf, std::sync::mpsc::Sender<bool>);
type Sw = (
    Option<std::sync::mpsc::Sender<Rq>>,
    Option<std::sync::mpsc::Receiver<bool>>,
);

thread_local! {
    static ST: std::cell::RefCell<Sw> = Default::default();
}

fn stat(p: PathBuf) -> Option<bool> {
    use std::sync::mpsc;
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(r) = &s.1 {
            if r.try_recv() == Err(mpsc::TryRecvError::Empty) {
                return None;
            }
            s.1 = None;
        }
        if s.0.is_none() {
            let (tx, rx) = mpsc::channel::<Rq>();
            std::thread::Builder::new()
                .name("stat".into())
                .spawn(move || {
                    for (p, r) in rx {
                        let _ = r.send(p.exists());
                    }
                })
                .ok()?;
            s.0 = Some(tx);
        }
        let (tx, rx) = mpsc::channel();
        if s.0.as_ref()?.send((p, tx)).is_err() {
            s.0 = None;
            return None;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(e) => Some(e),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                s.1 = Some(rx);
                None
            }
            Err(_) => {
                s.0 = None;
                None
            }
        }
    })
}

/// Opens a folder in a new file manager window (on GNOME a new Files window, even
/// when one is already open).
pub fn open_dir(path: &str) {
    let mut c = if cfg!(target_os = "macos") && runs(std::path::Path::new(path)) {
        reveal(path);
        return;
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        c.arg(path.replace('/', "\\"));
        c
    } else if let Some(n) = which("nautilus") {
        let mut c = Command::new(n);
        c.args(["--new-window", path]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    detach(&mut c);
}

const RUN: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "btm",
    "ps1",
    "psm1",
    "psd1",
    "ps1xml",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "wsc",
    "msi",
    "msp",
    "mst",
    "lnk",
    "url",
    "scr",
    "pif",
    "hta",
    "cpl",
    "msc",
    "jar",
    "reg",
    "inf",
    "scf",
    "appref-ms",
    "application",
    "gadget",
    "settingcontent-ms",
    "library-ms",
    "search-ms",
    "searchconnector-ms",
    "appx",
    "appxbundle",
    "msix",
    "msixbundle",
    "appinstaller",
    "diagcab",
    "chm",
    "xll",
    "website",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "app",
    "command",
    "tool",
    "terminal",
    "workflow",
    "action",
    "pkg",
    "mpkg",
    "fileloc",
    "prefpane",
    "saver",
    "kext",
    "mobileconfig",
    "shortcut",
    "desktop",
    "appimage",
    "deb",
    "rpm",
    "flatpakref",
    "snap",
];

const SCR: &[&str] = &[
    "py", "pyw", "pyz", "pyc", "pl", "rb", "sh", "bash", "zsh", "tcl",
];

fn ext(p: &std::path::Path) -> Option<String> {
    let n = p.file_name()?.to_string_lossy().to_lowercase();
    let n = n.trim_end_matches(['.', ' ']);
    n.rsplit_once('.').map(|x| x.1.to_string())
}

fn launcher(p: &std::path::Path) -> bool {
    ext(p).is_some_and(|e| {
        RUN.contains(&e.as_str())
            || (cfg!(any(windows, target_os = "macos")) && SCR.contains(&e.as_str()))
            || (cfg!(windows)
                && std::env::var("PATHEXT").is_ok_and(|v| {
                    v.split(';')
                        .any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(&e))
                }))
    })
}

fn runs(p: &std::path::Path) -> bool {
    launcher(p) || std::fs::canonicalize(p).is_ok_and(|c| launcher(&c)) || exe_bit(p)
}

#[cfg(unix)]
fn exe_bit(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn exe_bit(_: &std::path::Path) -> bool {
    false
}

#[cfg(not(windows))]
fn uri(b: &[u8]) -> String {
    let mut o = String::from("file://");
    for &c in b {
        if c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'.' | b'_' | b'~') {
            o.push(c as char);
        } else {
            o.push_str(&format!("%{c:02X}"));
        }
    }
    o
}

fn quiet(c: &mut Command) -> &mut Command {
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
}

fn reveal(path: &str) {
    if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.args(["-R", path]);
        quiet(&mut c);
        detach(&mut c);
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.replace('/', "\\")));
        quiet(&mut c);
        detach(&mut c);
    }
    #[cfg(not(windows))]
    {
        let p = std::path::Path::new(path);
        let up = p
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(d) = which("dbus-send") else {
            open_dir(&up);
            return;
        };
        let mut c = Command::new(d);
        c.args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{}", uri(p.as_os_str().as_encoded_bytes())),
            "string:",
        ]);
        quiet(&mut c);
        std::thread::spawn(move || {
            if !c.status().is_ok_and(|s| s.success()) {
                open_dir(&up);
            }
        });
    }
}

/// Starts an opener and waits for it on a thread, so a finished opener (xdg-open exits once
/// the app is up) is reaped instead of staying a zombie for as long as noname runs.
fn detach(c: &mut Command) {
    if let Ok(mut ch) = c.spawn() {
        std::thread::spawn(move || {
            let _ = ch.wait();
        });
    }
}

/// The current local month, counted from 0 (January) to 11 (December).
pub fn month() -> i32 {
    #[cfg(unix)]
    // SAFETY: `t` and `tm` are valid for the call; an all-zero libc::tm is a valid value, and
    // localtime_r (unlike localtime) writes only into the buffer we pass.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_mon
    }
    #[cfg(windows)]
    // SAFETY: `st` is a valid, zeroed SYSTEMTIME that GetLocalTime fills in.
    unsafe {
        let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
        st.wMonth as i32 - 1
    }
}

/// xclip goes through XWayland and needs no window focus. wl-copy/wl-paste on GNOME (no data-control
/// protocol) open a hidden window and wait for focus, which mutter does not always give: then they hang.
/// So when an X display and xclip are both there, xclip is used first.
pub fn xclip_first() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) && which("xclip").is_some()
}

/// Puts text on the system clipboard. Returns false when no clipboard tool was found or `clip` failed (Windows),
/// so the caller can say that only the terminal's own clipboard (OSC 52) was tried.
pub fn setclip(t: &str) -> bool {
    if cfg!(windows) {
        return winclip(t);
    }
    let c: Option<(&str, &[&str])> = if cfg!(target_os = "macos") {
        Some(("pbcopy", &[]))
    } else if std::env::var("WAYLAND_DISPLAY").is_ok()
        && which("wl-copy").is_some()
        && !xclip_first()
    {
        Some(("wl-copy", &[]))
    } else if which("xclip").is_some() {
        Some(("xclip", &["-selection", "clipboard"]))
    } else {
        None
    };
    let Some((p, a)) = c else {
        return false;
    };
    let Ok(mut ch) = Command::new(p)
        .args(a)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut i) = ch.stdin.take() {
        use std::io::Write;
        let _ = i.write_all(t.as_bytes());
    }
    std::thread::spawn(move || ch.wait());
    true
}

fn u16le(t: &str) -> Vec<u8> {
    std::iter::once(0xfeff)
        .chain(t.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn winclip(t: &str) -> bool {
    use std::io::Write;
    let Ok(mut ch) = Command::new("clip")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let w = ch
        .stdin
        .take()
        .is_some_and(|mut i| i.write_all(&u16le(t)).is_ok());
    ch.wait().is_ok_and(|s| s.success()) && w
}

#[derive(Clone)]
enum Tm {
    Px(String),
    Gt(String),
    Kt(String),
    Al(String),
}

fn tm() -> Option<Tm> {
    static T: std::sync::OnceLock<Option<Tm>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let e = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if let Some(p) = e("PTYXIS_PROFILE").filter(|p| ok(p)) {
            if which("gsettings").is_some() {
                return Some(Tm::Px(p));
            }
        }
        if let Some(k) = e("KITTY_WINDOW_ID").filter(|k| k.chars().all(|c| c.is_ascii_digit())) {
            if e("TMUX").is_none() || (e("KITTY_LISTEN_ON").is_some() && which("kitten").is_some())
            {
                return Some(Tm::Kt(k));
            }
        }
        if let Some(w) = e("ALACRITTY_WINDOW_ID").filter(|w| w.chars().all(|c| c.is_ascii_digit()))
        {
            if e("ALACRITTY_SOCKET").is_some() && which("alacritty").is_some() {
                return Some(Tm::Al(w));
            }
        }
        if e("GNOME_TERMINAL_SERVICE").is_some() && which("gsettings").is_some() {
            let id = gs(&["get", "org.gnome.Terminal.ProfilesList", "default"])?
                .trim_matches('\'')
                .to_string();
            if !id.is_empty() && ok(&id) {
                return Some(Tm::Gt(id));
            }
        }
        None
    })
    .clone()
}

fn gs(a: &[&str]) -> Option<String> {
    let o = Command::new("gsettings")
        .args(a)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn gpath(t: &Tm) -> String {
    match t {
        Tm::Px(p) => format!("org.gnome.Ptyxis.Profile:/org/gnome/Ptyxis/Profiles/{p}/"),
        Tm::Gt(p) => {
            format!("org.gnome.Terminal.Legacy.Profile:/org/gnome/terminal/legacy/profiles:/:{p}/")
        }
        _ => String::new(),
    }
}

fn gget(t: &Tm) -> Option<String> {
    let p = gpath(t);
    match t {
        Tm::Px(_) => gs(&["get", &p, "opacity"]),
        Tm::Gt(_) => Some(format!(
            "{} {}",
            gs(&["get", &p, "use-transparent-background"])?,
            gs(&["get", &p, "background-transparency-percent"])?
        )),
        _ => None,
    }
}

fn gput(t: &Tm, v: &str) -> bool {
    let p = gpath(t);
    match t {
        Tm::Px(_) => gs(&["set", &p, "opacity", v]).is_some(),
        Tm::Gt(_) => {
            let Some((on, pc)) = v.split_once(' ') else {
                return false;
            };
            gs(&["set", &p, "use-transparent-background", on]).is_some()
                && gs(&["set", &p, "background-transparency-percent", pc]).is_some()
        }
        _ => false,
    }
}

fn kt(id: &str, v: f32, tog: bool) -> bool {
    let to = std::env::var("KITTY_LISTEN_ON").unwrap_or_default();
    if !to.is_empty() && which("kitten").is_some() {
        let mut c = Command::new("kitten");
        c.args([
            "@",
            "--to",
            &to,
            "set-background-opacity",
            "-m",
            &format!("id:{id}"),
        ]);
        if tog {
            c.arg("--toggle");
        }
        c.arg(format!("{v:.2}"));
        return c
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    }
    use std::io::Write;
    let j = format!(
        r#"{{"cmd":"set-background-opacity","version":[0,14,2],"no_response":true,"payload":{{"opacity":{v:.2},"match_window":"id:{id}","toggle":{tog}}}}}"#
    );
    let mut o = std::io::stdout();
    write!(o, "\x1bP@kitty-cmd{j}\x1b\\")
        .and_then(|_| o.flush())
        .is_ok()
}

fn al(a: &[&str]) -> bool {
    Command::new("alacritty")
        .arg("msg")
        .args(a)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

static OPON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn origf(id: &str) -> String {
    cfgfile(&format!("opacity-orig-{id}"))
}

fn others(want: &str) -> bool {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return false;
    };
    rd.flatten().any(|e| {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            return false;
        };
        if pid == me
            || std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|c| c.trim() != "noname")
                .unwrap_or(true)
        {
            return false;
        }
        let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else {
            return false;
        };
        let vs: Vec<&[u8]> = env.split(|b| *b == 0).collect();
        let hit = vs.iter().any(|v| {
            *v == want.as_bytes() || (want.ends_with('=') && v.starts_with(want.as_bytes()))
        });
        hit && !vs.iter().any(|v| v.starts_with(b"NONAME_MOCK="))
    })
}

/// Says what a change of the window opacity affects in this terminal, or an empty string when the terminal is not supported.
pub fn opacity_scope() -> &'static str {
    match tm() {
        Some(Tm::Px(_)) | Some(Tm::Gt(_)) => "all tabs with this profile",
        Some(Tm::Kt(_)) if std::env::var("KITTY_LISTEN_ON").is_ok_and(|v| !v.is_empty()) => {
            "this kitty window"
        }
        Some(Tm::Kt(_)) => {
            "this kitty window, needs allow_remote_control and dynamic_background_opacity"
        }
        Some(Tm::Al(_)) => "this window",
        None => "",
    }
}

/// Sets the opacity of this terminal to `pct` percent and returns whether that worked. Where needed it saves the old setting first so it can be restored.
pub fn opacity_apply(pct: u8) -> bool {
    let Some(t) = tm() else { return false };
    let v = pct as f32 / 100.0;
    let ok = match &t {
        Tm::Px(id) | Tm::Gt(id) => {
            let f = origf(id);
            if std::fs::metadata(&f).is_err() {
                let Some(cur) = gget(&t) else { return false };
                if let Some(d) = std::path::Path::new(&f).parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(&f, cur);
            }
            match t {
                Tm::Px(_) => gput(&t, &format!("{v:.2}")),
                _ => gput(&t, &format!("true {}", 100 - pct.min(100))),
            }
        }
        Tm::Kt(id) => kt(id, v, false),
        Tm::Al(id) => al(&["config", "-w", id, &format!("window.opacity={v:.2}")]),
    };
    if ok {
        OPON.store(pct, std::sync::atomic::Ordering::SeqCst);
    }
    ok
}

/// Puts the terminal opacity back to what it was before `opacity_apply`.
pub fn opacity_restore() {
    let Some(t) = tm() else { return };
    let p = OPON.swap(0, std::sync::atomic::Ordering::SeqCst);
    match &t {
        Tm::Px(id) | Tm::Gt(id) => {
            let f = origf(id);
            if let Ok(s) = std::fs::read_to_string(&f) {
                if gput(&t, s.trim()) {
                    let _ = std::fs::remove_file(&f);
                }
            }
        }
        Tm::Kt(id) if p > 0 => {
            kt(id, p as f32 / 100.0, true);
        }
        Tm::Al(id) if p > 0 => {
            al(&["config", "-w", id, "--reset"]);
        }
        _ => {}
    }
}

/// Restores the opacity, unless another noname running in the same terminal profile still needs the changed value.
pub fn opacity_back() {
    let sh = match tm() {
        Some(Tm::Px(id)) => others(&format!("PTYXIS_PROFILE={id}")),
        Some(Tm::Gt(_)) => others("GNOME_TERMINAL_SERVICE="),
        Some(_) => false,
        None => return,
    };
    if !sh {
        opacity_restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_files_are_private_and_fresh() {
        let n = format!("noname-tmptest-{}", std::process::id());
        let f = tmp_write(&n, b"old").unwrap();
        let f2 = tmp_write(&n, b"new").unwrap();
        assert_eq!(std::fs::read(&f2).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&f).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn panic_report_is_appended_not_overwritten() {
        let d = std::env::temp_dir().join(format!("noname-panic-{}", std::process::id()));
        let f = d.join("sub").join("panic.log");
        log_panic_to(&f, "panicked at src/a.rs:1:2");
        log_panic_to(&f, "panicked at src/b.rs:3:4");
        let s = std::fs::read_to_string(&f).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert!(s.contains("src/a.rs:1:2") && s.contains("src/b.rs:3:4"));
        assert_eq!(s.matches("] noname ").count(), 2);
    }

    #[test]
    fn open_command_keeps_a_path_with_spaces_whole() {
        let p = "/home/u/project Solid Craft/a.rs";
        assert_eq!(
            open_cmd("code -g {}:{line}", p, "12"),
            ["code", "-g", "/home/u/project Solid Craft/a.rs:12"]
        );
        // no `{}`: the path goes last; no line: line 1
        assert_eq!(open_cmd("nvim +{line}", p, ""), ["nvim", "+1", p]);
        // a path that holds `{line}` itself is not filled in
        assert_eq!(open_cmd("ed {}", "/t/{line}", "3"), ["ed", "/t/{line}"]);
    }

    #[test]
    fn remote_paths_are_never_stat() {
        for p in [
            "//host/share/x",
            "\\\\host\\share",
            "\\\\?\\UNC\\host\\s",
            "//?/C:/x",
        ] {
            assert!(remote(p), "{p}");
            assert!(!exists(p), "{p}");
        }
        #[cfg(unix)]
        for p in [
            "/net/host/x",
            "/Network/Servers/h",
            "/afs/cell/x",
            "/net",
            "/tmp/../net/host",
        ] {
            assert!(remote(p), "{p}");
            assert!(!exists(p), "{p}");
        }
        for p in [
            "/tmp/x",
            "src/main.rs",
            "/network-tools/a",
            "~/x",
            "/netx/a",
        ] {
            assert!(!remote(p), "{p}");
        }
        assert!(exists("src/sys.rs"));
        assert_eq!(share("//host/share/a/b").as_deref(), Some("host/share"));
        assert_eq!(share("/a"), None);
    }

    #[test]
    fn launchers_are_detected() {
        let l = |s: &str| launcher(std::path::Path::new(s));
        for p in [
            "setup.bat",
            "a/b/Run.EXE",
            "x.lnk",
            "y.url",
            "z.ps1",
            "c.cmd",
            "v.vbs",
            "w.js",
            "Foo.app",
            "go.command",
            "t.tool",
            "s.terminal",
            "n.desktop",
            "setup.exe.",
            "k.jar",
        ] {
            assert!(l(p), "{p}");
        }
        for p in [
            "src/main.rs",
            "README.md",
            "a.txt",
            "Makefile",
            "photo.png",
            "x.exe.txt",
        ] {
            assert!(!l(p), "{p}");
        }
        assert_eq!(l("tool.py"), cfg!(any(windows, target_os = "macos")));
    }

    #[cfg(unix)]
    #[test]
    fn exec_bit_counts_as_launcher() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("noname-runs-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("plain");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!runs(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(runs(&f));
        let k = d.join("notes.md");
        std::os::unix::fs::symlink(d.join("x.desktop"), &k).unwrap();
        std::fs::write(d.join("x.desktop"), "x").unwrap();
        assert!(runs(&k));
        assert!(!runs(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(not(windows))]
    #[test]
    fn reveal_uri_is_escaped() {
        assert_eq!(uri(b"/a b/c,d.sh"), "file:///a%20b/c%2Cd.sh");
        assert_eq!(uri("/ä".as_bytes()), "file:///%C3%A4");
    }

    #[test]
    fn clip_gets_utf16_with_bom() {
        assert_eq!(u16le("aé"), [0xff, 0xfe, b'a', 0, 0xe9, 0]);
    }
}
