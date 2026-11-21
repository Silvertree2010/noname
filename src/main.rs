//! The `noname` binary: the terminal event loop, rendering and slash-command handling on top of the `noname` library.

use noname::pal::*;
use noname::*;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

struct V1 {
    db: [f32; 3],
    lt: u64,
    sf: f32,
    bg: Option<noname::bg::Bg>,
}

impl Clone for V1 {
    fn clone(&self) -> Self {
        V1 {
            db: self.db,
            lt: self.lt,
            sf: self.sf,
            bg: None,
        }
    }
}

fn gd() -> [Color; 3] {
    [LI(), L2(), CY()]
}

fn tail_nz(v: &mut Vec<Line<'static>>, a: &App, seed: u64) {
    if let Some(l) = v.last_mut() {
        l.spans.push(pad(1));
        l.spans.extend(noise(3, a.t, seed, &gd()));
    }
}

fn sy(k: Tk) -> (&'static str, Color) {
    match k {
        Tk::Read => (".", CY()),
        Tk::Edit => ("+", GR()),
        Tk::Bash => ("$", LI()),
        Tk::Grep => ("?", CY()),
        Tk::Web => ("@", L2()),
        Tk::Task => ("&", PK()),
        Tk::Oth => ("*", PE()),
    }
}

fn mdt() -> Md {
    Md {
        t: st(FG()),
        b: st(WH()).bold(),
        c: st(L2()),
        fe: st(PE()),
        kw: st(LI()),
        st: st(GR()),
        cm: st(FA()).italic(),
        nu: st(PK()),
        bul: "- ",
        bs: st(LI()),
        fpre: "  | ",
        fps: st(FA()),
    }
}

fn glitch(v: Vec<Span<'static>>, el: u64, t: u64) -> Vec<Span<'static>> {
    if el >= 16 {
        return v;
    }
    let mut o = vec![];
    if (t / 2) % 3 == 1 {
        o.push(pad(1));
    }
    let mut k = 0u64;
    for sp in v {
        let mut run = String::new();
        for c in sp.content.chars() {
            k += 1;
            if c != ' ' && hsh(t / 2, k) % 6 == 0 {
                if !run.is_empty() {
                    o.push(Span::styled(std::mem::take(&mut run), sp.style));
                }
                let gc = if hsh(t, k) % 2 == 0 { RD() } else { PK() };
                o.push(Span::styled(nz(t, k).to_string(), st(gc).bold()));
            } else {
                run.push(c);
            }
        }
        if !run.is_empty() {
            o.push(Span::styled(run, sp.style));
        }
    }
    o
}

fn menu(a: &App) -> Vec<(String, String)> {
    let Some(q) = a.inp.strip_prefix('/') else {
        return vec![];
    };
    if q.contains(' ') || a.inp.contains('\n') {
        return vec![];
    }
    let mut cs = a.cmds();
    cs.dedup_by(|x, y| x.0 == y.0);
    let mut p: Vec<(String, String)> = cs
        .iter()
        .filter(|(c, _)| c.starts_with(q))
        .cloned()
        .collect();
    p.sort_by_key(|(c, _)| (c.contains(':'), c.len()));
    let mut r: Vec<(String, String)> = cs
        .iter()
        .filter(|(c, _)| !c.starts_with(q) && c.contains(q))
        .cloned()
        .collect();
    r.sort_by_key(|(c, _)| (c.contains(':'), c.len()));
    p.extend(r);
    if p.len() == 1 && p[0].0 == q {
        return vec![];
    }
    p
}

fn ghost(a: &App) -> Option<(String, String)> {
    let q = a.inp.strip_prefix('/')?;
    if a.cur != a.inp.chars().count() {
        return None;
    }
    let m = menu(a);
    let (c, d) = m.get(a.ms.min(m.len().saturating_sub(1)))?;
    let r = c.strip_prefix(q)?;
    Some((r.to_string(), d.clone()))
}

fn unlig(v: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut o = vec![];
    for sp in v {
        for c in sp.content.chars() {
            let s = if c == '\\' {
                sp.style.remove_modifier(Modifier::BOLD)
            } else {
                sp.style.add_modifier(Modifier::BOLD)
            };
            o.push(Span::styled(c.to_string(), s));
        }
    }
    o
}

fn fnum(n: usize) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

fn pad(n: usize) -> Span<'static> {
    Span::raw(" ".repeat(n))
}

fn hid(a: &App, i: usize, b: &B) -> bool {
    match b {
        B::Th(s) => s.trim().is_empty() && a.streaming() != Some(i),
        B::Ask(k) => matches!(k.ans, Some('y') | Some('a')),
        B::Tl(t) => t.nm == "askuserquestion",
        _ => false,
    }
}

fn gok(b: &B) -> bool {
    matches!(b, B::Tl(t) if matches!(t.k, Tk::Read | Tk::Grep | Tk::Web | Tk::Oth) && t.st != St::Err && t.nm != "askuserquestion")
}

fn groups(a: &App) -> Vec<Option<(usize, Vec<usize>)>> {
    let n = a.bs.len();
    let mut g: Vec<Option<(usize, Vec<usize>)>> = (0..n).map(|_| None).collect();
    if a.open {
        return g;
    }
    let mut i = 0;
    while i < n {
        if !gok(&a.bs[i]) {
            i += 1;
            continue;
        }
        let mut m = vec![i];
        let mut j = i + 1;
        while j < n {
            if gok(&a.bs[j]) {
                m.push(j);
            } else if !hid(a, j, &a.bs[j]) {
                break;
            }
            j += 1;
        }
        if m.len() >= 2 {
            for &x in &m {
                g[x] = Some((i, vec![]));
            }
            g[i] = Some((i, m));
        }
        i = j;
    }
    g
}

fn gline(a: &App, ix: &[usize], w: usize) -> Vec<Line<'static>> {
    let ts: Vec<&noname::Tool> = ix
        .iter()
        .filter_map(|&i| {
            if let B::Tl(t) = &a.bs[i] {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    let run = ts.iter().any(|t| t.st == St::Run);
    let cnt = |k: Tk| ts.iter().filter(|t| t.k == k).count();
    let mut rd: Vec<&str> = ts
        .iter()
        .filter(|t| t.k == Tk::Read)
        .map(|t| t.a.as_str())
        .collect();
    rd.sort();
    rd.dedup();
    let parts: [(usize, [&str; 2], &str, &str); 5] = [
        (cnt(Tk::Bash), ["ran", "running"], "command", "commands"),
        (rd.len(), ["read", "reading"], "file", "files"),
        (
            cnt(Tk::Grep),
            ["searched for", "searching for"],
            "pattern",
            "patterns",
        ),
        (cnt(Tk::Web), ["fetched", "fetching"], "page", "pages"),
        (cnt(Tk::Oth), ["called", "calling"], "tool", "tools"),
    ];
    let sd = ix[0] as u64 * 7 + 3;
    let mut l = vec![pad(2)];
    if run {
        l.extend(noise(1, a.t, sd, &gd()));
    } else {
        let (c, col) = sy(ts.last().map(|t| t.k).unwrap_or(Tk::Oth));
        l.push(Span::styled(c, st(col).bold()));
    }
    l.push(pad(1));
    let mut first = true;
    for (n, vb, one, many) in parts {
        if n == 0 {
            continue;
        }
        if !first {
            l.push(Span::styled(", ", st(MU())));
        }
        let v = vb[run as usize];
        let v = if first {
            format!("{}{}", v[..1].to_uppercase(), &v[1..])
        } else {
            v.to_string()
        };
        first = false;
        l.push(Span::styled(format!("{v} "), st(MU())));
        l.push(Span::styled(n.to_string(), st(FG()).bold()));
        l.push(Span::styled(
            format!(" {}", if n == 1 { one } else { many }),
            st(MU()),
        ));
    }
    let t0 = ts
        .iter()
        .map(|t| t.t0)
        .filter(|&x| x > 0)
        .min()
        .unwrap_or(0);
    let r = if run {
        let mut r = noise(6, a.t, sd + 1, &gd());
        r.push(Span::styled(
            format!("  {:.1}s", a.t.saturating_sub(t0) as f32 / 30.0),
            st(FA()),
        ));
        r
    } else {
        vec![]
    };
    let mut v = vec![lr(l, r, w)];
    if run {
        if let Some(t) = ts.iter().rev().find(|t| t.st == St::Run) {
            let (c, _) = sy(t.k);
            let mut s = t.a.clone();
            let mx = w.saturating_sub(10);
            if s.chars().count() > mx {
                s = s.chars().take(mx.saturating_sub(1)).collect::<String>() + "~";
            }
            v.push(Line::from(vec![
                pad(4),
                Span::styled(format!("{c} "), st(FA())),
                Span::styled(s, st(FA())),
            ]));
        }
    }
    v
}

thread_local! {
    static LK: std::cell::RefCell<Vec<(usize, u16, u16, String)>> = const { std::cell::RefCell::new(vec![]) };
}

fn link(i: usize, x: usize, len: usize, t: &str) {
    if !t.is_empty() && len > 0 {
        LK.with(|l| {
            l.borrow_mut()
                .push((i, x as u16, (x + len) as u16, t.to_string()))
        });
    }
}

/// The characters of a rendered line with their column and width.
fn cols(l: &Line) -> Vec<(usize, char, usize)> {
    let mut o = vec![];
    let mut x = 0;
    for sp in &l.spans {
        for c in sp.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            o.push((x, c, cw));
            x += cw;
        }
    }
    o
}

/// Web links in a rendered line: (start column, end column, url).
#[cfg(test)]
fn urls(l: &Line) -> Vec<(u16, u16, String)> {
    links(std::slice::from_ref(l), 0, 1, usize::MAX)
        .into_iter()
        .map(|(_, xa, xb, u)| (xa, xb, u))
        .collect()
}

/// Web links in lines a..b of the body, as (line, start column, end column, url) pieces.
/// The wrapper cuts a word longer than the width hard at the right edge (column `w`) and
/// goes on at the same column on the next line, so such a cut is joined back first:
/// a link that wraps over several lines gives one piece per line, each with the full url.
fn links(bl: &[Line], a: usize, b: usize, w: usize) -> Vec<(usize, u16, u16, String)> {
    // (line, column, char, width); a '\n' with line usize::MAX separates lines that are not joined
    let mut cs: Vec<(usize, usize, char, usize)> = vec![];
    let mut from = 0;
    // the line before: its code gutter, where its text starts, and whether it is a wrapped part
    let mut pg: Option<(String, usize, bool)> = None;
    let mut nx = if a < b { cols(&bl[a]) } else { vec![] };
    for k in a..b {
        let cur = std::mem::take(&mut nx);
        let st = cs.len();
        cs.extend(
            cur.iter()
                .filter(|c| c.0 >= from)
                .map(|c| (k, c.0, c.1, c.2)),
        );
        from = 0;
        // a wrapped part of a code line sits behind the same gutter, two columns further in
        // than the line it belongs to (or as far as the wrapped part before it)
        let (gk, ek) = gutter(&cur);
        let wrapped = |w: bool, g: &str, e: usize, gn: &str, en: usize| {
            !g.is_empty() && g == gn && (e.checked_add(2) == Some(en) || (w && en == e))
        };
        let wk = pg
            .take()
            .is_some_and(|(g, e, w)| wrapped(w, &g, e, &gk, ek));
        pg = Some((gk.clone(), ek, wk));
        if k + 1 < b {
            nx = cols(&bl[k + 1]);
            if let Some(&(lx, lc, lw)) = cur.last() {
                if !lc.is_whitespace() && lx + lw == w {
                    // the last word of this line, and where it starts
                    let p = cs[st..]
                        .iter()
                        .rev()
                        .take_while(|c| !c.2.is_whitespace())
                        .last()
                        .map(|c| c.1)
                        .unwrap_or(0);
                    let cont = nx
                        .iter()
                        .find(|c| c.0 >= p)
                        .is_some_and(|c| c.0 == p && !c.1.is_whitespace());
                    if cont
                        && nx
                            .iter()
                            .take_while(|c| c.0 < p)
                            .all(|c| c.1.is_whitespace())
                    {
                        from = p;
                        continue;
                    }
                    // a code block line: the word is all the text behind the `|` gutter and
                    // goes on in the next line, a wrapped part of the same code line
                    let (gn, en) = gutter(&nx);
                    if p == ek && en < usize::MAX && wrapped(wk, &gk, ek, &gn, en) {
                        from = en;
                        continue;
                    }
                }
            }
        }
        cs.push((usize::MAX, 0, '\n', 0));
    }
    let t: Vec<char> = cs.iter().map(|c| c.2).collect();
    let mut o = vec![];
    let mut i = 0;
    while i < t.len() {
        let r = &t[i..];
        let at = r.starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/'])
            || r.starts_with(&['h', 't', 't', 'p', ':', '/', '/']);
        if !at || (i > 0 && t[i - 1].is_alphanumeric()) {
            if let Some(e) = path_at(&t, i) {
                piece(&cs, i, e, t[i..e].iter().collect(), &mut o);
                i = e;
            } else {
                i += 1;
            }
            continue;
        }
        let mut j = i;
        while j < t.len()
            && !t[j].is_whitespace()
            && !matches!(t[j], '<' | '>' | '"' | '`' | '│' | '…')
        {
            j += 1;
        }
        let mut e = j;
        while e > i
            && matches!(
                t[e - 1],
                '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_'
            )
        {
            e -= 1;
        }
        // a closing bracket belongs to the url only when it opened inside it
        while e > i && matches!(t[e - 1], ')' | ']') {
            let (op, cl) = if t[e - 1] == ')' {
                ('(', ')')
            } else {
                ('[', ']')
            };
            let n = |c: char| t[i..e].iter().filter(|&&x| x == c).count();
            if n(cl) > n(op) { e -= 1 } else { break }
        }
        let u: String = t[i..e].iter().collect();
        if u.len() > 10 {
            piece(&cs, i, e, u, &mut o);
        }
        i = j.max(i + 1);
    }
    o
}

/// The gutter of a line: its leading spaces and `|` / `│` bars, as (the gutter without its
/// trailing spaces, the column where the text after it starts or usize::MAX when none).
fn gutter(l: &[(usize, char, usize)]) -> (String, usize) {
    let g: Vec<&(usize, char, usize)> = l
        .iter()
        .take_while(|c| c.1.is_whitespace() || matches!(c.1, '|' | '│'))
        .collect();
    let s: String = g.iter().map(|c| c.1).collect();
    let end = l.get(g.len()).map(|c| c.0).unwrap_or(usize::MAX);
    (s.trim_end().to_string(), end)
}

/// One link piece per line that chars i..e of `cs` run over, each with the full target.
fn piece(
    cs: &[(usize, usize, char, usize)],
    i: usize,
    e: usize,
    u: String,
    o: &mut Vec<(usize, u16, u16, String)>,
) {
    let mut q = i;
    while q < e {
        let ln = cs[q].0;
        let mut z = q;
        while z < e && cs[z].0 == ln {
            z += 1;
        }
        let xb = cs[z - 1].1 + cs[z - 1].3;
        o.push((ln, cs[q].1 as u16, xb as u16, u.clone()));
        q = z;
    }
}

/// A file or folder path that starts at t[i] and exists on disk: returns where it ends.
/// It must start a word (after a space, a bracket, a quote or `*`) and look like a path
/// (a `/`, a `.` or a leading `~`); a `:line` or `:line:col` after it is kept, so the
/// click can open the file at that line.
fn path_at(t: &[char], i: usize) -> Option<usize> {
    if t[i].is_whitespace()
        || (i > 0
            && !(t[i - 1].is_whitespace()
                || matches!(
                    t[i - 1],
                    '(' | '[' | '{' | '"' | '\'' | '`' | '<' | '*' | '='
                )))
    {
        return None;
    }
    if !(t[i] == '/' || t[i] == '~' || t[i] == '.' || t[i].is_alphanumeric() || t[i] == '_') {
        return None;
    }
    let word = |mut j: usize| {
        while j < t.len()
            && !t[j].is_whitespace()
            && !matches!(t[j], '<' | '>' | '"' | '`' | '│' | '…' | '|')
        {
            j += 1;
        }
        j
    };
    let hit = |j: usize| {
        let mut e = j;
        while e > i
            && matches!(
                t[e - 1],
                '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | ')' | ']' | '}'
            )
        {
            e -= 1;
        }
        let s: String = t[i..e].iter().collect();
        let ok = s.chars().count() >= 2
            && (s.contains('/') || s.contains('.') || s.starts_with('~'))
            && s.chars().any(|c| c.is_alphanumeric());
        (ok && noname::sys::exists(noname::sys::split_line(&s).0)).then_some(e)
    };
    // a path can hold spaces ("~/project The Researcher"): a path with a `/` tries up to
    // 8 more words after single spaces and keeps the longest one that exists
    let mut j = word(i);
    let mut best = None;
    for n in 0..=8 {
        best = hit(j).or(best);
        if n == 8 || t.get(j) != Some(&' ') || !t[i..j].contains(&'/') {
            break;
        }
        j = word(j + 1);
    }
    best
}

