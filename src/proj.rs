//! Which project a chat is about, so the window title shows it and two windows on the same
//! project are noticed.
//!
//! The known projects live in the config file `projects`, one per line:
//!
//! ```text
//! BananaSplit: banana split, bananasplit.tech
//! noname
//! scan ~/project *
//! -review
//! ```
//!
//! `Name: alias, alias` names a project with other spellings, `scan DIR/PREFIX*` adds every folder in
//! DIR whose name starts with PREFIX (the rest of the folder name is the project), and `-name` leaves
//! a name out (a scanned folder with a too common name). A project set with `/project` that is not
//! known yet is added to the file.

use crate::sys;
use std::path::Path;

/// Path of the file with the known projects.
pub(crate) fn file() -> String {
    sys::cfgfile("projects")
}

fn mapfile() -> String {
    sys::cfgfile("session-projects")
}

fn expand(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(r) => format!("{}/{r}", sys::home()),
        None if p == "~" => sys::home(),
        None => p.to_string(),
    }
}

/// The known projects as (name, other spellings), read from `text` (the projects file).
pub(crate) fn parse(text: &str) -> Vec<(String, Vec<String>)> {
    let mut v: Vec<(String, Vec<String>)> = vec![];
    let mut skip: Vec<String> = vec![];
    let add = |v: &mut Vec<(String, Vec<String>)>, n: &str, al: Vec<String>| {
        let n = n.trim();
        if n.is_empty() {
            return;
        }
        if let Some(e) = v.iter_mut().find(|e| e.0.eq_ignore_ascii_case(n)) {
            e.1.extend(al);
        } else {
            v.push((n.to_string(), al));
        }
    };
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(n) = l.strip_prefix('-') {
            skip.push(n.trim().to_lowercase());
        } else if let Some(g) = l.strip_prefix("scan ") {
            let g = expand(g.trim().trim_end_matches('*'));
            let (dir, pre) = match g.rfind('/') {
                Some(i) => (&g[..i.max(1)], &g[i + 1..]),
                None => (".", g.as_str()),
            };
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut ns: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter_map(|n| n.strip_prefix(pre).map(|r| r.trim().to_string()))
                .filter(|n| !n.is_empty() && !n.starts_with('.'))
                .collect();
            ns.sort();
            for n in ns {
                add(&mut v, &n, vec![]);
            }
        } else {
            let (n, al) = l.split_once(':').unwrap_or((l, ""));
            let al = al
                .split(',')
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
            add(&mut v, n, al);
        }
    }
    v.retain(|e| !skip.contains(&e.0.to_lowercase()));
    v
}

/// The known projects from the projects file.
pub(crate) fn list() -> Vec<(String, Vec<String>)> {
    parse(&std::fs::read_to_string(file()).unwrap_or_default())
}

