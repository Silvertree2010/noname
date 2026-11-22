//! Self-update: checks the releases of the project, tells installs from a package manager apart
//! from the ones noname manages itself, and downloads and swaps in the new binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};

use sha2::{Digest, Sha256};

use crate::sys;

/// Git commit this build was made from (empty when unknown).
pub const REV: &str = env!("NONAME_REV");
/// Whether the working tree had uncommitted changes when this build was made.
pub const DIRTY: bool = !env!("NONAME_DIRTY").is_empty();
/// Date of the build's commit, as `YYYY-MM-DD`.
pub const DATE: &str = env!("NONAME_DATE");
const URL: &str = env!("CARGO_PKG_REPOSITORY");
const TARGET: &str = env!("NONAME_TARGET");
/// Version number from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const CHANNEL: &str = match option_env!("NONAME_CHANNEL") {
    Some(c) => c,
    None => "source",
};
const EVERY: u64 = 6 * 3600;

/// Result of an update check.
pub enum Up {
    /// This build is already the newest.
    Current,
    /// A newer build was installed. Holds its short revision or release tag.
    Ready(String),
    /// No update was tried. Holds the reason.
    Skip(String),
    /// The update failed. Holds the reason.
    Fail(String),
}

/// Name of the package manager that owns this install, or `None` when noname updates itself.
pub fn managed() -> Option<&'static str> {
    let p = std::env::current_exe()
        .ok()?
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    if p.contains("/cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
        Some("brew")
    } else if p.contains("/scoop/") {
        Some("scoop")
    } else if p.contains("/winget/") || p.contains("/windowsapps/") {
        Some("winget")
    } else if p.starts_with("/usr/")
        || p.starts_with("/nix/")
        || p.starts_with("/opt/")
        || CHANNEL == "package"
    {
        Some("your package manager")
    } else {
        None
    }
}

/// Short version label: the version number for release builds, otherwise the first seven characters of the revision (`dev` if unknown), with `+` added for a dirty tree.
pub fn short() -> String {
    if CHANNEL != "source" {
        return VERSION.to_string();
    }
    let r: String = REV.chars().take(7).collect();
    let r = if r.is_empty() { "dev".to_string() } else { r };
    if DIRTY { format!("{r}+") } else { r }
}

/// Whether automatic updates are on: `NONAME_NO_UPDATE` is not set and the `autoupdate` config file does not say `off`.
pub fn enabled() -> bool {
    std::env::var("NONAME_NO_UPDATE").is_err()
        && std::fs::read_to_string(sys::cfgfile("autoupdate"))
            .map(|s| s.trim() != "off")
            .unwrap_or(true)
}

/// Turns automatic updates on or off and saves the choice in the `autoupdate` config file.
pub fn set_enabled(on: bool) {
    let f = sys::cfgfile("autoupdate");
    if let Some(d) = Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = sys::save(&f, if on { "on" } else { "off" });
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether at least six hours have passed since the last update check.
pub fn due() -> bool {
    let last = std::fs::read_to_string(sys::cfgfile("update-check"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    now().saturating_sub(last) >= EVERY
}

fn cache() -> PathBuf {
    #[cfg(windows)]
    if let Ok(a) = std::env::var("LOCALAPPDATA") {
        return Path::new(&a).join("noname").join("src");
    }
    let b = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", sys::home()));
    Path::new(&b).join("noname").join("src")
}

fn quiet(c: &mut Command) -> &mut Command {
    sys::guard(c)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
}

fn ok(c: &mut Command) -> bool {
    match quiet(c).spawn() {
        Ok(mut ch) => {
            crate::live::track_child(&ch);
            let r = ch.wait().is_ok_and(|s| s.success());
            crate::live::untrack_pid(ch.id() as i32);
            r
        }
        Err(_) => false,
    }
}

fn out(c: &mut Command) -> String {
    sys::guard(c)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Deletes the old executable left behind by the last update on Windows; does nothing on other systems.
pub fn cleanup() {
    #[cfg(windows)]
    if let Ok(e) = std::env::current_exe() {
        let _ = std::fs::remove_file(e.with_extension("old.exe"));
    }
}

/// Starts an update check on a background thread and returns the receiver for its single `Up` result. `force` lets a source build with uncommitted changes update too.
pub fn check(force: bool) -> Receiver<Up> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(force));
    });
    rx
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock_at(p: &Path) -> Option<Lock> {
    use std::io::Write;
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(p)
        {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                return Some(Lock(p.to_path_buf()));
            }
            Err(_) if stale(p) => {
                let _ = std::fs::remove_file(p);
            }
            Err(_) => return None,
        }
    }
    None
}

fn stale(p: &Path) -> bool {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e.as_secs() > 2 * 3600)
}