/// The panel lines (menus and pickers above the input) cut to `h` rows. When they do not
/// fit, the window scrolls so the selected row (the one starting with "› ") stays in view,
/// with "↑ n more" / "↓ n more" on the first and last row when there is more.
fn fit_panel(v: Vec<Line<'static>>, h: usize) -> Vec<Line<'static>> {
    let n = v.len();
    if n <= h {
        return v;
    }
    if h == 0 {
        return vec![];
    }
    let foc = v
        .iter()
        .position(|l| l.spans.first().is_some_and(|s| s.content == "› "))
        .unwrap_or(0);
    // rows for content: the arrows take a row each when there is more on that side
    let mut s = foc.saturating_sub(h / 2).min(n - h);
    let up = |s: usize| s > 0 && h >= 3;
    let down = |s: usize| s + h < n && h >= 3;
    // keep the selected row off the arrow rows
    while up(s) && foc <= s && s > 0 {
        s -= 1;
    }
    while down(s) && foc + 1 >= s + h && s + h < n {
        s += 1;
    }
    let (a, b) = (up(s), down(s));
    let mut o: Vec<Line<'static>> = v[s..s + h].to_vec();
    let arrow = |t: String| Line::from(vec![pad(2), Span::styled(t, st(FA()))]);
    if a {
        o[0] = arrow(format!("↑ {} more", s + 1));
    }
    if b {
        let k = o.len() - 1;
        o[k] = arrow(format!("↓ {} more", n - (s + h) + 1));
    }
    o
}

fn body(a: &App, w: usize) -> Vec<Line<'static>> {
    LK.with(|l| l.borrow_mut().clear());
    let m = mdt();
    let mut v: Vec<Line<'static>> = vec![];
    let mut pk = 0u8;
    let mut tl0 = 0usize;
    let lu = a.bs.iter().rposition(|b| matches!(b, B::U(..)));
    let fly = a.ts > 0 && a.hist.last().is_some_and(|h| !h.starts_with('/'));
    let gs = groups(a);
    for (i, b) in a.bs.iter().enumerate() {
        if hid(a, i, b) {
            continue;
        }
        if let Some((s0, ix)) = &gs[i] {
            if *s0 != i {
                continue;
            }
            if !v.is_empty() && (pk != 4 || v.len() > tl0 + 1) {
                v.push(Line::default());
            }
            pk = 4;
            tl0 = v.len();
            v.extend(gline(a, ix, w));
            continue;
        }
        let k = match b {
            B::U(..) => 1,
            B::T(_) => 2,
            B::Th(_) => 3,
            B::Tl(_) => 4,
            B::Ask(_) => 7,
            B::Sys(_) | B::Qa(..) => 5,
            B::Rc(..) => 6,
        };
        if !v.is_empty() && k != 6 && (k != pk || k == 1 || k == 2 || (k == 4 && v.len() > tl0 + 1))
        {
            v.push(Line::default());
            if k == 1 {
                v.push(Line::default());
            }
        }
        pk = k;
        tl0 = v.len();
        match b {
            B::U(s, _) => {
                let el = if fly && lu == Some(i) {
                    a.t.saturating_sub(a.ts) * 3
                } else {
                    u64::MAX / 2
                };
                for (j, l) in wrap(s, w - 4).into_iter().enumerate() {
                    let mut sp = vec![Span::styled(
                        if j == 0 { "> " } else { "  " },
                        st(LI()).bold(),
                    )];
                    sp.extend(decode(
                        &l,
                        el.saturating_sub(j as u64 * 30),
                        a.t,
                        300 + j as u64,
                        st(WH()).bold(),
                        &gd(),
                    ));
                    v.push(Line::from(sp));
                }
            }
            B::Th(s) if s.trim().is_empty() => {
                let mut sp = vec![pad(2), Span::styled("~ ", st(FA()))];
                sp.extend(noise(6, a.t, i as u64, &gd()));
                sp.push(Span::styled("  thinking", st(FA())));
                v.push(Line::from(sp));
            }
            B::Th(s) => {
                let ls = wrap(s, w - 6);
                let n = if a.open || a.streaming() == Some(i) {
                    ls.len()
                } else {
                    1
                };
                for (j, l) in ls.iter().take(n).enumerate() {
                    let mut sp = vec![
                        pad(2),
                        Span::styled(if j == 0 { "~ " } else { "  " }, st(FA())),
                        Span::styled(l.clone(), st(MU()).italic()),
                    ];
                    if j == n - 1 && n < ls.len() {
                        sp.push(Span::styled(" ...", st(FA())));
                    }
                    v.push(Line::from(sp));
                }
                if a.streaming() == Some(i) {
                    tail_nz(&mut v, a, i as u64);
                }
            }
            B::T(s) => {
                for l in md(s, w - 2, &m) {
                    let mut sp = vec![pad(2)];
                    sp.extend(l.spans);
                    v.push(Line::from(sp));
                }
                if a.streaming() == Some(i) {
                    tail_nz(&mut v, a, i as u64);
                }
            }
            B::Tl(t) => {
                let (c, col) = sy(t.k);
                let sd = i as u64 * 7 + 3;
                let ea = if t.t0 > 0 {
                    a.t.saturating_sub(t.t0)
                } else {
                    a.t
                };
                let ef = if t.t0 > 0 {
                    a.t.saturating_sub(t.t1)
                } else {
                    a.t
                };
                let mut l = vec![pad(2)];
                if t.st == St::Run {
                    l.extend(noise(1, a.t, sd, &gd()));
                } else {
                    let cc = if t.st == St::Err { RD() } else { col };
                    l.push(Span::styled(
                        if t.st == St::Err { "x" } else { c }.to_string(),
                        st(cc).bold(),
                    ));
                }
                l.push(pad(1));
                l.push(Span::styled(format!("{:<6} ", t.nm), st(MU())));
                l.extend(decode(&t.a, ea, a.t, sd, st(FG()), &gd()));
                let r = if t.st == St::Run {
                    let mut r = noise(6, a.t, sd + 1, &gd());
                    r.push(Span::styled(format!("  {:.1}s", a.secs(t)), st(FA())));
                    r
                } else if t.st == St::Err {
                    vec![Span::styled(t.sum.clone(), st(RD()))]
                } else if t.k == Tk::Edit {
                    let (ad, de) = t.sum.split_once(' ').unwrap_or((&t.sum, ""));
                    let mut r = decode(ad, ef, a.t, sd, st(GR()), &gd());
                    r.push(pad(1));
                    r.extend(decode(
                        de,
                        ef.saturating_sub(2),
                        a.t,
                        sd + 5,
                        st(RD()),
                        &gd(),
                    ));
                    r
                } else if t.k == Tk::Bash && !a.open {
                    let n = t.body.len();
                    let s = if n == 0 {
                        "no output".to_string()
                    } else if n == 1 {
                        "1 line".into()
                    } else {
                        format!("{n} lines")
                    };
                    decode(&s, ef, a.t, sd, st(FA()), &gd())
                } else {
                    decode(&t.sum, ef, a.t, sd, st(FA()), &gd())
                };
                let line = lr(l, r, w);
                if matches!(t.k, Tk::Read | Tk::Edit) && !t.a.is_empty() {
                    link(
                        v.len(),
                        11,
                        unicode_width::UnicodeWidthStr::width(t.a.as_str())
                            .min(w.saturating_sub(12)),
                        &t.a,
                    );
                }
                if t.st == St::Err {
                    let el = a.t.saturating_sub(t.t1);
                    let mut sp: Vec<Span<'static>> = line
                        .spans
                        .into_iter()
                        .map(|x| {
                            if x.style.fg == Some(FG()) || x.style.fg == Some(MU()) {
                                Span::styled(x.content, st(RD()))
                            } else {
                                x
                            }
                        })
                        .collect();
                    sp = glitch(sp, el, a.t);
                    v.push(Line::from(sp));
                } else {
                    v.push(line);
                }
                if t.k == Tk::Task && t.st == St::Run && !a.open {
                    let ps: Vec<&String> = t
                        .body
                        .iter()
                        .filter_map(|l| if let Ln::P(x) = l { Some(x) } else { None })
                        .collect();
                    if ps.len() > 3 {
                        v.push(Line::from(vec![
                            pad(4),
                            Span::styled("| ", st(FA())),
                            Span::styled(
                                format!("+{} more tool uses", ps.len() - 3),
                                st(FA()).italic(),
                            ),
                        ]));
                    }
                    for x in ps
                        .iter()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        let mut x = (*x).clone();
                        if x.chars().count() > w.saturating_sub(10) {
                            x = x.chars().take(w.saturating_sub(11)).collect::<String>() + "~";
                        }
                        v.push(Line::from(vec![
                            pad(4),
                            Span::styled("| ", st(FA())),
                            Span::styled(x, st(FA())),
                        ]));
                    }
                }
                if t.k == Tk::Bash && t.st == St::Ok && !a.open && !t.body.is_empty() {
                    let n = t.body.len();
                    let k = if n > 5 { 4 } else { n };
                    if n > k {
                        v.push(Line::from(vec![
                            pad(4),
                            Span::styled("| ", st(FA())),
                            Span::styled(format!("+{} lines · ctrl+o", n - k), st(FA()).italic()),
                        ]));
                    }
                    for ln in &t.body[n - k..] {
                        if let Ln::P(x) = ln {
                            let mut x = x.clone();
                            if x.chars().count() > w.saturating_sub(8) {
                                x = x.chars().take(w.saturating_sub(9)).collect::<String>() + "~";
                            }
                            let c = if x.contains("FAIL")
                                || (x.contains("failed") && !x.contains(" 0 failed"))
                                || x.starts_with("error")
                            {
                                RD()
                            } else {
                                MU()
                            };
                            v.push(Line::from(vec![
                                pad(4),
                                Span::styled("| ", st(FA())),
                                Span::styled(x, st(c)),
                            ]));
                        }
                    }
                }
                let show = t.st != St::Run && (t.k == Tk::Edit || t.st == St::Err || a.open);
                let auto = t.k == Tk::Edit || t.st == St::Err;
                let base = if auto {
                    if t.t0 > 0 { t.t1 } else { 0 }
                } else {
                    a.to
                };
                let kk = if auto && a.open {
                    usize::MAX
                } else {
                    a.t.saturating_sub(base) as usize + 1
                };
                if show {
                    let el = a.t.saturating_sub(base);
                    let nd = t.body.iter().filter(|x| matches!(x, Ln::D(..))).count() as u64;
                    let fx = t.k == Tk::Edit && !a.open;
                    let (mut jd, mut ja) = (0u64, 0u64);
                    for ln in t.body.iter().take(if fx { usize::MAX } else { kk }) {
                        let (n, s, c, mk) = match ln {
                            Ln::P(s) => (
                                String::new(),
                                s.clone(),
                                if s.contains("FAIL") || s.contains("failed") {
                                    RD()
                                } else {
                                    MU()
                                },
                                ' ',
                            ),
                            Ln::A(n, s) => (fnum(*n), s.clone(), GR(), '+'),
                            Ln::D(n, s) => (fnum(*n), s.clone(), RD(), '-'),
                            Ln::C(n, s) => (fnum(*n), s.clone(), FA(), ' '),
                        };
                        let mut s = s;
                        if s.chars().count() > w.saturating_sub(14) {
                            s = s.chars().take(w.saturating_sub(15)).collect::<String>() + "~";
                        }
                        if t.k == Tk::Grep {
                            if let Ln::P(x) = ln {
                                let tok = x.split_whitespace().next().unwrap_or("");
                                let fp = tok.split(':').next().unwrap_or("");
                                if !fp.is_empty() && std::path::Path::new(fp).is_file() {
                                    let tg = tok
                                        .trim_end_matches(':')
                                        .split(':')
                                        .take(2)
                                        .collect::<Vec<_>>()
                                        .join(":");
                                    link(v.len(), 12, tok.chars().count(), &tg);
                                }
                            }
                        }
                        let mut sp = vec![
                            pad(4),
                            Span::styled("| ", st(FA())),
                            Span::styled(format!("{n:>3} "), st(FA())),
                        ];
                        match ln {
                            Ln::D(..) if fx => {
                                let ts = nd + 5 + jd;
                                jd += 1;
                                if el < jd {
                                    continue;
                                }
                                let (col, md) = if el < ts {
                                    (RD(), Modifier::empty())
                                } else {
                                    let f = ((el - ts) as f32 / 10.0).min(1.0);
                                    (
                                        grad(&[RD(), grad(&[RD(), BASE()], 0.5)], f),
                                        Modifier::CROSSED_OUT,
                                    )
                                };
                                sp.push(Span::styled("- ", st(col)));
                                sp.push(Span::styled(s, st(col).add_modifier(md)));
                            }
                            Ln::A(..) if fx => {
                                let sa = if nd > 0 { nd + 7 } else { 0 } + ja * 2;
                                ja += 1;
                                if el < sa {
                                    continue;
                                }
                                let nc = ((el - sa) * 6) as usize;
                                let cs: Vec<char> = s.chars().collect();
                                sp.push(Span::styled("+ ", st(GR())));
                                if nc >= cs.len() {
                                    sp.push(Span::styled(s, st(GR())));
                                } else {
                                    sp.push(Span::styled(
                                        cs[..nc].iter().collect::<String>(),
                                        st(GR()),
                                    ));
                                    sp.push(Span::styled(cs[nc].to_string(), st(WH()).bold()));
                                }
                            }
                            _ => {
                                sp.push(Span::styled(format!("{mk} "), st(c)));
                                sp.push(Span::styled(s, st(c)));
                            }
                        }
                        v.push(Line::from(sp));
                    }
                    pk = 0;
                }
            }
            B::Ask(k) => match k.ans {
                None => {
                    let iw = w.min(64) - 4;
                    let pc = grad(&[PK(), grad(&[PK(), BASE()], 0.75)], pulse(a.t, 60.0));
                    v.push(Line::from(vec![
                        pad(2),
                        Span::styled("+- ", st(pc)),
                        Span::styled("permission", st(PK()).bold()),
                        Span::styled(format!(" {}+", "-".repeat(iw.saturating_sub(13))), st(pc)),
                    ]));
                    let row = |sp: Vec<Span<'static>>| {
                        let n = sw(&sp);
                        let mut x = vec![pad(2), Span::styled("| ", st(pc))];
                        x.extend(sp);
                        x.push(pad(iw.saturating_sub(n + 1)));
                        x.push(Span::styled("|", st(pc)));
                        Line::from(x)
                    };
                    v.push(row(vec![]));
                    v.push(row(vec![Span::styled(k.cmd.clone(), st(WH()).bold())]));
                    v.push(row(vec![]));
                    v.push(row(vec![
                        Span::styled("[y]", st(GR()).bold()),
                        Span::styled(" allow   ", st(FG())),
                        Span::styled("[n]", st(RD()).bold()),
                        Span::styled(" deny   ", st(FG())),
                        Span::styled("[a]", st(LI()).bold()),
                        Span::styled(" always allow", st(FG())),
                    ]));
                    v.push(Line::from(vec![
                        pad(2),
                        Span::styled(format!("+{}+", "-".repeat(iw)), st(pc)),
                    ]));
                }
                Some(c) => {
                    let (t, col) = if c == 'n' {
                        ("denied", RD())
                    } else {
                        ("allowed", GR())
                    };
                    v.push(lr(
                        vec![
                            pad(2),
                            Span::styled("! ", st(PK()).bold()),
                            Span::styled(format!("{:<6}", "ask"), st(MU())),
                            Span::styled(k.cmd.clone(), st(FG())),
                        ],
                        vec![Span::styled(t, st(col))],
                        w,
                    ));
                }
            },
            B::Qa(q, r) => {
                v.push(Line::from(vec![
                    pad(2),
                    Span::styled("? ", st(CY()).bold()),
                    Span::styled(q.clone(), st(MU())),
                ]));
                v.push(Line::from(vec![pad(4), Span::styled(r.clone(), st(WH()))]));
            }
            B::Rc(x, t0) => {
                let el = a.t.saturating_sub(*t0) * 2;
                let mut sp = vec![pad(2)];
                sp.extend(decode(x, el, a.t, i as u64 + 900, st(FA()), &gd()));
                v.push(Line::from(sp));
            }
            B::Sys(s) => v.push(Line::from(vec![
                pad(2),
                Span::styled("- ", st(FA())),
                Span::styled(s.clone(), st(MU())),
            ])),
        }
    }
    if a.busy
        && a.ask.is_none()
        && a.streaming().is_none()
        && !matches!(a.bs.last(), Some(B::Tl(t)) if t.st == St::Run)
    {
        v.push(Line::default());
        let mut sp = vec![pad(2)];
        sp.extend(noise(8, a.t, 99, &gd()));
        match &a.retry {
            Some((m, until)) => {
                let s = until.saturating_sub(a.t) as f32 / 30.0;
                sp.push(Span::styled(
                    format!(
                        "  retrying{}  ",
                        if s >= 0.5 {
                            format!(" in {s:.0}s")
                        } else {
                            String::new()
                        }
                    ),
                    st(PE()),
                ));
                sp.push(Span::styled(m.clone(), st(FA())));
            }
            None => sp.push(Span::styled(
                format!("  thinking  {:.1}s", a.turn_secs()),
                st(FA()),
            )),
        }
        v.push(Line::from(sp));
    }
    v
}

