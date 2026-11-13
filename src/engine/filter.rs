use crate::config::{Config, RepoRule};
use crate::forge::{ROLE_ADMIN, Repo};

use super::Engine;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    pub sync: bool,
    pub pending: bool,
    pub why: String,
    pub rule: Option<RepoRule>,
}

impl Engine {
    pub fn selected(&self, r: &Repo) -> (bool, String) {
        let v = self.decide(r);
        (v.sync, v.why)
    }

    pub fn decide(&self, r: &Repo) -> Verdict {
        self.decide_in(&self.config(), r)
    }

    pub(super) fn decide_in(&self, cfg: &Config, r: &Repo) -> Verdict {
        let rule = cfg.repos.rule(&r.full());
        let verdict = |sync: bool, pending: bool, why: String| Verdict {
            sync,
            pending,
            why,
            rule: rule.cloned(),
        };
        if let Some(allow) = rule.and_then(|x| x.sync) {
            return if allow {
                verdict(true, false, "allowed by you".into())
            } else {
                verdict(false, false, "ignored by you".into())
            };
        }
        let f = &cfg.filter;
        let no = |why: &str| verdict(false, false, why.to_string());
        if f.skip_forks && r.fork {
            return no("fork");
        }
        if f.skip_mirrors && r.mirror {
            return no("pull mirror");
        }
        if f.skip_archived && r.archived {
            return no("archived");
        }
        if f.skip_private && r.private {
            return no("private");
        }
        if !f.topic.is_empty() && !r.topics.contains(&f.topic) {
            return no(&format!("no '{}' topic", f.topic));
        }
        if f.scope == "admin" && !r.role.is_empty() && r.role != ROLE_ADMIN {
            return no("you are not an admin of it");
        }
        if !f.include.is_empty() && !matches(&f.include, r) {
            return no("not in filter.include");
        }
        if matches(&f.exclude, r) {
            return no("in filter.exclude");
        }
        if f.new_repos != "sync" {
            let managed = self
                .state
                .with(|st| st.repos.get(&r.id).is_some_and(|x| x.managed));
            if !managed {
                if f.new_repos == "review" {
                    return verdict(false, true, "waiting for your decision".into());
                }
                return no("not allowed yet");
            }
        }
        verdict(true, false, String::new())
    }
}

pub(super) fn unwanted(cfg: &Config, owner: &str, name: &str) -> Option<&'static str> {
    let full = super::full_name(owner, name);
    if let Some(r) = cfg.repos.rule(&full)
        && let Some(allow) = r.sync
    {
        return (!allow).then_some("ignored by you");
    }
    let f = &cfg.filter;
    if !f.include.is_empty() && !matches_name(&f.include, &full, name) {
        return Some("not in filter.include");
    }
    if matches_name(&f.exclude, &full, name) {
        return Some("in filter.exclude");
    }
    None
}

fn matches(patterns: &[String], r: &Repo) -> bool {
    matches_name(patterns, &r.full(), &r.name)
}

fn matches_name(patterns: &[String], full: &str, name: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let full = full.to_lowercase();
    let name = name.to_lowercase();
    patterns.iter().any(|p| {
        let p = p.to_lowercase();
        glob_match(&p, &full) || (!p.contains('/') && glob_match(&p, &name))
    })
}

pub fn glob_match(pattern: &str, name: &str) -> bool {
    matches_go(pattern.as_bytes(), name.as_bytes()).unwrap_or(false)
}

struct BadPattern;

