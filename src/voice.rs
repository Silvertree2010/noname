//! `/voice`: dictation into the input box. Claude Code's own `/voice` only works in its
//! interactive screen, so noname records the microphone itself and transcribes on this machine
//! with whisper.cpp (`whisper-cli` for Whisper, `parakeet-cli` for NVIDIA Parakeet). The engine
//! and the model are downloaded on first use from ggml-org, pinned by sha-256.

use crate::{B, sys};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

/// A speech model that can be downloaded and run.
pub(crate) struct Model {
    /// Name used in `/voice NAME`.
    pub(crate) name: &'static str,
    /// whisper.cpp program that runs it.
    cli: &'static str,
    /// File name of the model in the voice folder.
    file: &'static str,
    url: &'static str,
    sha: &'static str,
    /// Download size in MB.
    pub(crate) mb: u32,
}

/// The models `/voice` offers.
pub(crate) const MODELS: [Model; 2] = [
    Model {
        name: "whisper",
        cli: "whisper-cli",
        file: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        sha: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        mb: 148,
    },
    Model {
        name: "parakeet",
        cli: "parakeet-cli",
        file: "ggml-parakeet-tdt-0.6b-v3-q4_0.bin",
        url: "https://huggingface.co/ggml-org/parakeet-GGUF/resolve/main/ggml-parakeet-tdt-0.6b-v3-q4_0.bin",
        sha: "aa7fe2f5fb47d863ca23e8b1d490632d63a2599f515268b6d6bd656158dad45e",
        mb: 356,
    },
];

/// Shown by `/voice` before a model is picked.
pub(crate) const HELP: &str = "voice  dictation that runs on this machine (whisper.cpp). Pick a model, downloaded once:\n  /voice whisper    OpenAI Whisper base, 148 MB, many languages\n  /voice parakeet   NVIDIA Parakeet TDT 0.6B v3, 356 MB, 25 European languages, more accurate\nAfter that /voice starts recording: enter puts the text in the input box, esc cancels.";

// shortcut: whisper.cpp build b5907 is pinned; bump the tag and the sums together to update.
const WCPP: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5907/";

/// The whisper.cpp build for this system as (archive name, sha-256), if ggml-org publishes one.
fn engine() -> Option<(&'static str, &'static str)> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some((
            "whisper-bin-ubuntu-x64.tar.gz",
            "a6199027fe5b4239c84171d2bca4c0ff44ec86a515494d5a39b237b212d429bd",
        ))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some((
            "whisper-bin-ubuntu-arm64.tar.gz",
            "da2e86ac058e1db0281167355428665b82c0d92e4506d186a3894bec55ceefef",
        ))
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some((
            "whisper-bin-x64.zip",
            "ecca07b277d943e9b9f4712856a144b857271a6b333104e511e1988e80db582b",
        ))
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some((
            "whisper-bin-win-cpu-arm64.zip",
            "4e522946827dcbc517d3cd7ee8d6cfe760e8143076924ac8513622694aa3ccfd",
        ))
    } else {
        None
    }
}

/// The model called `n`, if any.
pub(crate) fn find(n: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.name == n)
}

/// The model picked last time, if any.
pub(crate) fn chosen() -> Option<&'static Model> {
    find(std::fs::read_to_string(sys::cfgfile("voice")).ok()?.trim())
}

/// Remembers `m` for the next `/voice`.
pub(crate) fn choose(m: &Model) {
    let _ = std::fs::create_dir_all(sys::cfgdir());
    let _ = sys::save(sys::cfgfile("voice"), m.name);
}

/// Folder for the engine, the models and the recording: `noname\voice` under `LOCALAPPDATA` on
/// Windows, otherwise `noname/voice` under `XDG_DATA_HOME` or `~/.local/share`.
fn dir() -> PathBuf {
    #[cfg(windows)]
    if let Ok(a) = std::env::var("LOCALAPPDATA") {
        return Path::new(&a).join("noname").join("voice");
    }
    let b =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", sys::home()));
    Path::new(&b).join("noname").join("voice")
}

/// Path of the program `n` inside the unpacked engine (it sits one folder down).
fn cli(n: &str) -> Option<PathBuf> {
    cli_in(&dir().join("engine"), n)
}

fn cli_in(e: &Path, n: &str) -> Option<PathBuf> {
    e.join(".done").is_file().then(|| seek(e, n)).flatten()
}

fn seek(e: &Path, n: &str) -> Option<PathBuf> {
    let n = format!("{n}{}", std::env::consts::EXE_SUFFIX);
    std::iter::once(e.to_path_buf())
        .chain(std::fs::read_dir(e).ok()?.flatten().map(|x| x.path()))
        .map(|d| d.join(&n))
        .find(|p| p.is_file())
}