impl V for V1 {
    fn key(&mut self, k: &Ke, a: &mut App) -> bool {
        if a.fs.is_some() {
            return false;
        }
        let am = a.atmenu();
        if !am.is_empty() && a.rw.is_none() && a.pk.is_none() && a.tk.is_none() {
            let sel = a.ms.min(am.len() - 1);
            match k.code {
                K::Up => {
                    a.ms = sel.saturating_sub(1);
                    return true;
                }
                K::Down => {
                    a.ms = (sel + 1).min(am.len() - 1);
                    return true;
                }
                K::Tab | K::Right | K::Enter if k.modifiers.is_empty() => {
                    a.atpick(&am[sel]);
                    return true;
                }
                K::Esc => {
                    a.fhide = a.attok().map(|x| x.1);
                    return true;
                }
                _ => {}
            }
        }
        let m = menu(a);
        if !m.is_empty() && a.rw.is_none() && a.rs.is_none() {
            let sel = a.ms.min(m.len() - 1);
            match k.code {
                K::Up => {
                    a.ms = sel.saturating_sub(1);
                    return true;
                }
                K::Down => {
                    a.ms = (sel + 1).min(m.len() - 1);
                    return true;
                }
                K::Tab | K::Right => {
                    a.inp = format!("/{} ", m[sel].0);
                    a.cur = a.inp.chars().count();
                    a.ms = 0;
                    return true;
                }
                K::Enter if k.modifiers.is_empty() => {
                    a.inp = format!("/{}", m[sel].0);
                    a.cur = a.inp.chars().count();
                    a.ms = 0;
                    a.submit();
                    return true;
                }
                _ => {}
            }
        }
        let end = a.cur == a.inp.chars().count();
        if matches!(k.code, K::Tab | K::Right) && end && a.rw.is_none() {
            if let Some((r, _)) = ghost(a) {
                a.inp.push_str(&r);
                a.inp.push(' ');
                a.cur = a.inp.chars().count();
                return true;
            }
            if a.inp.is_empty() {
                if let Some(x) = a.sug.take() {
                    a.inp = x;
                    a.cur = a.inp.chars().count();
                    return true;
                }
            }
            return k.code == K::Tab;
        }
        false
    }

