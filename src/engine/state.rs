use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{lock, unix_now};
use crate::config_err;
use crate::error::{Error, io_message};
use crate::github::Snap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub owner: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gh_owner: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gh_name: String,
    pub managed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sig: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gh: Option<Snap>,
    #[serde(skip_serializing_if = "is_zero")]
    pub last_ok: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub last_push: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refs: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_error: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub attempts: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub retry_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub missing_since: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub excluded: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hook: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refused_branch: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pending: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub version: i64,
    pub system_hook: i64,
    pub pulls: BTreeMap<String, i64>,
    pub pulls_ready: bool,
    pub repos: HashMap<i64, Entry>,
}

impl Default for State {
    fn default() -> State {
        State {
            version: 1,
            system_hook: 0,
            pulls: BTreeMap::new(),
            pulls_ready: false,
            repos: HashMap::new(),
        }
    }
}

struct SortedRepos<'a>(&'a HashMap<i64, Entry>);

impl Serialize for SortedRepos<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut rows: Vec<(String, &Entry)> =
            self.0.iter().map(|(id, x)| (id.to_string(), x)).collect();
        rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut m = s.serialize_map(Some(rows.len()))?;
        for (k, v) in &rows {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

#[derive(Serialize)]
struct StateOut<'a> {
    version: i64,
    #[serde(skip_serializing_if = "is_zero")]
    system_hook: i64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pulls: &'a BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "is_false")]
    pulls_ready: bool,
    repos: SortedRepos<'a>,
}

#[derive(Deserialize)]
struct StateIn {
    #[serde(default)]
    version: i64,
    #[serde(default)]
    system_hook: i64,
    #[serde(default)]
    pulls: Option<BTreeMap<String, i64>>,
    #[serde(default)]
    pulls_ready: bool,
    #[serde(default)]
    repos: Option<BTreeMap<String, Entry>>,
}

impl Serialize for State {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        StateOut {
            version: self.version,
            system_hook: self.system_hook,
            pulls: &self.pulls,
            pulls_ready: self.pulls_ready,
            repos: SortedRepos(&self.repos),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<State, D::Error> {
        let raw = StateIn::deserialize(d)?;
        let repos = raw
            .repos
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| k.parse::<i64>().ok().map(|id| (id, v)))
            .collect();
        Ok(State {
            version: raw.version,
            system_hook: raw.system_hook,
            pulls: raw.pulls.unwrap_or_default(),
            pulls_ready: raw.pulls_ready,
            repos,
        })
    }
}

impl State {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::with_capacity(256 + self.repos.len() * 512);
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        self.serialize(&mut ser)?;
        Ok(go_escape(buf))
    }
}