fn unpack(tar: &Path, arc: &Path, d: &Path, n: &str) -> Result<(), String> {
    let e = d.join("engine");
    let t = d.join(format!("engine.{}.tmp", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    std::fs::create_dir_all(&t).map_err(|e| e.to_string())?;
    let ok = Command::new(tar)
        .arg("-xf")
        .arg(arc)
        .arg("-C")
        .arg(&t)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok
        && seek(&t, n).is_some()
        && std::fs::write(t.join(".done"), "").is_ok()
        && cli_in(&e, n).is_none()
    {
        let _ = std::fs::remove_dir_all(&e);
        let _ = std::fs::rename(&t, &e);
    }
    let _ = std::fs::remove_dir_all(&t);
    if cli_in(&e, n).is_some() {
        Ok(())
    } else {
        Err("could not unpack whisper.cpp".into())
    }
}

/// Whether engine and model of `m` are on disk.
pub(crate) fn ready(m: &Model) -> bool {
    cli(m.cli).is_some() && dir().join(m.file).is_file()
}

/// MB downloaded so far by a running `fetch`.
pub(crate) fn got_mb() -> u64 {
    std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|x| {
            x.file_name()
                .to_string_lossy()
                .ends_with(&format!(".{}.part", std::process::id()))
        })
        .filter_map(|x| x.metadata().ok())
        .map(|x| x.len())
        .sum::<u64>()
        / 1_000_000
}

/// Downloads what `m` still needs in the background.
pub(crate) fn fetch(m: &'static Model) -> Receiver<Result<(), String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(get(m));
    });
    rx
}

fn get(m: &Model) -> Result<(), String> {
    let curl = sys::which("curl").ok_or("needs curl")?;
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("no voice folder: {e}"))?;
    sweep();
    if cli(m.cli).is_none() {
        let (name, sha) = engine().ok_or("no whisper.cpp build for this system")?;
        let tar = sys::which("tar").ok_or("needs tar")?;
        let arc = d.join(format!("{name}.{}.part", std::process::id()));
        dl(&curl, &format!("{WCPP}{name}"), &arc, sha)?;
        let r = unpack(&tar, &arc, &d, m.cli);
        let _ = std::fs::remove_file(&arc);
        r?;
    }
    let f = d.join(m.file);
    if !f.is_file() {
        let p = d.join(format!("{}.{}.part", m.file, std::process::id()));
        dl(&curl, m.url, &p, m.sha)?;
        std::fs::rename(&p, &f).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Downloads `url` to `to` and keeps it only when its sha-256 is `sha`.
fn dl(curl: &Path, url: &str, to: &Path, sha: &str) -> Result<(), String> {
    let ok = Command::new(curl)
        .args(["-fsSL", "-A", "noname", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok && sha_file(to).as_deref() == Some(sha) {
        return Ok(());
    }
    let _ = std::fs::remove_file(to);
    Err(if ok {
        format!("{url} does not match its checksum, removed")
    } else {
        format!("download failed: {url}")
    })
}

fn sha_file(p: &Path) -> Option<String> {
    let mut h = Sha256::new();
    std::io::copy(&mut std::fs::File::open(p).ok()?, &mut h).ok()?;
    Some(h.finalize().iter().map(|x| format!("{x:02x}")).collect())
}

/// A running microphone recording.
pub(crate) struct Rec {
    /// When it started.
    pub(crate) t: Instant,
    #[cfg(unix)]
    ch: std::process::Child,
}

fn wav() -> PathBuf {
    dir().join(format!("rec-{}.wav", std::process::id()))
}

fn sweep() {
    let old = |x: &std::fs::DirEntry| {
        x.metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|e| e.as_secs() > 86_400)
    };
    for x in std::fs::read_dir(dir()).into_iter().flatten().flatten() {
        let n = x.file_name().to_string_lossy().to_string();
        if !old(&x) {
            continue;
        }
        if n.starts_with("rec") && n.ends_with(".wav") || n.ends_with(".part") {
            let _ = std::fs::remove_file(x.path());
        } else if n.starts_with("engine.") && n.ends_with(".tmp") {
            let _ = std::fs::remove_dir_all(x.path());
        }
    }
}

impl Rec {
    /// Starts recording 16 kHz mono into the voice folder, with the first of pw-record, parecord
    /// or arecord that is installed.
    #[cfg(unix)]
    pub(crate) fn start() -> Result<Rec, String> {
        let w = wav();
        let _ = std::fs::create_dir_all(dir());
        sweep();
        let _ = std::fs::remove_file(&w);
        let ws = w.to_string_lossy().to_string();
        let tries: [(&str, Vec<&str>); 3] = [
            (
                "pw-record",
                vec!["--rate", "16000", "--channels", "1", "--format", "s16", &ws],
            ),
            (
                "parecord",
                vec![
                    "--rate=16000",
                    "--channels=1",
                    "--format=s16le",
                    "--file-format=wav",
                    &ws,
                ],
            ),
            (
                "arecord",
                vec![
                    "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "wav", &ws,
                ],
            ),
        ];
        let (b, a) = tries
            .iter()
            .find_map(|(b, a)| sys::which(b).map(|p| (p, a)))
            .ok_or("needs pw-record, parecord or arecord to record")?;
        let ch = sys::guard(Command::new(b).args(a))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not record: {e}"))?;
        crate::live::track_child(&ch);
        Ok(Rec {
            t: Instant::now(),
            ch,
        })
    }

    /// Starts recording 16 kHz mono through the Windows MCI wave device.
    #[cfg(windows)]
    pub(crate) fn start() -> Result<Rec, String> {
        let _ = std::fs::create_dir_all(dir());
        sweep();
        mci("close nnrec");
        if mci("open new type waveaudio alias nnrec") != 0 {
            return Err("no microphone found".into());
        }
        mci("set nnrec bitspersample 16 samplespersec 16000 channels 1");
        if mci("record nnrec") != 0 {
            mci("close nnrec");
            return Err("could not record".into());
        }
        Ok(Rec { t: Instant::now() })
    }

    /// Stops and transcribes the recording with `m` in the background.
    #[cfg_attr(windows, allow(unused_mut))]
    pub(crate) fn finish(mut self, m: &'static Model) -> Receiver<Result<String, String>> {
        let w = wav();
        #[cfg(unix)]
        {
            // SIGINT lets the recorder write the WAV header before it exits
            // SAFETY: kill only sends a signal to the pid of our own child, which is not reaped yet.
            unsafe { libc::kill(self.ch.id() as i32, libc::SIGINT) };
            let _ = self.ch.wait();
            crate::live::untrack_pid(self.ch.id() as i32);
        }
        #[cfg(windows)]
        {
            mci("stop nnrec");
            mci(&format!("save nnrec \"{}\"", w.display()));
            mci("close nnrec");
        }
        std::mem::drop(self);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = run(m, &w);
            let _ = std::fs::remove_file(&w);
            let _ = tx.send(r);
        });
        rx
    }
}