    fn scen(&mut self, s: &str, a: &mut App) {
        match s {
            "plan" => a.mode = 1,
            "byp" => a.mode = 3,
            "morph" => {
                a.mode = 2;
                a.t = 5;
                a.tm = 1;
            }
            _ => {}
        }
        if s == "tasks" || s == "taskd" {
            let mk = |kind: u8,
                      desc: &str,
                      st: u8,
                      t0: u64,
                      t1: u64,
                      tools: u64,
                      toks: u64,
                      out: &str,
                      acts: &[&str]| Task {
                id: desc.into(),
                kind,
                desc: desc.into(),
                st,
                t0,
                t1,
                tool: String::new(),
                last: acts.last().map(|x| x.to_string()).unwrap_or_default(),
                tools,
                toks,
                out: out.into(),
                acts: acts.iter().map(|x| x.to_string()).collect(),
                ..Default::default()
            };
            a.tasks = vec![
                mk(
                    0,
                    "cargo watch -x test",
                    0,
                    1,
                    0,
                    0,
                    0,
                    "running 14 tests\ntest feed::tick ... ok\ntest swipe::snap ... ok\ntest result: ok. 14 passed",
                    &[],
                ),
                mk(
                    1,
                    "tail -f logs/app.log | grep ERROR",
                    0,
                    1,
                    0,
                    0,
                    0,
                    "",
                    &[],
                ),
                mk(
                    2,
                    "explore: where is the feed rendered",
                    0,
                    1,
                    0,
                    14,
                    32100,
                    "",
                    &["grep render  src", "read src/feed.rs", "read src/ui/mod.rs"],
                ),
                mk(3, "review-changes", 1, 1, 400, 0, 0, "", &[]),
            ];
            a.t = a.t.max(900);
            a.tk = Some((if s == "taskd" { 0 } else { 2 }, s == "taskd"));
        }
        if s == "agents" {
            let ag = |desc: &str, st: u8, t0: u64, tools: u64, toks: u64, last: &str| Task {
                id: desc.into(),
                kind: 2,
                desc: desc.into(),
                st,
                t0,
                t1: if st == 0 { 0 } else { t0 + 300 },
                last: last.into(),
                tools,
                toks,
                ..Default::default()
            };
            a.tasks = vec![
                ag(
                    "research: VTE clipboard support",
                    1,
                    10,
                    9,
                    21400,
                    "read vte docs",
                ),
                ag(
                    "research: OSC 8 hyperlinks",
                    0,
                    10,
                    6,
                    15800,
                    "WebFetch gitlab.gnome.org/GNOME/vte",
                ),
                ag(
                    "research: wl-clipboard on GNOME",
                    0,
                    12,
                    3,
                    7100,
                    "WebSearch wl-copy mutter data-control",
                ),
                Task {
                    id: "sh".into(),
                    kind: 0,
                    desc: "cargo build --release".into(),
                    st: 0,
                    t0: 30,
                    ..Default::default()
                },
                Task {
                    id: "wf".into(),
                    kind: 3,
                    desc: "review the changes".into(),
                    name: "deep-research".into(),
                    st: 0,
                    t0: 20,
                    wf: vec![(1, 1), (2, 1), (3, 0), (4, 0), (5, 0)],
                    phase: "Verify".into(),
                    tools: 31,
                    toks: 88200,
                    ..Default::default()
                },
            ];
            a.t = a.t.max(900);
        }
        if s == "chat" {
            let tl = |k: Tk, nm: &str, ar: &str, sum: &str, st: St, body: Vec<Ln>| {
                B::Tl(noname::Tool {
                    k,
                    nm: nm.into(),
                    a: ar.into(),
                    sum: sum.into(),
                    body,
                    st,
                    t0: 1,
                    t1: 2,
                })
            };
            let p = |x: &[&str]| x.iter().map(|s| Ln::P(s.to_string())).collect::<Vec<_>>();
            a.bs = vec![
                B::U("compare the three renderers and run the tests".into(), String::new()),
                B::T("Here is the comparison:\n\n| renderer | fps | memory | notes |\n|---|---:|---:|---|\n| ratatui | 60 | 2.1 MB | diffing backend, **fast** on large buffers |\n| crossterm raw | 30 | 1.4 MB | no diffing, writes every cell each frame which flickers in some terminals |\n| notcurses | 120 | 8.9 MB | `C` dependency |\n\nTests next.".into()),
                tl(Tk::Task, "agent", "explore: where is the feed rendered", "14 tools · 32.1k tok · 45s", St::Ok, p(&["grep render  src", "read src/feed.rs"])),
                tl(Tk::Task, "agent", "research: wl-clipboard on GNOME", "", St::Run, p(&["web wl-copy mutter data-control", "read docs/clip.md", "bash rg -n clipboard src", "web gitlab.gnome.org/GNOME/mutter/-/issues/524", "read src/img.rs"])),
                tl(Tk::Bash, "bash", "cargo test --release 2>&1 | tail -20", "test result: ok. 14 passed; 0 failed", St::Ok, p(&["running 14 tests", "test feed::tick ... ok", "test swipe::snap ... ok", "test swipe::fling ... ok", "test md::table ... ok", "test result: ok. 14 passed; 0 failed"])),
                tl(Tk::Bash, "bash", "git status --short", "ok", St::Ok, p(&[" M src/main.rs", " M src/lib.rs"])),
                tl(Tk::Bash, "bash", "cargo build --release", "", St::Run, vec![]),
            ];
            a.t = a.t.max(900);
        }
        if s == "slash" {
            a.inp = "/co".into();
            a.cur = 3;
        }
    }