fn go_escape(buf: Vec<u8>) -> Vec<u8> {
    let special = |w: &[u8]| matches!(w, [0xe2, 0x80, 0xa8] | [0xe2, 0x80, 0xa9]);
    let needs = buf.iter().any(|b| matches!(b, b'<' | b'>' | b'&')) || buf.windows(3).any(special);
    if !needs {
        return buf;
    }
    let mut out = Vec::with_capacity(buf.len() + 64);
    let mut i = 0;
    while i < buf.len() {
        match buf[i] {
            b'<' => out.extend_from_slice(b"\\u003c"),
            b'>' => out.extend_from_slice(b"\\u003e"),
            b'&' => out.extend_from_slice(b"\\u0026"),
            0xe2 if i + 2 < buf.len() && special(&buf[i..i + 3]) => {
                out.extend_from_slice(if buf[i + 2] == 0xa8 {
                    b"\\u2028"
                } else {
                    b"\\u2029"
                });
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    out
}

fn state_file(dir: &str) -> PathBuf {
    Path::new(dir).join("state.json")
}

fn read_file(file: &Path) -> Result<State, ReadError> {
    let b = std::fs::read(file).map_err(ReadError::Io)?;
    serde_json::from_slice(&b).map_err(|e| ReadError::Decode(e.to_string()))
}

enum ReadError {
    Io(io::Error),
    Decode(String),
}

impl ReadError {
    fn message(&self, file: &Path) -> String {
        match self {
            ReadError::Io(e) => format!("open {}: {}", file.display(), io_message(e)),
            ReadError::Decode(m) => m.clone(),
        }
    }
}

pub fn read_state(dir: &str) -> Result<State, Error> {
    let file = state_file(dir);
    read_file(&file).map_err(|e| Error::Other(e.message(&file)))
}

pub(super) struct Store {
    data: Mutex<State>,
    io: Mutex<Option<u64>>,
    file: PathBuf,
    dry: bool,
}

impl Store {
    pub(super) fn load(dir: &str, dry: bool) -> Result<Store, Error> {
        let file = state_file(dir);
        let data = match read_file(&file) {
            Ok(st) => st,
            Err(ReadError::Io(e)) if e.kind() == io::ErrorKind::NotFound => State::default(),
            Err(e) => {
                return Err(config_err!(
                    "cannot read {}: {}",
                    file.display(),
                    e.message(&file)
                ));
            }
        };
        Ok(Store {
            data: Mutex::new(data),
            io: Mutex::new(None),
            file,
            dry,
        })
    }

    pub(super) fn with<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&lock(&self.data))
    }

    pub(super) fn save(&self) {
        if self.dry {
            return;
        }
        let mut written = lock(&self.io);
        let blob = match self.with(State::encode) {
            Ok(b) => b,
            Err(e) => {
                errorf!("cannot encode state: {e}");
                return;
            }
        };
        let sum = {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut h = DefaultHasher::new();
            blob.hash(&mut h);
            h.finish()
        };
        if *written == Some(sum) {
            return;
        }
        if let Some(dir) = self.file.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            errorf!("cannot create state dir: {}", io_message(&e));
            return;
        }
        let mut tmp = self.file.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if let Err(e) = write_private(&tmp, &blob) {
            errorf!("cannot write state: {}", io_message(&e));
            return;
        }
        crate::config::like_dir(&tmp, self.file.parent().unwrap_or(Path::new("")));
        if let Err(e) = std::fs::rename(&tmp, &self.file) {
            errorf!("cannot write state: {}", io_message(&e));
            return;
        }
        sync_dir(self.file.parent());
        *written = Some(sum);
    }

    pub(super) fn get(&self, id: i64) -> Option<Entry> {
        self.with(|st| st.repos.get(&id).cloned())
    }

    pub(super) fn update(&self, id: i64, create: bool, f: impl FnOnce(&mut Entry)) {
        let mut st = lock(&self.data);
        match st.repos.get_mut(&id) {
            Some(x) => f(x),
            None if create => f(st.repos.entry(id).or_default()),
            None => {}
        }
    }

    pub(super) fn drop_entry(&self, id: i64) {
        lock(&self.data).repos.remove(&id);
    }

    pub(super) fn clear_retries(&self) {
        let mut st = lock(&self.data);
        for x in st.repos.values_mut() {
            if !x.last_error.is_empty() {
                x.retry_at = 0;
            }
        }
    }

    pub(super) fn set_system_hook(&self, id: i64) {
        lock(&self.data).system_hook = id;
        self.save();
    }

    pub(super) fn mark_pull(&self, key: &str) {
        let mut st = lock(&self.data);
        if !st.pulls.contains_key(key) {
            st.pulls.insert(key.to_string(), unix_now());
        }
    }

    pub(super) fn finish_pulls(&self, open: &std::collections::HashSet<String>, complete: bool) {
        let mut st = lock(&self.data);
        if complete {
            st.pulls.retain(|k, _| open.contains(k));
        }
        st.pulls_ready = st.pulls_ready || complete;
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(unix)]
fn sync_dir(dir: Option<&Path>) {
    let dir = match dir {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: Option<&Path>) {}