fn matches_go(mut pattern: &[u8], mut name: &[u8]) -> Result<bool, BadPattern> {
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            return Ok(!name.contains(&b'/'));
        }
        let (t, ok, err) = match_chunk(chunk, name);
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if err {
            return Err(BadPattern);
        }
        if star {
            let mut i = 0;
            while i < name.len() && name[i] != b'/' {
                let (t, ok, err) = match_chunk(chunk, &name[i + 1..]);
                if ok {
                    if pattern.is_empty() && !t.is_empty() {
                        i += 1;
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
                if err {
                    return Err(BadPattern);
                }
                i += 1;
            }
        }
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            if match_chunk(chunk, b"").2 {
                return Err(BadPattern);
            }
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while let [b'*', rest @ ..] = pattern {
        pattern = rest;
        star = true;
    }
    let mut inrange = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => {
                if i + 1 < pattern.len() {
                    i += 1;
                }
            }
            b'[' => inrange = true,
            b']' => inrange = false,
            b'*' if !inrange => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

fn decode(s: &[u8]) -> (Option<char>, usize) {
    let n = match s.first() {
        None => return (None, 0),
        Some(b) if *b < 0x80 => 1,
        Some(b) if *b >= 0xf0 => 4,
        Some(b) if *b >= 0xe0 => 3,
        Some(b) if *b >= 0xc0 => 2,
        Some(_) => 1,
    };
    let n = n.min(s.len());
    match std::str::from_utf8(&s[..n]) {
        Ok(t) => (t.chars().next(), n),
        Err(_) => (None, 1),
    }
}

fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> (&'a [u8], bool, bool) {
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                let mut r = None;
                if !failed {
                    let (c, n) = decode(s);
                    r = c;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                let mut negated = false;
                if chunk.first() == Some(&b'^') {
                    negated = true;
                    chunk = &chunk[1..];
                }
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if chunk.first() == Some(&b']') && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let Some((lo, rest)) = get_esc(chunk) else {
                        return (b"", false, true);
                    };
                    chunk = rest;
                    let mut hi = lo;
                    if chunk[0] == b'-' {
                        let Some((h, rest)) = get_esc(&chunk[1..]) else {
                            return (b"", false, true);
                        };
                        hi = h;
                        chunk = rest;
                    }
                    if r.is_some_and(|r| lo <= r && r <= hi) {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s[0] == b'/' {
                        failed = true;
                    }
                    let (_, n) = decode(s);
                    s = &s[n..];
                }
                chunk = &chunk[1..];
            }
            c => {
                let c = if c == b'\\' {
                    chunk = &chunk[1..];
                    match chunk.first() {
                        Some(c) => *c,
                        None => return (b"", false, true),
                    }
                } else {
                    c
                };
                if !failed {
                    if c != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        return (b"", false, false);
    }
    (s, true, false)
}

fn get_esc(mut chunk: &[u8]) -> Option<(char, &[u8])> {
    match chunk.first() {
        None | Some(b'-') | Some(b']') => return None,
        Some(b'\\') => {
            chunk = &chunk[1..];
            if chunk.is_empty() {
                return None;
            }
        }
        _ => {}
    }
    let (c, n) = decode(chunk);
    let c = c?;
    let rest = &chunk[n..];
    if rest.is_empty() {
        return None;
    }
    Some((c, rest))
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs_behave_like_go_path_match() {
        let cases: &[(&str, &str, bool)] = &[
            ("abc", "abc", true),
            ("*", "abc", true),
            ("*c", "abc", true),
            ("a*", "a", true),
            ("a*", "abc", true),
            ("a*", "ab/c", false),
            ("a*/b", "abc/b", true),
            ("a*/b", "a/c/b", false),
            ("a*b*c*d*e*/f", "axbxcxdxe/f", true),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/f", true),
            ("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", false),
            ("a*b?c*x", "abxbbxdbxebxczzx", true),
            ("a*b?c*x", "abxbbxdbxebxczzy", false),
            ("ab[c]", "abc", true),
            ("ab[b-d]", "abc", true),
            ("ab[e-g]", "abc", false),
            ("ab[^c]", "abc", false),
            ("ab[^b-d]", "abc", false),
            ("ab[^e-g]", "abc", true),
            ("a\\*b", "a*b", true),
            ("a\\*b", "ab", false),
            ("a?b", "a世b", true),
            ("a[^a]b", "a世b", true),
            ("a???b", "a世b", false),
            ("[a-ζ]*", "α", true),
            ("*[a-ζ]", "A", false),
            ("a?b", "a/b", false),
            ("a*b", "a/b", false),
            ("[\\]a]", "]", true),
            ("[\\-]", "-", true),
            ("[x\\-]", "x", true),
            ("[x\\-]", "-", true),
            ("[x\\-]", "z", false),
            ("[\\-x]", "x", true),
            ("[]a]", "]", false),
            ("[-]", "-", false),
            ("[x-]", "x", false),
            ("[a-b-c]", "a", false),
            ("[", "a", false),
            ("[^", "a", false),
            ("[^bc", "a", false),
            ("a[", "a", false),
            ("a[", "ab", false),
            ("a[", "x", false),
            ("a/b[", "x", false),
            ("*x", "xxx", true),
            ("si*", "site", true),
            ("alice/*", "alice/site", true),
        ];
        for (p, s, want) in cases {
            assert_eq!(glob_match(p, s), *want, "{p} {s}");
        }
    }
}