    fn draw(&mut self, f: &mut Frame, a: &App) {
        let ar = f.area();
        // the frame needs 7 rows (input, status and borders) and some width; below that
        // say so on one line instead of drawing past the edge
        if ar.height < 7 || ar.width < 24 {
            let t = if ar.height < 7 {
                "noname: make the window taller"
            } else {
                "noname: wider"
            };
            let t: String = t.chars().take(ar.width as usize).collect();
            f.render_widget(
                Paragraph::new(Span::styled(t, st(MU()))),
                Rect {
                    height: ar.height.min(1),
                    ..ar
                },
            );
            return;
        }
        let w = (ar.width as usize).saturating_sub(6);
        let x0 = ar.x + (ar.width - w as u16) / 2;
        let iw = w - 2;
        let (il, (cx, cy)) = match &a.fs {
            Some((q, _)) => (
                vec![q.clone()],
                (unicode_width::UnicodeWidthStr::width(q.as_str()), 0),
            ),
            None => inwrap(&a.inp, a.cur, iw),
        };
        let ih = il.len().clamp(1, (ar.height as usize / 3).max(8));
        let mut extra: Vec<Line<'static>> = vec![];
        let mut bt: Vec<Line<'static>> = vec![];
        if let Some(b) = &a.btw {
            let l = vec![
                pad(2),
                Span::styled("btw  ", st(L2()).bold()),
                Span::styled(b.q.clone(), st(MU())),
            ];
            bt.push(Line::from(l));
            for l in md(&b.a, w - 9, &mdt()) {
                let mut sp = vec![pad(7)];
                sp.extend(l.spans);
                bt.push(Line::from(sp));
            }
            if !b.done() {
                tail_nz(&mut bt, a, 77);
            }
        }
        if a.tk.is_none() {
            agents(&mut bt, a, w);
        }
        let bwh = bt.len() as u16;
        if let Some((sel, opt)) = a.rw {
            let us = a.users();
            let n = us.len();
            let lo = sel.saturating_sub(6).min(n.saturating_sub(7));
            extra.push(Line::from(Span::styled("rewind", st(L2()).bold())));
            for (j, &bi) in us.iter().enumerate().skip(lo).take(7) {
                let t = match a.bs.get(bi) {
                    Some(B::U(t, _)) => t.lines().next().unwrap_or("").to_string(),
                    _ => String::new(),
                };
                let t: String = if t.chars().count() > w - 8 {
                    t.chars().take(w - 9).collect::<String>() + "…"
                } else {
                    t
                };
                let on = j == sel;
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(t, if on { st(WH()).bold() } else { st(MU()) }),
                ]));
                if on {
                    if let Some(o) = opt {
                        let mut sp = vec![pad(4)];
                        for (k, l) in ["code + conversation", "conversation", "code"]
                            .iter()
                            .enumerate()
                        {
                            if k > 0 {
                                sp.push(pad(4));
                            }
                            if k == o {
                                sp.push(Span::styled(format!("[ {l} ]"), st(LI()).bold()));
                            } else {
                                sp.push(Span::styled(format!("  {l}  "), st(FA())));
                            }
                        }
                        extra.push(Line::from(sp));
                    }
                }
            }
            extra.push(Line::default());
        }
        let am = a.atmenu();
        if !am.is_empty() && a.rw.is_none() {
            let sel = a.ms.min(am.len() - 1);
            let lo = sel.saturating_sub(5).min(am.len().saturating_sub(6));
            for (j, pth) in am.iter().enumerate().skip(lo).take(6) {
                let on = j == sel;
                let (d, b) = match pth.trim_end_matches('/').rsplit_once('/') {
                    Some((d, b)) => (
                        format!("{d}/"),
                        format!("{b}{}", if pth.ends_with('/') { "/" } else { "" }),
                    ),
                    None => (String::new(), pth.clone()),
                };
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled("@", st(FA())),
                    Span::styled(d, st(if on { MU() } else { FA() })),
                    Span::styled(b, if on { st(WH()).bold() } else { st(L2()) }),
                ]));
            }
            extra.push(Line::default());
        }
        let mm = menu(a);
        if !mm.is_empty() && a.rw.is_none() {
            let sel = a.ms.min(mm.len() - 1);
            let lo = sel.saturating_sub(5).min(mm.len().saturating_sub(6));
            let cw = mm
                .iter()
                .skip(lo)
                .take(6)
                .map(|x| x.0.chars().count())
                .max()
                .unwrap_or(0)
                .min(36)
                + 3;
            for (j, (c, d)) in mm.iter().enumerate().skip(lo).take(6) {
                let on = j == sel;
                let c: String = c.chars().take(36).collect();
                let dw = w.saturating_sub(cw + 4);
                let d: String = if d.chars().count() > dw {
                    d.chars().take(dw.saturating_sub(1)).collect::<String>() + "…"
                } else {
                    d.clone()
                };
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(
                        format!("/{c:<w$}", w = cw - 1),
                        if on { st(WH()).bold() } else { st(L2()) },
                    ),
                    Span::styled(d, st(if on { MU() } else { FA() })),
                ]));
            }
            if mm.len() > 6 {
                extra.push(Line::from(Span::styled(
                    format!("  {} more", mm.len() - 6),
                    st(FA()),
                )));
            }
            extra.push(Line::default());
        }
        if let Some(q) = &a.qa {
            let c = &q.qs[q.i];
            let hd = vec![Span::styled(
                if c.h.is_empty() {
                    "question".to_string()
                } else {
                    c.h.to_lowercase()
                },
                st(L2()).bold(),
            )];
            let rt = if q.qs.len() > 1 {
                vec![Span::styled(
                    format!("{}/{}", q.i + 1, q.qs.len()),
                    st(FA()),
                )]
            } else {
                vec![]
            };
            extra.push(lr(hd, rt, w));
            extra.push(Line::from(Span::styled(c.q.clone(), st(WH()).bold())));
            extra.push(Line::default());
            let lw = c
                .opts
                .iter()
                .map(|o| o.0.chars().count())
                .max()
                .unwrap_or(0)
                .min(40)
                + 3;
            for (j, (l, d)) in c.opts.iter().enumerate() {
                let on = j == q.sel;
                let mut sp = vec![Span::styled(if on { "› " } else { "  " }, st(LI()).bold())];
                if c.multi {
                    sp.push(Span::styled(
                        if q.pick[j] { "[x] " } else { "[ ] " },
                        st(if q.pick[j] { GR() } else { FA() }),
                    ));
                } else {
                    sp.push(Span::styled(format!("{}  ", j + 1), st(FA())));
                }
                let l: String = l.chars().take(40).collect();
                sp.push(Span::styled(
                    format!("{l:<lw$}"),
                    if on { st(WH()).bold() } else { st(FG()) },
                ));
                let dw = w.saturating_sub(lw + 10);
                let d: String = if d.chars().count() > dw {
                    d.chars().take(dw.saturating_sub(1)).collect::<String>() + "…"
                } else {
                    d.clone()
                };
                sp.push(Span::styled(d, st(if on { MU() } else { FA() })));
                extra.push(Line::from(sp));
            }
            let on = q.sel == c.opts.len();
            extra.push(Line::from(vec![
                Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                Span::styled(if c.multi { "    " } else { "   " }, st(FA())),
                Span::styled("other", if on { st(WH()).bold() } else { st(MU()) }),
            ]));
            extra.push(Line::default());
        }
        if let Some(sel) = a.ns {
            extra.push(Line::from(Span::styled("noname", st(L2()).bold())));
            let rows = a.nrows();
            let cw = rows.iter().map(|r| r.0.len()).max().unwrap_or(0) + 3;
            for (j, (k, v)) in rows.iter().enumerate() {
                let on = j == sel;
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(
                        format!("{k:<cw$}"),
                        if on { st(WH()).bold() } else { st(MU()) },
                    ),
                    Span::styled(v.clone(), st(if on { L2() } else { FA() })),
                ]));
            }
            extra.push(Line::default());
        }
        if let Some(p) = &a.pk {
            if p.kind == 4 {
                let sel = p.items.get(p.sel).map(|x| x.2.clone()).unwrap_or_default();
                extra.push(lr(
                    vec![
                        Span::styled("diff  ", st(L2()).bold()),
                        Span::styled(sel, st(WH()).bold()),
                    ],
                    vec![Span::styled(
                        format!("{}/{}", (p.off + 1).min(p.lines.len()), p.lines.len()),
                        st(FA()),
                    )],
                    w,
                ));
                let hh = (ar.height as usize / 2).clamp(6, 30);
                for (c, l) in p.lines.iter().skip(p.off).take(hh) {
                    let col = match c {
                        '+' => GR(),
                        '-' => RD(),
                        '@' => CY(),
                        _ => MU(),
                    };
                    let mut l = l.clone();
                    if l.chars().count() > w.saturating_sub(4) {
                        l = l.chars().take(w.saturating_sub(5)).collect::<String>() + "~";
                    }
                    if *c == '@' {
                        extra.push(Line::from(Span::styled(l, st(col))));
                    } else {
                        extra.push(Line::from(vec![
                            Span::styled(format!("{c} "), st(col)),
                            Span::styled(l, st(col)),
                        ]));
                    }
                }
            } else {
                extra.push(Line::from(Span::styled(p.title.clone(), st(L2()).bold())));
                if p.items.is_empty() {
                    extra.push(Line::from(
                        vec![pad(2)]
                            .into_iter()
                            .chain(noise(6, a.t, 610, &gd()))
                            .collect::<Vec<_>>(),
                    ));
                }
                let n = p.items.len();
                let lo = p.sel.saturating_sub(9).min(n.saturating_sub(10));
                let cw = p
                    .items
                    .iter()
                    .map(|x| x.0.chars().count())
                    .max()
                    .unwrap_or(0)
                    .min(48)
                    + 3;
                for (j, (l, rt, _)) in p.items.iter().enumerate().skip(lo).take(10) {
                    let on = j == p.sel;
                    let l: String = l.chars().take(48).collect();
                    let rc = if rt.contains("failed") || rt.contains("needs-auth") {
                        RD()
                    } else if rt.contains("connected") || rt.starts_with('+') {
                        GR()
                    } else {
                        FA()
                    };
                    extra.push(Line::from(vec![
                        Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                        Span::styled(
                            format!("{l:<cw$}"),
                            if on { st(WH()).bold() } else { st(FG()) },
                        ),
                        Span::styled(rt.clone(), st(if on { MU() } else { rc })),
                    ]));
                }
            }
            extra.push(Line::default());
        }
        if let Some((sel, det)) = a.tk {
            let n = a.tasks.len();
            let sel = sel.min(n.saturating_sub(1));
            let tsec =
                |t: &Task| (if t.t1 > 0 { t.t1 } else { a.t }).saturating_sub(t.t0) as f32 / 30.0;
            let stx = |t: &Task| -> (String, Color) {
                match t.st {
                    0 => (dur(tsec(t)), CY()),
                    1 => (format!("done  {}", dur(tsec(t))), GR()),
                    2 => ("failed".into(), RD()),
                    _ => ("stopped".into(), FA()),
                }
            };
            let meta = |t: &Task| -> String {
                let mut m = vec![];
                if t.tools > 0 {
                    m.push(format!("{} tools", t.tools));
                }
                if t.toks > 0 {
                    m.push(format!("{:.1}k tok", t.toks as f32 / 1000.0));
                }
                m.join(" · ")
            };
            if !det {
                extra.push(lr(
                    vec![Span::styled("tasks", st(L2()).bold())],
                    vec![Span::styled(
                        format!("{}/{}", a.tasks.iter().filter(|t| t.st == 0).count(), n),
                        st(FA()),
                    )],
                    w,
                ));
                let lo = sel.saturating_sub(7).min(n.saturating_sub(8));
                for (j, t) in a.tasks.iter().enumerate().skip(lo).take(8) {
                    let on = j == sel;
                    let (sym, kn, _) = TKIND[t.kind as usize];
                    let (sx, sc) = stx(t);
                    let m = meta(t);
                    let rt = if m.is_empty() {
                        sx.clone()
                    } else {
                        format!("{m}   {sx}")
                    };
                    let dw = w.saturating_sub(rt.chars().count() + 18);
                    let d: String = if t.desc.chars().count() > dw {
                        t.desc
                            .chars()
                            .take(dw.saturating_sub(1))
                            .collect::<String>()
                            + "…"
                    } else {
                        t.desc.clone()
                    };
                    let mut l = vec![Span::styled(if on { "› " } else { "  " }, st(LI()).bold())];
                    if t.st == 0 {
                        l.push(Span::styled(
                            sym,
                            st(grad(&[LI(), CY()], pulse(a.t + j as u64 * 9, 45.0))).bold(),
                        ));
                    } else {
                        l.push(Span::styled(sym, st(if t.st == 2 { RD() } else { FA() })));
                    }
                    l.push(Span::styled(format!(" {kn:<11}"), st(MU())));
                    l.push(Span::styled(d, if on { st(WH()).bold() } else { st(FG()) }));
                    let mut rr = vec![];
                    if !m.is_empty() {
                        rr.push(Span::styled(format!("{m}   "), st(FA())));
                    }
                    rr.push(Span::styled(sx, st(sc)));
                    extra.push(lr(l, rr, w));
                }
            } else if let Some(t) = a.tasks.get(sel) {
                let (sym, kn, _) = TKIND[t.kind as usize];
                let (sx, sc) = stx(t);
                extra.push(lr(
                    vec![
                        Span::styled(format!("{sym} {kn}  "), st(L2()).bold()),
                        Span::styled(t.desc.clone(), st(WH()).bold()),
                    ],
                    vec![Span::styled(sx, st(sc))],
                    w,
                ));
                let m = meta(t);
                if !m.is_empty() {
                    extra.push(Line::from(Span::styled(m, st(FA()))));
                }
                extra.push(Line::default());
                let src: Vec<String> = if t.kind <= 1 {
                    t.out
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string)
                        .collect()
                } else if !t.acts.is_empty() {
                    t.acts.clone()
                } else {
                    t.last.lines().map(str::to_string).collect()
                };
                let hh = (ar.height as usize / 3).clamp(4, 14);
                if src.is_empty() {
                    extra.push(Line::from(Span::styled("  no output yet", st(FA()))));
                }
                for l in src
                    .iter()
                    .rev()
                    .take(hh)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    let mut l = l.clone();
                    if l.chars().count() > w.saturating_sub(6) {
                        l = l.chars().take(w.saturating_sub(7)).collect::<String>() + "~";
                    }
                    extra.push(Line::from(vec![
                        Span::styled("| ", st(FA())),
                        Span::styled(l, st(MU())),
                    ]));
                }
                if t.kind == 2 && !t.last.is_empty() && t.acts.last() != Some(&t.last) {
                    extra.push(Line::from(vec![
                        Span::styled("  ", st(FA())),
                        Span::styled(
                            t.last.lines().next().unwrap_or("").to_string(),
                            st(FG()).italic(),
                        ),
                    ]));
                }
            }
            extra.push(Line::default());
        }
        if let Some(sel) = a.pp {
            extra.push(Line::from(Span::styled(
                "default permission",
                st(L2()).bold(),
            )));
            for (j, m) in MODES.iter().enumerate() {
                let on = j == sel;
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(m.to_string(), if on { st(WH()).bold() } else { st(MU()) }),
                ]));
            }
            extra.push(Line::default());
        }
        if let Some((sel, _)) = a.tp {
            extra.push(Line::from(Span::styled("theme", st(L2()).bold())));
            for (j, t) in pal::all().iter().enumerate() {
                let on = j == sel;
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(t.name, if on { st(WH()).bold() } else { st(MU()) }),
                ]));
            }
            extra.push(Line::default());
        }
        if let Some((sel, _)) = a.bp {
            let v = noname::bg::avail();
            extra.push(Line::from(Span::styled("background", st(L2()).bold())));
            let lo = sel.saturating_sub(8).min(v.len().saturating_sub(9));
            for (j, k) in v.iter().enumerate().skip(lo).take(9) {
                let on = j == sel;
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(k.to_string(), if on { st(WH()).bold() } else { st(MU()) }),
                ]));
            }
            extra.push(Line::default());
        }
        if let Some((all, sel)) = &a.rs {
            let v = a.rhits();
            let sel = (*sel).min(v.len().saturating_sub(1));
            let hd = vec![
                Span::styled("resume", st(L2()).bold()),
                Span::styled("  ", st(FA())),
                Span::styled(
                    if a.rq.is_empty() {
                        "type to search".to_string()
                    } else {
                        a.rq.clone()
                    },
                    if a.rq.is_empty() { st(FA()) } else { st(WH()) },
                ),
            ];
            extra.push(lr(
                hd,
                vec![Span::styled(format!("{}/{}", v.len(), all.len()), st(FA()))],
                w,
            ));
            extra.push(Line::default());
            let lo = sel.saturating_sub(5).min(v.len().saturating_sub(6));
            for (j, (_, t, meta)) in v.iter().enumerate().skip(lo).take(6) {
                let on = j == sel;
                let tw = w.saturating_sub(4);
                let t: String = if t.chars().count() > tw {
                    t.chars().take(tw.saturating_sub(1)).collect::<String>() + "…"
                } else {
                    t.clone()
                };
                extra.push(Line::from(vec![
                    Span::styled(if on { "› " } else { "  " }, st(LI()).bold()),
                    Span::styled(t, if on { st(WH()).bold() } else { st(FG()) }),
                ]));
                extra.push(Line::from(vec![
                    pad(2),
                    Span::styled(meta.clone(), st(if on { MU() } else { FA() })),
                ]));
            }
            if v.is_empty() {
                extra.push(Line::from(Span::styled("  no match", st(FA()))));
            }
            extra.push(Line::default());
        }
        if a.stodo && !a.todo.is_empty() {
            for (s, t) in &a.todo {
                let (m, c) = match s {
                    2 => ("[x] ", GR()),
                    1 => ("[>] ", LI()),
                    _ => ("[ ] ", FA()),
                };
                extra.push(Line::from(vec![
                    Span::styled(m, st(c)),
                    Span::styled(t.clone(), st(if *s == 2 { MU() } else { FG() })),
                ]));
            }
            extra.push(Line::default());
        }
        let h = ar.height;
        let top_in = lay(0) == 1;
        let tvis = match lay(4) {
            0 => !a.bs.iter().any(|b| matches!(b, B::U(..))),
            1 => true,
            _ => false,
        };
        let tp = lay(3);
        let lgw = LOGO[0].chars().count();
        let tvis = tvis && w >= lgw + 4 && h >= 24;
        let rt = if tvis && (1..=3).contains(&tp) {
            5u16
        } else {
            0
        };
        let rb = if tvis && tp == 4 { 5u16 } else { 0 };
        // Fit everything in the window: the input, the panels above it (menus, pickers)
        // and the rows under it share what the frame leaves, so nothing is drawn past the
        // bottom (ratatui panics there). The body keeps up to 3 lines (1 while a panel is open); a panel that does not
        // fit scrolls to its selected row, rows under the input end with "↓ n more".
        let ih = ih.min(h.saturating_sub(6 + rt + rb) as usize).max(1);
        let room = h.saturating_sub(ih as u16 + 6 + rt + rb);
        // an open menu or picker gets the room first; the conversation keeps one line then
        let room = room - room.min(if extra.is_empty() { 3 } else { 1 });
        let eh = (extra.len() as u16).min(room);
        let extra = fit_panel(extra, eh as usize);
        let bwh = bwh.min(room - eh);
        if bt.len() > bwh as usize {
            let more = bt.len() - bwh as usize + 1;
            bt.truncate((bwh as usize).saturating_sub(1));
            if bwh > 0 {
                bt.push(Line::from(vec![
                    pad(2),
                    Span::styled(format!("↓ {more} more"), st(FA())),
                ]));
            }
        }
        let bh = h.saturating_sub(ih as u16 + bwh + eh + 6 + rt + rb);
        let (top, ey, ry, iy, bty) = if top_in {
            let iy = 2u16 + rt;
            let ry = iy + ih as u16;
            let bty = ry + 1;
            let ey = bty + bwh;
            (ey + eh + 1, ey, ry, iy, bty)
        } else {
            let top = 2u16 + rt;
            let ey = top + bh + 1;
            let ry = ey + eh;
            let iy = ry + 1;
            (top, ey, ry, iy, iy + ih as u16)
        };
        let sy = if top_in {
            h.saturating_sub(1)
        } else {
            bty + bwh + 1 + rb
        };
        let lgx = match tp {
            1 => x0,
            2 => x0 + (w - lgw.min(w)) as u16 / 2,
            _ => x0 + (w - lgw.min(w)) as u16,
        };
        let lgel = a.t.saturating_sub(a.tc) * 2;
        let lgy = if rt > 0 {
            Some(0u16)
        } else if rb > 0 {
            Some(sy - 5)
        } else {
            None
        };
        if let Some(y0) = lgy {
            for (i, l) in LOGO.iter().enumerate() {
                let sp = unlig(decode(
                    l,
                    lgel.saturating_sub(i as u64 * 6),
                    a.t,
                    i as u64 + 50,
                    st(LI()),
                    &gd(),
                ));
                f.render_widget(
                    Paragraph::new(Line::from(sp)),
                    Rect {
                        x: lgx,
                        y: y0 + i as u16,
                        width: lgw as u16,
                        height: 1,
                    },
                );
            }
        }
        let col = |y: u16, hh: u16| Rect {
            x: x0,
            y,
            width: w as u16,
            height: hh,
        };

        let mid = tvis && tp == 0 && bh >= 6;
        if a.bs.is_empty() {
            a.cg.set((0, 0, 0, 0));
            if mid {
                let y = top + bh / 2 - 3;
                for (i, l) in LOGO.iter().enumerate() {
                    let sp = unlig(decode(
                        l,
                        lgel.saturating_sub(i as u64 * 6),
                        a.t,
                        i as u64 + 50,
                        st(LI()),
                        &gd(),
                    ));
                    f.render_widget(
                        Paragraph::new(Line::from(sp)).alignment(Alignment::Center),
                        col(y + i as u16, 1),
                    );
                }
            }
        } else {
            let bl = body(a, w);
            let mx = bl.len().saturating_sub(bh as usize);
            a.smax.set(mx);
            let tg = a.scr.min(mx) as f32;
            let n = a.t.saturating_sub(self.lt).min(60) as i32;
            self.sf += (tg - self.sf) * (1.0 - 0.55f32.powi(n));
            if (tg - self.sf).abs() < 0.4 {
                self.sf = tg;
            }
            let mut scr = self.sf.round() as usize;
            let nb = bl.len();
            // links of the visible lines, found with a margin so a wrapped link that
            // starts above or ends below the screen still opens whole
            let (v0, vn) = if nb <= bh as usize {
                (0, nb)
            } else {
                (nb - bh as usize - scr.min(nb - bh as usize), bh as usize)
            };
            let wl = links(&bl, v0.saturating_sub(16), (v0 + vn + 16).min(nb), w);
            a.bv.set((nb, bh as usize));
            *a.btx.borrow_mut() = bl
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            let ls = tail(bl, bh as usize, &mut scr);
            let pad_top = bh as usize - ls.len();
            let s0 = nb.saturating_sub(ls.len() + scr);
            a.cg.set((top + pad_top as u16, ls.len(), s0, x0));
            let mut lk = a.links.borrow_mut();
            lk.clear();
            LK.with(|l| {
                for (i, xa, xb, t) in l.borrow().iter() {
                    if *i >= s0 && *i < s0 + ls.len() {
                        let y = top + pad_top as u16 + (*i - s0) as u16;
                        lk.push((y, x0 + xa, x0 + xb, t.clone()));
                    }
                }
            });
            let y0 = top + pad_top as u16;
            let nl = ls.len();
            let mut ul = vec![];
            for (i, xa, xb, u) in wl {
                if i < s0 || i >= s0 + ls.len() {
                    continue;
                }
                let y = y0 + (i - s0) as u16;
                if !lk
                    .iter()
                    .any(|x| x.0 == y && x.1 < x0 + xb && x0 + xa < x.2)
                {
                    lk.push((y, x0 + xa, x0 + xb, u));
                    ul.push((y, x0 + xa, x0 + xb));
                }
            }
            drop(lk);
            f.render_widget(
                Paragraph::new(ls),
                col(top + pad_top as u16, bh - pad_top as u16),
            );
            let buf = f.buffer_mut();
            for (y, xa, xb) in ul {
                for x in xa..xb {
                    if let Some(c) = buf.cell_mut((x, y)) {
                        c.set_style(c.style().add_modifier(ratatui::style::Modifier::UNDERLINED));
                    }
                }
            }
            if let Some((_, ix)) = &a.fs {
                for (j, (i, x, hw)) in a.fhits().into_iter().enumerate() {
                    if i < s0 || i >= s0 + nl {
                        continue;
                    }
                    let y = y0 + (i - s0) as u16;
                    for xx in x0 + x as u16..(x0 + (x + hw) as u16).min(x0 + w as u16) {
                        if let Some(c) = buf.cell_mut((xx, y)) {
                            if j == *ix {
                                c.modifier.insert(Modifier::REVERSED | Modifier::BOLD);
                            } else {
                                c.modifier.insert(Modifier::UNDERLINED);
                                c.fg = PE();
                            }
                        }
                    }
                }
            }
            if scr > 0 {
                f.render_widget(
                    Paragraph::new(Span::styled(format!("↓ {scr}"), st(FA())))
                        .alignment(Alignment::Right),
                    col(top + bh - 1, 1),
                );
            }
            if mid {
                let y0 = top + bh / 2 - 2;
                let xl = x0 + (w - lgw) as u16 / 2;
                let c = grad(&[FA(), BASE()], 0.45);
                let buf = f.buffer_mut();
                let mut put = vec![];
                let clear = (y0.saturating_sub(1)..y0 + 5).all(|y| {
                    (xl.saturating_sub(2)..xl + lgw as u16 + 2)
                        .all(|x| buf.cell((x, y)).is_none_or(|k| k.symbol() == " "))
                });
                for (i, l) in LOGO.iter().enumerate() {
                    for (j, ch) in l.chars().enumerate() {
                        if clear && ch != ' ' {
                            put.push((xl + j as u16, y0 + i as u16, ch));
                        }
                    }
                }
                for (x, y, ch) in put {
                    if let Some(k) = buf.cell_mut((x, y)) {
                        k.set_char(ch).set_style(st(c));
                    }
                }
            }
        }
        f.render_widget(Paragraph::new(extra), col(ey, eh));
        let rule = Span::styled("─".repeat(w), st(FA()));
        let el = a.t.saturating_sub(a.ts);
        if a.ts > 0 && el < 20 {
            let head = (el as f32 / 14.0 * (w + 16) as f32) as i64;
            let sp: Vec<Span<'static>> = (0..w as i64)
                .map(|x| {
                    let d = head - x;
                    let c = if (0..16).contains(&d) {
                        grad(&[WH(), LI(), FA()], d as f32 / 16.0)
                    } else {
                        FA()
                    };
                    Span::styled("─", st(c))
                })
                .collect();
            f.render_widget(Paragraph::new(Line::from(sp)), col(ry, 1));
        } else {
            f.render_widget(Paragraph::new(rule.clone()), col(ry, 1));
        }
        let pc = if a.ask.is_some() || a.qa.is_some() {
            PK()
        } else {
            [LI(), CY(), GR(), RD()][a.mode]
        };
        let pg: Vec<Span<'static>> = if a.tm > 0 && a.t.saturating_sub(a.tm) < 9 {
            noise(1, a.t, 61, &gd())
        } else {
            vec![Span::styled(["›", "?", "»", "!"][a.mode], st(pc).bold())]
        };
        let gh = ghost(a);
        let mut il2: Vec<Line> = vec![];
        let sk = cy.saturating_sub(ih - 1);
        for (j, l) in il.iter().enumerate().skip(sk).take(ih) {
            let mut sp = if j == 0 { pg.clone() } else { vec![pad(1)] };
            sp.push(pad(1));
            let ff = a.inp.is_empty()
                && a.ts > 0
                && a.t.saturating_sub(a.ts) < 12
                && a.hist.last().is_some_and(|h| !h.starts_with('/'));
            if j == 0 && ff {
                let el = a.t.saturating_sub(a.ts);
                let h = a.hist.last().cloned().unwrap_or_default();
                for (ci, ch) in h.chars().take(w.saturating_sub(4)).enumerate() {
                    let r = hsh(ci as u64, 7) % 10;
                    if ch == ' ' || r + 2 < el {
                        sp.push(pad(1));
                    } else if r < el + 2 {
                        sp.push(Span::styled(
                            nz(a.t, ci as u64).to_string(),
                            st(grad(&gd(), (ci % 12) as f32 / 12.0)),
                        ));
                    } else {
                        sp.push(Span::styled(ch.to_string(), st(WH())));
                    }
                }
            } else {
                sp.extend(imgspans(l, st(WH()), st(PE()).bold()));
            }
            if j == 0 {
                if let Some((rest, _)) = &gh {
                    sp.push(Span::styled(rest.clone(), st(FA())));
                } else if a.inp.is_empty() && !a.busy && a.ask.is_none() {
                    if let Some(x) = &a.sug {
                        sp.push(Span::styled(
                            x.lines().next().unwrap_or("").to_string(),
                            st(FA()).italic(),
                        ));
                    }
                }
            }
            let dn = il.len().saturating_sub(sk + ih);
            let hint: Vec<Span<'static>> = if j == sk && sk > 0 {
                vec![Span::styled(format!("↑ {sk} "), st(FA()))]
            } else if j + 1 == sk + ih && il.len() > ih {
                let x = a.expand(&a.inp);
                let n = x.chars().count();
                let c = if n >= 1000 {
                    format!("{:.1}k", n as f32 / 1000.0)
                } else {
                    n.to_string()
                };
                let mut v = vec![];
                if dn > 0 {
                    v.push(Span::styled(format!("↓ {dn}   "), st(FA())));
                }
                v.push(Span::styled(
                    format!("{} lines · {c} chars ", x.lines().count()),
                    st(FA()),
                ));
                v
            } else {
                vec![]
            };
            il2.push(if hint.is_empty() {
                Line::from(sp)
            } else {
                lr(sp, hint, w)
            });
        }
        if let Some((q, ix)) = &a.fs {
            let n = a.fhits().len();
            let r = if q.is_empty() {
                "type to search  ·  esc close".to_string()
            } else if n == 0 {
                "no match  ·  esc close".to_string()
            } else {
                format!(
                    "{}/{n}  ·  enter older  ·  shift+enter newer  ·  esc close ",
                    ix + 1
                )
            };
            il2 = vec![lr(
                vec![
                    Span::styled("/", st(LI()).bold()),
                    pad(1),
                    Span::styled(q.clone(), st(WH())),
                ],
                vec![Span::styled(r, st(FA()))],
                w,
            )];
        }
        f.render_widget(Paragraph::new(il2), col(iy, ih as u16));
        a.ig.set((iy, ih as u16, iw));
        let cyy = cy - sk;
        setcur(f, a, x0 + 2 + cx as u16, iy + cyy as u16);
        f.render_widget(Paragraph::new(bt), col(bty, bwh));
        let tg = [a.ctx, a.h5, a.d7];
        let n = a.t.saturating_sub(self.lt).min(400) as i32;
        let k = 1.0 - 0.88f32.powi(n);
        for (d, t) in self.db.iter_mut().zip(tg) {
            *d += (t.max(0.0) - *d) * k;
        }
        self.lt = a.t;
        let db = self.db;
        // opencode and agy send no usage numbers, so its empty bars are left out instead of showing dashes
        let oc = a.is_opencode() || a.is_agy();
        let vis = bars();
        let nolabel = free::enabled() && free::claude_enabled();
        // the bars are `bw` cells wide; a narrow window shrinks them instead of dropping the usage
        let mk = |bw: usize| -> Vec<Vec<Span<'static>>> {
            let (c1, c2) = bar(db[0], bw, "▰", "▱");
            let (h1, h2) = bar(db[1], bw, "▰", "▱");
            let (d1, d2) = bar(db[2], bw, "▰", "▱");
            let pct = |j: usize| {
                Span::styled(
                    if tg[j] < 0.0 {
                        " –".to_string()
                    } else {
                        format!(" {:.0}%", db[j])
                    },
                    st(FG()),
                )
            };
            let segs = [
                vec![
                    Span::styled(if nolabel { "" } else { "context " }, st(FA())),
                    Span::styled(c1, st(LI())),
                    Span::styled(c2, st(FA())),
                    pct(0),
                ],
                vec![
                    Span::styled("5h ", st(FA())),
                    Span::styled(h1, st(CY())),
                    Span::styled(h2, st(FA())),
                    pct(1),
                ],
                vec![
                    Span::styled("week ", st(FA())),
                    Span::styled(d1, st(CY())),
                    Span::styled(d2, st(FA())),
                    pct(2),
                ],
            ];
            segs.into_iter()
                .enumerate()
                .filter(|(j, _)| vis[*j] && !(oc && tg[*j] < 0.0))
                .map(|(_, x)| x)
                .collect()
        };
        let shown = mk(10);
        let toast = match &a.toast {
            Some((t, e)) if !t.starts_with("mode") => {
                decode(t, a.t + 60 - e, a.t, 13, st(L2()), &gd())
            }
            _ => vec![],
        };
        let mut model = if a.work {
            vec![Span::styled("work  ", st(PK()).bold())]
        } else {
            vec![]
        };
        model.extend(decode(
            &a.mname.replace(' ', "-"),
            a.t / 2,
            a.t,
            41,
            st(LI()),
            &gd(),
        ));
        // effort sits on the usage line, on the side opposite the bars; with usage off it stays by the model
        let eff = (!a.effs.is_empty()).then(|| vec![Span::styled(a.effs.clone(), st(FA()))]);
        if lay(1) == 4 {
            if let Some(e) = &eff {
                model.push(pad(2));
                model.extend(e.clone());
            }
        }
        let cwd = vec![
            Span::styled(a.cwd.clone(), st(MU())),
            Span::styled(
                if a.br.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", a.br)
                },
                st(FA()),
            ),
        ];
        let mut rows: [[Vec<Ch>; 3]; 2] = Default::default();
        match lay(5) {
            0 => rows[0][0].push(Ch::S(cwd)),
            1 => rows[0][1].push(Ch::S(cwd)),
            2 => rows[0][2].insert(0, Ch::S(cwd)),
            3 => rows[1][0].push(Ch::S(cwd)),
            4 => rows[1][1].push(Ch::S(cwd)),
            5 => rows[1][2].push(Ch::S(cwd)),
            _ => {}
        }
        let mut mid: Vec<Span<'static>> = vec![];
        if let (true, Some(n)) = (free::enabled(), a.frv) {
            mid.extend([
                Span::styled("free ", st(FA())),
                Span::styled(free::short(n), st(FG())),
                Span::styled(" tok", st(FA())),
            ]);
        }
        if let (true, Some(n)) = (free::claude_enabled(), a.clv) {
            if !mid.is_empty() {
                mid.push(pad(3));
            }
            mid.extend([
                Span::styled("claude ", st(FA())),
                Span::styled(free::short(n), st(FG())),
                Span::styled(" tok", st(FA())),
            ]);
        }
        // the totals sit next to the effort label; with no effort side (or effort by the model) they stay centred
        let by_eff = eff.is_some() && lay(1) < 4;
        if !mid.is_empty() && !by_eff {
            rows[1][1].push(Ch::S(mid.clone()));
        }
        let run: Vec<usize> = (0..TKIND.len())
            .map(|k| {
                a.tasks
                    .iter()
                    .filter(|t| t.st == 0 && t.kind == k as u8)
                    .count()
            })
            .collect();
        if run.iter().sum::<usize>() > 0 {
            let mut sp = noise(1, a.t, 420, &gd());
            sp.push(pad(1));
            let mut first = true;
            for (k, &c) in run.iter().enumerate() {
                if c == 0 {
                    continue;
                }
                if !first {
                    sp.push(Span::styled(" · ", st(FA())));
                }
                first = false;
                sp.push(Span::styled(c.to_string(), st(tcol(k as u8)).bold()));
                sp.push(Span::styled(
                    format!(" {}", if c == 1 { TKIND[k].1 } else { TKIND[k].2 }),
                    st(tcol(k as u8)),
                ));
            }
            rows[1][2].push(Ch::S(sp));
        }
        if let Some(h) = a.hooks.iter().find(|h| a.t.saturating_sub(h.2) > 15) {
            let mut sp = noise(1, a.t, 430, &gd());
            sp.push(Span::styled(format!(" hook {}", h.1), st(MU())));
            rows[1][2].push(Ch::S(sp));
        }
        if !toast.is_empty() {
            rows[1][2].push(Ch::S(toast));
        }
        match lay(1) {
            0 => rows[1][0].push(Ch::U),
            1 => rows[1][2].push(Ch::U),
            2 => rows[0][0].push(Ch::U),
            3 => rows[0][2].push(Ch::U),
            _ => {}
        }
        match lay(2) {
            0 => rows[0][2].push(Ch::S(model)),
            1 => rows[1][2].push(Ch::S(model)),
            _ => {}
        }
        if let Some(e) = eff {
            let m = (!mid.is_empty() && by_eff).then(|| Ch::S(mid.clone()));
            // on the right-hand side the totals sit 2 columns left of the effort label
            let mr = m.as_ref().map(|_| {
                let mut v = mid.clone();
                // "medium" is two letters longer than "high": two columns less so the gap looks the same
                // (and so the context bar and the label still fit in a narrow window)
                let gap = if a.effs == "medium" { 0 } else { 2 };
                v.push(pad(gap));
                Ch::S(v)
            });
            match lay(1) {
                0 => rows[1][2].extend(mr.into_iter().chain([Ch::S(e)])),
                1 => {
                    rows[1][0].insert(0, Ch::S(e));
                    rows[1][0].splice(1..1, m);
                }
                2 => rows[0][2].extend(mr.into_iter().chain([Ch::S(e)])),
                3 => {
                    rows[0][0].insert(0, Ch::S(e));
                    rows[0][0].splice(1..1, m);
                }
                _ => {}
            }
        }
        for (ri, y) in [(0usize, rt), (1, sy)] {
            let [l, c, r] = &rows[ri];
            let c = join(c, &shown, 2);
            let cwn = sw(&c);
            let mut mode = 2;
            let (mut ls, mut rs) = (join(l, &shown, mode), join(r, &shown, mode));
            let fits = |ls: &[Span], rs: &[Span]| {
                if cwn > 0 {
                    sw(ls).max(sw(rs)) * 2 + cwn + 4 <= w
                } else {
                    sw(ls) + sw(rs) + 2 <= w
                }
            };
            while !fits(&ls, &rs) && mode > 0 {
                mode -= 1;
                ls = join(l, &shown, mode);
                rs = join(r, &shown, mode);
            }
            if mode == 0 && l.iter().chain(r).any(|x| matches!(x, Ch::U)) {
                // even one bar does not fit at full size: shorten the bars before giving up
                for bw in (3..10).rev() {
                    let sh = mk(bw);
                    let (l2, r2) = (join(l, &sh, 1), join(r, &sh, 1));
                    if fits(&l2, &r2) {
                        (ls, rs) = (l2, r2);
                        break;
                    }
                }
            }
            let cok = cwn > 0 && fits(&ls, &rs);
            f.render_widget(Paragraph::new(lr(ls, rs, w)), col(y, 1));
            if cok {
                let cx = x0 + (w.saturating_sub(cwn) / 2) as u16;
                f.render_widget(
                    Paragraph::new(Line::from(c)),
                    Rect {
                        x: cx,
                        y,
                        width: cwn as u16,
                        height: 1,
                    },
                );
            }
        }
        if self.bg.as_ref().map(|b| b.name) != a.bgw {
            let ar = f.area();
            self.bg = a
                .bgw
                .and_then(|k| noname::bg::Bg::new(k, ar.height, ar.width).ok());
        }
        if a.msel.is_some() || a.csel.is_some() {
            *a.txt.borrow_mut() = Some(f.buffer_mut().clone());
        }
        if let Some(b) = &mut self.bg {
            b.paint(f.buffer_mut(), a.t);
        }
    }
}

