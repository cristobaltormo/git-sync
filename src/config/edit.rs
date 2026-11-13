use std::collections::BTreeMap;
use std::io::Write as _;

use super::owner::{self, TempFile};
use super::{Config, parent_dir};
use crate::config_err;
use crate::error::{Error, io_message};

fn section_header(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with("[[") {
        return None;
    }
    let rest = t.strip_prefix('[')?;
    let end = rest.find(']')?;
    let name = &rest[..end];
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return None;
    }
    let tail = rest[end + 1..].trim_start();
    if tail.is_empty() || tail.starts_with('#') {
        Some(name)
    } else {
        None
    }
}

fn is_key_line(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

pub(super) fn set_literal(
    mut lines: Vec<String>,
    section: &str,
    key: &str,
    literal: &str,
) -> Vec<String> {
    let mut start = None;
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate() {
        if let Some(name) = section_header(l) {
            if start.is_some() {
                end = i;
                break;
            }
            if name == section {
                start = Some(i);
            }
        }
    }
    let entry = format!("{key} = {literal}");
    let Some(start) = start else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.push(entry);
        return lines;
    };
    for i in start + 1..end {
        if !is_key_line(&lines[i], key) {
            continue;
        }
        let mut stop = i + 1;
        let line = &lines[i];
        let value = line[line.find('=').map_or(line.len(), |p| p + 1)..].trim();
        if value.starts_with('[') && !value.contains(']') {
            while stop < end && !lines[stop].contains(']') {
                stop += 1;
            }
            stop += 1;
        }
        let stop = stop.min(lines.len());
        lines.splice(i..stop, [entry]);
        return lines;
    }
    let mut at = end;
    while at > start + 1 && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    lines.insert(at, entry);
    lines
}

pub fn set_values(path: &str, kv: &BTreeMap<String, String>) -> Result<(), Error> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| config_err!("cannot read {path}: {}", io_message(&e)))?;
    let crlf = raw.contains("\r\n");
    let text = raw.replace("\r\n", "\n");
    let mut lines: Vec<String> = text
        .trim_end_matches('\n')
        .split('\n')
        .map(String::from)
        .collect();
    for (name, literal) in kv {
        let (sec, key) = name.split_once('.').unwrap_or((name.as_str(), ""));
        lines = set_literal(lines, sec, key, literal);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    if crlf {
        out = out.replace('\n', "\r\n");
    }
    let st = std::fs::metadata(path)
        .map_err(|e| config_err!("cannot read {path}: {}", io_message(&e)))?;
    let dir = parent_dir(path);
    let (tmp, mut file) = TempFile::create(&dir, ".config-", ".toml")
        .map_err(|e| config_err!("cannot write in {dir}: {}", io_message(&e)))?;
    file.write_all(out.as_bytes())?;
    drop(file);
    let tmp_name = tmp.path().to_string_lossy().into_owned();
    if let Err(e) = Config::load(&tmp_name) {
        let msg = e.to_string();
        let prefix = format!("{tmp_name}: ");
        let msg = msg.strip_prefix(&prefix).unwrap_or(&msg);
        return Err(config_err!(
            "that would make the config invalid, nothing was changed: {msg}"
        ));
    }
    owner::set_mode(tmp.path(), owner::mode(&st))?;
    owner::keep_owner(tmp.path(), &st);
    tmp.persist(path)?;
    Ok(())
}