/// Position of `w` in `t` as a whole word (both already lower case).
fn word_at(t: &str, w: &str) -> Option<usize> {
    if w.is_empty() {
        return None;
    }
    let al = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
    let mut from = 0;
    while let Some(i) = t[from..].find(w) {
        let s = from + i;
        let e = s + w.len();
        if !al(t[..s].chars().next_back()) && !al(t[e..].chars().next()) {
            return Some(s);
        }
        from = s + w.chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// The project of `ps` that `text` names first (a longer name wins at the same spot).
pub(crate) fn find_in(ps: &[(String, Vec<String>)], text: &str) -> Option<String> {
    let t = text.to_lowercase();
    let mut best: Option<(usize, usize, &str)> = None;
    for (n, al) in ps {
        for w in std::iter::once(n).chain(al) {
            let w = w.to_lowercase();
            if let Some(i) = word_at(&t, &w) {
                let k = (i, usize::MAX - w.len());
                if best.is_none_or(|b| k < (b.0, b.1)) {
                    best = Some((k.0, k.1, n));
                }
            }
        }
    }
    best.map(|b| b.2.to_string())
}

/// The known project that `text` names first.
pub(crate) fn find(text: &str) -> Option<String> {
    find_in(&list(), text)
}

/// The known spelling of `name`, or `name` itself after adding it to the projects file.
/// The bool says whether the project is new.
pub(crate) fn learn(name: &str) -> (String, bool) {
    let name = name.trim();
    let ps = list();
    if let Some(e) = ps.iter().find(|e| {
        e.0.eq_ignore_ascii_case(name) || e.1.iter().any(|a| a.eq_ignore_ascii_case(name))
    }) {
        return (e.0.clone(), false);
    }
    let f = file();
    let mut t = std::fs::read_to_string(&f).unwrap_or_default();
    if !t.is_empty() && !t.ends_with('\n') {
        t.push('\n');
    }
    t += name;
    t.push('\n');
    let _ = std::fs::create_dir_all(sys::cfgdir());
    let _ = std::fs::write(&f, t);
    (name.to_string(), true)
}

/// The project saved for session `sid`.
pub(crate) fn of_session(sid: &str) -> Option<String> {
    if sid.is_empty() {
        return None;
    }
    std::fs::read_to_string(mapfile())
        .ok()?
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .rfind(|(s, _)| *s == sid)
        .map(|(_, p)| p.to_string())
        .filter(|p| !p.is_empty())
}

/// Saves `p` as the project of session `sid` (empty `p` clears it).
pub(crate) fn save_session(sid: &str, p: &str) {
    if sid.is_empty() || of_session(sid).as_deref().unwrap_or("") == p {
        return;
    }
    let f = mapfile();
    let mut t = std::fs::read_to_string(&f).unwrap_or_default();
    // keep the file small: the newest 2000 lines
    let n = t.lines().count();
    if n > 2000 {
        t = t.lines().skip(n - 2000).map(|l| format!("{l}\n")).collect();
    }
    t += &format!("{sid}\t{p}\n");
    let _ = std::fs::create_dir_all(sys::cfgdir());
    let _ = std::fs::write(&f, t);
}

fn live_dir() -> String {
    let b = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    format!("{b}/noname-projects")
}

/// Tells the other noname windows that this one works on `p` (empty `p`: on nothing).
pub(crate) fn announce(p: &str) {
    let d = live_dir();
    let f = format!("{d}/{}", std::process::id());
    if p.is_empty() {
        let _ = std::fs::remove_file(f);
    } else {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(f, p);
    }
}

/// How many other running noname windows work on `p`.
pub(crate) fn others(p: &str) -> usize {
    if p.is_empty() || !cfg!(target_os = "linux") {
        return 0;
    }
    let me = std::process::id().to_string();
    let Ok(rd) = std::fs::read_dir(live_dir()) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let pid = e.file_name().to_string_lossy().to_string();
        if pid == me {
            continue;
        }
        if !Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        if std::fs::read_to_string(e.path()).is_ok_and(|x| x.trim().eq_ignore_ascii_case(p)) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps() -> Vec<(String, Vec<String>)> {
        parse(
            "BananaSplit: banana split\nnoname\nThe Researcher: researcher\nNero\n-nero\nSolidCraft\n",
        )
    }

    #[test]
    fn finds_the_first_named_project() {
        let p = ps();
        assert_eq!(
            find_in(&p, "in noname i have an issue, like bananasplit"),
            Some("noname".into())
        );
        assert_eq!(
            find_in(&p, "the Banana Split mic hums"),
            Some("BananaSplit".into())
        );
        assert_eq!(
            find_in(&p, "open The Researcher popup"),
            Some("The Researcher".into())
        );
        assert_eq!(
            find_in(&p, "fix ~/src/noname-work/src/lib.rs"),
            Some("noname".into())
        );
    }

    #[test]
    fn needs_whole_words_and_skips_left_out_names() {
        let p = ps();
        assert_eq!(find_in(&p, "nonames and solidcrafting"), None);
        assert_eq!(find_in(&p, "the nero layout"), None);
    }

    #[test]
    fn aliases_join_one_project() {
        let p = parse("A: x\nA: y\n");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].1, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn scan_adds_folders_with_the_prefix() {
        let d = std::env::temp_dir().join(format!("nn-proj-{}", std::process::id()));
        std::fs::create_dir_all(d.join("project Alpha")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        let p = parse(&format!("scan {}/project *\n", d.display()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "Alpha");
    }
}