const LOGO: [&str; 4] = [
    r"   ____  ____  ____  ____ _____ ___  ___ ",
    r"  / __ \/ __ \/ __ \/ __ `/ __ `__ \/ _ \",
    r" / / / / /_/ / / / / /_/ / / / / / /  __/",
    r"/_/ /_/\____/_/ /_/\__,_/_/ /_/ /_/\___/ ",
];

#[derive(Clone)]
enum Ch {
    S(Vec<Span<'static>>),
    U,
}

fn join(v: &[Ch], us: &[Vec<Span<'static>>], mode: u8) -> Vec<Span<'static>> {
    let mut o: Vec<Span<'static>> = vec![];
    for x in v {
        let part: Vec<Span<'static>> = match x {
            Ch::S(s) => s.clone(),
            Ch::U if mode == 0 => vec![],
            Ch::U => {
                let mut p = vec![];
                for (j, sg) in us
                    .iter()
                    .take(if mode == 1 { 1 } else { usize::MAX })
                    .enumerate()
                {
                    if j > 0 {
                        p.push(pad(3));
                    }
                    p.extend(sg.clone());
                }
                p
            }
        };
        if part.is_empty() || sw(&part) == 0 {
            continue;
        }
        if !o.is_empty() {
            o.push(pad(4));
        }
        o.extend(part);
    }
    o
}

/// `████░░░░ 45%`; with no total yet a block sweeps through the bar.
fn pbar(done: usize, tot: usize, bw: usize, t: u64) -> Vec<Span<'static>> {
    if tot == 0 {
        let k = 4.min(bw);
        let x = (t as usize / 3) % (bw + k);
        let cell = |i: usize| if i + k > x && i <= x { "█" } else { "░" };
        let s: String = (0..bw).map(cell).collect();
        return vec![Span::styled(s, st(FA())), Span::styled("     ", st(FA()))];
    }
    let f = done as f32 / tot as f32;
    let k = ((f * bw as f32).round() as usize).min(bw);
    vec![
        Span::styled("█".repeat(k), st(LI())),
        Span::styled("░".repeat(bw - k), st(FA())),
        Span::styled(
            format!(" {:>3}%", (f * 100.0).round() as u32),
            st(FG()).bold(),
        ),
    ]
}

/// 950, 14.3k, 1.2m
fn ntok(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f32 / 1e3)
    } else {
        format!("{:.1}m", n as f32 / 1e6)
    }
}