fn run(force: bool) -> Up {
    if let Some(m) = managed() {
        return Up::Skip(format!("updates come from {m}"));
    }
    let Some(_l) = lock_at(&cache().with_file_name("update.lock")) else {
        return Up::Skip("another noname is updating".into());
    };
    if replaced() {
        return Up::Skip("noname was replaced on disk, restart to use it".into());
    }
    if CHANNEL == "release" {
        return release();
    }
    if DIRTY && !force {
        return Up::Skip("local build with uncommitted changes".into());
    }
    let (Some(git), Some(cargo)) = (sys::which("git"), sys::which("cargo")) else {
        return Up::Skip("needs git and cargo".into());
    };
    let c = cache();
    let f = sys::cfgfile("update-check");
    if let Some(d) = Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = sys::save(&f, now().to_string());
    if c.join(".git").is_dir() {
        if !ok(Command::new(&git)
            .arg("-C")
            .arg(&c)
            .args(["fetch", "--quiet", "origin"]))
        {
            return Up::Fail("could not reach github".into());
        }
    } else {
        let _ = std::fs::remove_dir_all(&c);
        if let Some(p) = c.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if !ok(Command::new(&git).args(["clone", "--quiet", URL]).arg(&c)) {
            return Up::Fail("could not reach github".into());
        }
    }
    let remote = out(Command::new(&git)
        .arg("-C")
        .arg(&c)
        .args(["rev-parse", "origin/HEAD"]));
    if remote.is_empty() {
        return Up::Fail("no remote head".into());
    }
    if remote == REV {
        return Up::Current;
    }
    if !REV.is_empty()
        && !ok(Command::new(&git).arg("-C").arg(&c).args([
            "merge-base",
            "--is-ancestor",
            REV,
            &remote,
        ]))
    {
        return Up::Skip("this build is newer than github".into());
    }
    if !ok(Command::new(&git)
        .arg("-C")
        .arg(&c)
        .args(["checkout", "--quiet", "--force", &remote]))
    {
        return Up::Fail("checkout failed".into());
    }
    if !ok(Command::new(&cargo)
        .args(["build", "--release", "--locked", "--quiet"])
        .current_dir(&c))
    {
        return Up::Fail("build failed".into());
    }
    let nb = c.join("target").join("release").join(if cfg!(windows) {
        "noname.exe"
    } else {
        "noname"
    });
    let Ok(me) = std::env::current_exe() else {
        return Up::Fail("unknown install path".into());
    };
    if let Err(e) = swap(&nb, &me) {
        return Up::Fail(format!("install failed: {e}"));
    }
    Up::Ready(remote.chars().take(7).collect())
}

/// Whether the running binary was replaced or deleted on disk since it started (another
/// noname updated it, or it was installed again). Linux then names it `... (deleted)`,
/// and swapping a new build in there would only leave a stray file of that name.
fn replaced() -> bool {
    std::env::current_exe().is_ok_and(|p| gone(&p))
}

fn gone(p: &Path) -> bool {
    p.to_string_lossy().ends_with(" (deleted)") && !p.exists()
}