impl Drop for Rec {
    fn drop(&mut self) {
        #[cfg(unix)]
        if matches!(self.ch.try_wait(), Ok(None)) {
            let _ = self.ch.kill();
            let _ = self.ch.wait();
            crate::live::untrack_pid(self.ch.id() as i32);
            let _ = std::fs::remove_file(wav());
        }
        #[cfg(windows)]
        mci("close nnrec");
    }
}

#[cfg(windows)]
fn mci(s: &str) -> u32 {
    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call; no return buffer
    // (null, length 0) and no callback window are passed.
    unsafe {
        windows_sys::Win32::Media::Multimedia::mciSendStringW(
            w.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        )
    }
}

/// Transcribes `w` with `m` and returns the text on one line.
fn run(m: &Model, w: &Path) -> Result<String, String> {
    let c = cli(m.cli).ok_or("whisper.cpp is missing, run /voice again")?;
    let mut cmd = Command::new(&c);
    cmd.arg("-m")
        .arg(dir().join(m.file))
        .arg("-f")
        .arg(w)
        .arg("-np");
    if m.cli == "whisper-cli" {
        cmd.args(["-nt", "-l", "auto"]);
    }
    if let Some(d) = c.parent() {
        cmd.env("LD_LIBRARY_PATH", d);
    }
    let o = sys::guard(&mut cmd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", m.cli))?;
    let t = clean(&String::from_utf8_lossy(&o.stdout));
    if t.is_empty() {
        Err("heard nothing".into())
    } else {
        Ok(t)
    }
}

/// Joins the lines of a transcript and drops whisper's `[BLANK_AUDIO]`-style tags.
fn clean(s: &str) -> String {
    s.split_whitespace()
        .filter(|w| !(w.starts_with('[') && w.ends_with(']')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `/voice` job running in the background.
pub(crate) enum VJob {
    /// Downloading the engine and this model.
    Get(
        &'static Model,
        std::sync::mpsc::Receiver<Result<(), String>>,
    ),
    /// Transcribing a recording.
    Text(std::sync::mpsc::Receiver<Result<String, String>>),
}

impl crate::App {
    /// `/voice [whisper|parakeet]`: picks the model, downloads it once, then starts (or stops) recording.
    pub(crate) fn voice(&mut self, a: &str) {
        if self.vjob.is_some() {
            self.toast("voice  still busy");
            return;
        }
        if self.vrec.is_some() {
            self.vstop(true);
            return;
        }
        let m = if a.is_empty() {
            chosen()
        } else if let Some(m) = find(a) {
            choose(m);
            Some(m)
        } else {
            self.toast(format!("unknown voice model  {a}"));
            return;
        };
        let Some(m) = m else {
            self.bs.push(B::Sys(HELP.into()));
            return;
        };
        if !ready(m) {
            self.bs.push(B::Sys(format!(
                "voice  downloading {} ({} MB) and whisper.cpp, once",
                m.name, m.mb
            )));
            self.vjob = Some(VJob::Get(m, fetch(m)));
            return;
        }
        match Rec::start() {
            Ok(r) => self.vrec = Some(r),
            Err(e) => self.toast(format!("voice  {e}")),
        }
    }

    /// Ends the recording: `go` transcribes it, otherwise it is thrown away.
    pub(crate) fn vstop(&mut self, go: bool) {
        let Some(r) = self.vrec.take() else { return };
        match chosen() {
            Some(m) if go => {
                self.toast("voice  transcribing");
                self.vjob = Some(VJob::Text(r.finish(m)));
            }
            _ => self.toast("voice  cancelled"),
        }
    }

    /// Shows recording and download progress and takes in finished `/voice` jobs.
    pub(crate) fn vpoll(&mut self) {
        if let Some(r) = &self.vrec {
            let s = r.t.elapsed().as_secs();
            self.toast(format!(
                "● recording {}:{:02}  ·  enter puts the text in the input  ·  esc cancels",
                s / 60,
                s % 60
            ));
        }
        match &self.vjob {
            Some(VJob::Get(m, rx)) => match rx.try_recv() {
                Ok(Ok(())) => {
                    let n = m.name;
                    self.vjob = None;
                    self.bs
                        .push(B::Sys(format!("voice  {n} is ready  ·  /voice to talk")));
                }
                Ok(Err(e)) => {
                    self.vjob = None;
                    self.bs.push(B::Sys(format!("voice  {e}")));
                }
                Err(_) => {
                    let t = format!(
                        "voice  downloading {}  {} of ~{} MB",
                        m.name,
                        got_mb(),
                        m.mb
                    );
                    self.toast(t);
                }
            },
            Some(VJob::Text(rx)) => {
                if let Ok(r) = rx.try_recv() {
                    self.vjob = None;
                    match r {
                        Ok(t) => {
                            self.snap((self.inp.clone(), self.cur));
                            let sp = self.cur > 0
                                && !self
                                    .inp
                                    .chars()
                                    .nth(self.cur - 1)
                                    .is_some_and(char::is_whitespace);
                            if sp {
                                self.ins(' ');
                            }
                            t.chars().for_each(|c| self.ins(c));
                            self.toast = None;
                        }
                        Err(e) => self.toast(format!("voice  {e}")),
                    }
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_transcripts() {
        assert_eq!(
            clean(" Hello there,\n how are you? \n"),
            "Hello there, how are you?"
        );
        assert_eq!(clean("[BLANK_AUDIO]\n"), "");
    }

    #[test]
    fn finds_models() {
        assert_eq!(find("parakeet").map(|m| m.cli), Some("parakeet-cli"));
        assert!(find("nope").is_none());
        assert!(MODELS.iter().all(|m| m.sha.len() == 64));
    }

    #[test]
    fn an_engine_counts_only_once_unpacked_completely() {
        let d = std::env::temp_dir().join(format!("nn-voice-eng-{}", std::process::id()));
        let b = d.join("Release");
        std::fs::create_dir_all(&b).unwrap();
        let n = format!("whisper-cli{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(b.join(&n), "").unwrap();
        assert_eq!(seek(&d, "whisper-cli"), Some(b.join(&n)));
        assert!(cli_in(&d, "whisper-cli").is_none());
        std::fs::write(d.join(".done"), "").unwrap();
        assert_eq!(cli_in(&d, "whisper-cli"), Some(b.join(&n)));
        assert!(cli_in(&d, "parakeet-cli").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_recording_is_per_process() {
        assert!(
            wav()
                .to_string_lossy()
                .contains(&std::process::id().to_string())
        );
    }

    #[test]
    fn checks_sha_of_a_file() {
        let p = std::env::temp_dir().join(format!("nn-voice-sha-{}", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha_file(&p).as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        let _ = std::fs::remove_file(&p);
    }
}