/// Running background tasks under the prompt, like Claude Code shows them: one line per
/// task, the last activity under it, and a progress bar for workflows and agent batches.
fn agents(bt: &mut Vec<Line<'static>>, a: &App, w: usize) {
    let run: Vec<&Task> = a.tasks.iter().filter(|t| t.st == 0).collect();
    if run.is_empty() {
        return;
    }
    let cut = |s: &str, n: usize| -> String {
        if s.chars().count() > n {
            s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
        } else {
            s.to_string()
        }
    };
    let show = 4;
    for (j, t) in run.iter().take(show).enumerate() {
        let (sym, kn, _) = TKIND[t.kind as usize];
        let sx = dur(a.t.saturating_sub(t.t0) as f32 / 30.0);
        let dot = Span::styled(
            sym,
            st(grad(&[tcol(t.kind), FA()], pulse(a.t + j as u64 * 9, 45.0))).bold(),
        );
        if t.kind == 3 {
            // ◯ deep-research  ██████░░░░  40%   2m 26s · ↓ 1.2m tokens · ⚠ 1 failed
            let done = t.wf.iter().filter(|x| x.1 != 0).count();
            let bad = t.wf.iter().filter(|x| x.1 == 2).count();
            let nm = if t.name.is_empty() {
                t.desc.clone()
            } else {
                t.name.clone()
            };
            let mut rr = vec![Span::styled(sx, st(CY()))];
            if t.toks > 0 {
                rr.push(Span::styled(
                    format!(" · ↓ {} tokens", ntok(t.toks)),
                    st(FA()),
                ));
            }
            if bad > 0 {
                rr.push(Span::styled(format!(" · ⚠ {bad} failed"), st(RD())));
            }
            let bw = w.saturating_sub(sw(&rr) + 30).clamp(8, 24);
            let mut l = vec![
                pad(2),
                dot,
                Span::styled(format!(" {:<14} ", cut(&nm, 14)), st(FG()).bold()),
            ];
            l.extend(pbar(done, t.wf.len(), bw, a.t));
            bt.push(lr(l, rr, w));
            let mut l2 = vec![pad(4)];
            if !t.wf.is_empty() {
                l2.push(Span::styled(
                    format!("{done}/{} agents", t.wf.len()),
                    st(MU()),
                ));
            }
            if !t.phase.is_empty() {
                l2.push(Span::styled(
                    format!(
                        "{}{}",
                        if t.wf.is_empty() { "" } else { " · " },
                        cut(&t.phase, 30)
                    ),
                    st(FA()),
                ));
            }
            if l2.len() > 1 {
                bt.push(Line::from(l2));
            }
            continue;
        }
        let mut m = vec![];
        if t.tools > 0 {
            m.push(format!("{} tools", t.tools));
        }
        if t.toks > 0 {
            m.push(format!("↓ {} tokens", ntok(t.toks)));
        }
        let rt = if m.is_empty() {
            sx.clone()
        } else {
            format!("{}   {sx}", m.join(" · "))
        };
        let d = cut(&t.desc, w.saturating_sub(rt.chars().count() + 18));
        let l = vec![
            pad(2),
            dot,
            Span::styled(format!(" {kn:<11}"), st(tcol(t.kind))),
            Span::styled(d, st(FG())),
        ];
        let mut rr = vec![];
        if !m.is_empty() {
            rr.push(Span::styled(format!("{}   ", m.join(" · ")), st(FA())));
        }
        rr.push(Span::styled(sx, st(CY())));
        bt.push(lr(l, rr, w));
        if !t.last.is_empty() {
            let s = t.last.lines().next().unwrap_or("");
            bt.push(Line::from(vec![
                pad(14),
                Span::styled(cut(s, w.saturating_sub(16)), st(FA())),
            ]));
        }
    }
    // agents started together count as one batch: from the oldest running agent on
    let t0 = run.iter().filter(|t| t.kind == 2).map(|t| t.t0).min();
    let batch: Vec<&Task> = t0
        .map(|t0| {
            a.tasks
                .iter()
                .filter(|t| t.kind == 2 && t.t0 >= t0)
                .collect()
        })
        .unwrap_or_default();
    if batch.len() >= 2 {
        let done = batch.iter().filter(|t| t.st != 0).count();
        let mut l = vec![pad(2), Span::styled("agents      ", st(tcol(2)))];
        l.extend(pbar(done, batch.len(), 20, a.t));
        l.push(Span::styled(
            format!("  {done}/{} done", batch.len()),
            st(FA()),
        ));
        bt.push(Line::from(l));
    }
    let more = if run.len() > show {
        format!("+{} more  ·  ", run.len() - show)
    } else {
        String::new()
    };
    bt.push(Line::from(vec![
        pad(2),
        Span::styled(format!("{more}↓ to manage"), st(FA())),
    ]));
}