fn swap(nb: &Path, me: &Path) -> std::io::Result<()> {
    let tmp = me.with_extension(format!("new.{}", std::process::id()));
    let r = put(nb, me, &tmp);
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

fn put(nb: &Path, me: &Path, tmp: &Path) -> std::io::Result<()> {
    std::fs::copy(nb, tmp)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(tmp)?
        .sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    let old = {
        let o = me.with_extension("old.exe");
        let _ = std::fs::remove_file(&o);
        std::fs::rename(me, &o)?;
        o
    };
    let r = std::fs::rename(tmp, me);
    #[cfg(windows)]
    if r.is_err() {
        let _ = std::fs::rename(&old, me);
    }
    r
}

fn newer(a: &str, b: &str) -> bool {
    let p = |s: &str| {
        s.trim_start_matches('v')
            .split(['.', '-'])
            .map(|x| x.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    p(a) > p(b)
}

fn release() -> Up {
    let Some(curl) = sys::which("curl") else {
        return Up::Skip("needs curl".into());
    };
    let Some(tar) = sys::which("tar") else {
        return Up::Skip("needs tar".into());
    };
    let f = sys::cfgfile("update-check");
    if let Some(d) = Path::new(&f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = sys::save(&f, now().to_string());
    let api =
        URL.replace("https://github.com/", "https://api.github.com/repos/") + "/releases/latest";
    let j = out(Command::new(&curl).args([
        "-fsSL",
        "-H",
        "Accept: application/vnd.github+json",
        "-A",
        "noname-updater",
        &api,
    ]));
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&j) else {
        return Up::Fail("could not reach github".into());
    };
    let tag = v["tag_name"].as_str().unwrap_or("").to_string();
    if tag.is_empty() {
        return Up::Fail("no release found".into());
    }
    if !newer(&tag, VERSION) {
        return Up::Current;
    }
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let name = format!("noname-{TARGET}.{ext}");
    let asset = |n: &str| {
        v["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"] == n))
            .and_then(|x| x["browser_download_url"].as_str())
            .map(|s| s.to_string())
    };
    let Some(url) = asset(&name) else {
        return Up::Fail(format!("no build for {TARGET}"));
    };
    // the release lists the sha-256 of every archive; without it nothing is installed
    let Some(want) = ["SHA256SUMS".to_string(), format!("{name}.sha256")]
        .iter()
        .filter_map(|n| asset(n))
        .find_map(|u| {
            sum_for(
                &out(Command::new(&curl).args(["-fsSL", "-A", "noname-updater", &u])),
                &name,
            )
        })
    else {
        return Up::Fail("no checksum in the release, not installed".into());
    };
    let d = cache().with_file_name(format!("dl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(cache().with_file_name("dl"));
    let _ = std::fs::remove_dir_all(&d);
    if std::fs::create_dir_all(&d).is_err() {
        return Up::Fail("no cache folder".into());
    }
    let r = fetch(&curl, &tar, &url, &want, &d, &name, tag);
    let _ = std::fs::remove_dir_all(&d);
    r
}

fn fetch(curl: &Path, tar: &Path, url: &str, want: &str, d: &Path, name: &str, tag: String) -> Up {
    let arc = d.join(name);
    if !ok(Command::new(curl)
        .args(["-fsSL", "-A", "noname-updater", "-o"])
        .arg(&arc)
        .arg(url))
    {
        return Up::Fail("download failed".into());
    }
    match std::fs::read(&arc) {
        Ok(b) if hex(&Sha256::digest(&b)) == want => {}
        _ => {
            return Up::Fail("download does not match its checksum, not installed".into());
        }
    }
    if !ok(Command::new(tar).arg("-xf").arg(&arc).arg("-C").arg(d)) {
        return Up::Fail("could not unpack".into());
    }
    let nb = d.join(if cfg!(windows) {
        "noname.exe"
    } else {
        "noname"
    });
    let Ok(me) = std::env::current_exe() else {
        return Up::Fail("unknown install path".into());
    };
    if let Err(e) = swap(&nb, &me) {
        return Up::Fail(format!("install failed: {e}"));
    }
    Up::Ready(tag)
}

/// The sha-256 listed for file `name` in a `sha256sum` style text (`<hex>  <name>` per
/// line, or `<hex> *<name>`), lowercase; `None` when it is not listed or not 64 hex digits.
fn sum_for(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (h, n) = l.trim().split_once(char::is_whitespace)?;
        let n = n.trim_start().trim_start_matches('*');
        (n == name && h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| h.to_ascii_lowercase())
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sum_of_one_file() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let t = format!("{a}  noname-x.tar.gz\n{b} *noname-y.zip\n");
        assert_eq!(sum_for(&t, "noname-x.tar.gz"), Some(a));
        assert_eq!(sum_for(&t, "noname-y.zip"), Some("b".repeat(64)));
        assert_eq!(sum_for(&t, "noname-z.zip"), None);
        // a name that only ends the same, a short hash, an empty text
        assert_eq!(sum_for(&t, "x.tar.gz"), None);
        assert_eq!(sum_for("abc  f", "f"), None);
        assert_eq!(sum_for("", "f"), None);
    }

    #[test]
    fn only_one_update_runs_at_a_time() {
        let d = std::env::temp_dir().join(format!("nn-upd-lock-{}", std::process::id()));
        let p = d.join("update.lock");
        let a = lock_at(&p).unwrap();
        assert!(lock_at(&p).is_none());
        drop(a);
        assert!(!p.exists());
        assert!(lock_at(&p).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_puts_the_new_binary_in_place() {
        let d = std::env::temp_dir().join(format!("nn-upd-swap-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (nb, me) = (d.join("nb"), d.join("me.exe"));
        std::fs::write(&nb, "new").unwrap();
        std::fs::write(&me, "old").unwrap();
        swap(&nb, &me).unwrap();
        assert_eq!(std::fs::read_to_string(&me).unwrap(), "new");
        assert!(swap(&d.join("missing"), &me).is_err());
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|x| x.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".new."))
            .collect();
        assert!(left.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sha256_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_replaced_binary_is_seen() {
        let d = std::env::temp_dir().join(format!("nn-gone-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(gone(&d.join("noname (deleted)")));
        // a real file of that name, and a plain path, are not
        std::fs::write(d.join("x (deleted)"), "").unwrap();
        assert!(!gone(&d.join("x (deleted)")));
        assert!(!gone(&d.join("noname")));
        assert!(!replaced());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