fn dur(s: f32) -> String {
    if s < 60.0 {
        format!("{s:.0}s")
    } else if s < 3600.0 {
        format!("{}m {:02}s", s as u64 / 60, s as u64 % 60)
    } else {
        format!("{}h {:02}m", s as u64 / 3600, s as u64 % 3600 / 60)
    }
}

fn main() -> std::io::Result<()> {
    pal::load();
    run(V1 {
        db: [0.0; 3],
        lt: 0,
        sf: 0.0,
        bg: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<String> {
        urls(&Line::from(s.to_string()))
            .into_iter()
            .map(|x| x.2)
            .collect()
    }

    #[test]
    fn finds_web_links() {
        assert_eq!(
            u("see https://example.com/a?b=1."),
            ["https://example.com/a?b=1"]
        );
        assert_eq!(
            u("[docs](https://example.com/x)"),
            ["https://example.com/x"]
        );
        assert_eq!(
            u("https://en.wikipedia.org/wiki/Rust_(language))"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(
            u("two: http://a.example/1 and <https://b.example/2>"),
            ["http://a.example/1", "https://b.example/2"]
        );
        assert!(u("no link, xhttps://nope.example and https://").is_empty());
        let l = Line::from(vec![Span::raw("  é "), Span::raw("https://example.com")]);
        assert_eq!(urls(&l)[0].0, 4);
    }

    /// Lines as the body draws assistant text: wrapped by wsegs at w - 2, two spaces in front.
    fn wrapped(s: &str, w: usize) -> Vec<Line<'static>> {
        noname::wsegs(&[(s.to_string(), Style::default())], w - 2)
            .into_iter()
            .map(|l| {
                let mut v = vec![Span::raw("  ")];
                v.extend(l);
                Line::from(v)
            })
            .collect()
    }

    #[test]
    fn wrapped_link_opens_whole() {
        let u = "https://github.com/Silvertree2010/noname/blob/main/src/main.rs?plain=1#L255";
        let bl = wrapped(&format!("see {u} for it."), 30);
        assert!(bl.len() >= 4);
        let l = links(&bl, 0, bl.len(), 30);
        assert!(l.len() >= 3, "{l:?}");
        assert!(l.iter().all(|x| x.3 == u), "{l:?}");
        // every piece sits on its own line, inside the text area
        let mut rows: Vec<usize> = l.iter().map(|x| x.0).collect();
        rows.dedup();
        assert_eq!(rows.len(), l.len());
        assert!(l.iter().all(|x| x.1 >= 2 && x.2 <= 30));
        // the window can start in the middle of the link
        let l2 = links(&bl, 2, bl.len(), 30);
        assert!(l2.iter().all(|x| x.3 != u));
    }

    #[test]
    fn link_in_wrapped_code_block_opens_whole() {
        let u = "https://example.com/a/very/long/path/that/keeps/going/until/it/wraps/index.html";
        for w in [30, 41, 60, 80] {
            let bl = noname::md(&format!("```\ncurl -L {u} -o out.html\n```"), w, &mdt());
            let l = links(&bl, 0, bl.len(), w);
            assert!(!l.is_empty(), "w={w} {l:?}");
            assert!(l.iter().all(|x| x.3 == u), "w={w} {l:?}");
            // the indented code keeps its indent on the wrapped parts too
            let bl = noname::md(&format!("```\n    x {u}\n```"), w, &mdt());
            let l = links(&bl, 0, bl.len(), w);
            assert!(!l.is_empty() && l.iter().all(|x| x.3 == u), "w={w} {l:?}");
        }
        // two code lines that only happen to fill the width are not joined
        let bl = noname::md("```\nhttps://a.example/xx\nyyyy\n```", 24, &mdt());
        let l = links(&bl, 0, bl.len(), 24);
        assert!(l.iter().all(|x| x.3 == "https://a.example/xx"), "{l:?}");
        // a blank code line (gutter only) before another code line must not overflow
        let bl = noname::md("```\nfn a() {}\n\nfn b() {}\n```", 40, &mdt());
        links(&bl, 0, bl.len(), 40);
    }

    #[test]
    fn finds_paths_that_exist() {
        let d = std::env::temp_dir().join(format!("nn-paths-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/a.rs"), "x").unwrap();
        let ds = d.to_string_lossy().to_string();
        let f = format!("{ds}/sub/a.rs");
        // a file, a file with a line, a folder, a folder in backticks with a dot after it
        assert_eq!(u(&format!("see {f}.")), std::slice::from_ref(&f));
        assert_eq!(u(&format!("at {f}:12:4,")), [format!("{f}:12:4")]);
        assert_eq!(u(&format!("({ds}/sub)")), [format!("{ds}/sub")]);
        assert_eq!(u(&format!("`{ds}/sub/`.")), [format!("{ds}/sub/")]);
        // names with spaces: the longest part that exists, also when a shorter part does
        std::fs::create_dir_all(d.join("my dir/Research - RAM 2026")).unwrap();
        let sp = format!("{ds}/my dir/Research - RAM 2026");
        assert_eq!(u(&format!("  {sp} and more")), std::slice::from_ref(&sp));
        assert_eq!(
            u(&format!("{ds}/sub and {ds}/sub")),
            [format!("{ds}/sub"), format!("{ds}/sub")]
        );
        // relative to the working folder (cargo test runs in the crate root)
        assert_eq!(
            u("edit src/main.rs and Cargo.toml"),
            ["src/main.rs", "Cargo.toml"]
        );
        // not a path: missing, plain words, mid-word, a url's own path
        assert!(u(&format!("{ds}/nope.rs and/or e.g. x{f}")).is_empty());
        assert_eq!(
            u("https://example.com/src/main.rs"),
            ["https://example.com/src/main.rs"]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn line_numbers_split_off() {
        assert_eq!(noname::sys::split_line("a/b.rs:12"), ("a/b.rs", "12"));
        assert_eq!(noname::sys::split_line("a/b.rs:12:4"), ("a/b.rs", "12"));
        assert_eq!(noname::sys::split_line("a/b.rs"), ("a/b.rs", ""));
        assert_eq!(noname::sys::split_line("C:/x"), ("C:/x", ""));
    }

    #[test]
    fn short_links_are_not_joined() {
        // the link ends at the edge but the next line is a new word at another column
        let bl = vec![Line::from("ab https://a.example/x"), Line::from("cd more")];
        assert_eq!(
            links(&bl, 0, 2, 22)
                .iter()
                .map(|x| x.3.as_str())
                .collect::<Vec<_>>(),
            ["https://a.example/x"]
        );
        // a link that does not reach the edge stays on its line
        let bl = vec![Line::from("  https://a.example/x"), Line::from("  more")];
        assert_eq!(
            links(&bl, 0, 2, 30)
                .iter()
                .map(|x| x.3.as_str())
                .collect::<Vec<_>>(),
            ["https://a.example/x"]
        );
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    fn panel(n: usize, sel: usize) -> Vec<Line<'static>> {
        let mut v = vec![Line::from("title")];
        for i in 0..n {
            v.push(Line::from(vec![
                Span::raw(if i == sel { "› " } else { "  " }),
                Span::raw(format!("row {i}")),
            ]));
        }
        v.push(Line::default());
        v
    }

    fn text(v: &[Line]) -> Vec<String> {
        v.iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn panel_that_fits_is_unchanged() {
        assert_eq!(fit_panel(panel(5, 2), 10).len(), 7);
    }

    #[test]
    fn selected_row_stays_visible() {
        for h in 1..12 {
            for sel in 0..15 {
                let o = text(&fit_panel(panel(15, sel), h));
                assert_eq!(o.len(), h);
                assert!(
                    o.iter().any(|l| l == &format!("› row {sel}")),
                    "h {h} sel {sel}: {o:?}"
                );
            }
        }
    }

    #[test]
    fn arrows_count_hidden_rows() {
        let o = text(&fit_panel(panel(15, 7), 5));
        let up: usize = o[0]
            .trim()
            .trim_start_matches("↑ ")
            .trim_end_matches(" more")
            .parse()
            .unwrap();
        let down: usize = o[4]
            .trim()
            .trim_start_matches("↓ ")
            .trim_end_matches(" more")
            .parse()
            .unwrap();
        // 17 lines in all: the 3 shown rows plus what the arrows stand for
        assert_eq!(up + 3 + down, 17);
    }
}
