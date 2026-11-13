use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use hmac::{Hmac, KeyInit, Mac};
use sha1::{Digest, Sha1};
use sha2::Sha256;

use crate::config::Config;
use crate::error::Error;
use crate::httpx::Headers;

mod bitbucket;
pub(crate) mod common;
mod gitbucket;
mod gitea;
mod gitlab;
mod onedev;

pub use bitbucket::{BitbucketCloud, BitbucketServer};
pub use gitbucket::GitBucket;
pub use gitea::Gitea;
pub use gitlab::GitLab;
pub use onedev::OneDev;

pub const USER_AGENT: &str = "gitsync";
pub const ONEDEV_ROOT: &str = "/";

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_WRITE: &str = "write";
pub const ROLE_READ: &str = "read";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Repo {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub private: bool,
    pub description: String,
    pub website: String,
    pub archived: bool,
    pub default_branch: String,
    pub fork: bool,
    pub mirror: bool,
    pub empty: bool,
    pub topics: Vec<String>,
    pub updated: String,
    pub role: &'static str,
}

impl Repo {
    pub fn full(&self) -> String {
        if self.owner == ONEDEV_ROOT {
            return self.name.clone();
        }
        let mut s = String::with_capacity(self.owner.len() + self.name.len() + 1);
        s.push_str(&self.owner);
        s.push('/');
        s.push_str(&self.name);
        s
    }

    pub fn sig(&self) -> String {
        let mut topics: Vec<&str> = self.topics.iter().map(String::as_str).collect();
        topics.sort_unstable();
        let mut s = String::with_capacity(128);
        let _ = write!(
            s,
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.owner,
            self.name,
            self.private,
            self.description,
            self.website,
            self.archived,
            self.default_branch,
            topics.join(","),
            self.fork
        );
        let sum = Sha1::digest(s.as_bytes());
        hex(&sum[..8])
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub login: String,
    pub admin: bool,
    pub system_hooks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Changed = 1,
    Removed = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: Change,
    pub owner: String,
    pub name: String,
    pub id: i64,
}

pub trait Hooks: Send + Sync {
    fn global(&self, known: i64, force: bool) -> Result<(i64, String), Error>;
    fn repo(&self, r: &Repo, force: bool) -> Result<String, Error>;
    fn remove_global(&self, id: i64) -> bool;
    fn remove_repo(&self, r: &Repo) -> i64;
}

pub trait Source: Send + Sync {
    fn kind(&self) -> &'static str;
    fn whoami(&self) -> Result<Identity, Error>;
    fn list(&self, owner: &str) -> Result<Vec<Arc<Repo>>, Error>;
    fn get(&self, owner: &str, name: &str) -> Result<Option<Arc<Repo>>, Error>;
    fn owner_exists(&self, owner: &str) -> Result<bool, Error>;
    fn hooks(&self) -> Option<&dyn Hooks>;
}

pub trait Provider: Source {
    fn clone_url(&self, r: &Repo) -> String;
    fn git_header(&self) -> Result<String, Error>;
    fn parse_webhook(&self, h: &Headers, body: &[u8], secret: &str)
    -> Result<Option<Event>, Error>;
}

pub fn new(cfg: &Config) -> Result<Arc<dyn Provider>, Error> {
    Ok(match cfg.source.kind.as_str() {
        "forgejo" | "gitea" | "codeberg" | "gogs" => Arc::new(Gitea::new(cfg)),
        "gitlab" => Arc::new(GitLab::new(cfg)),
        "gitbucket" => Arc::new(GitBucket::new(cfg)),
        "onedev" => Arc::new(OneDev::new(cfg)),
        "bitbucket-cloud" => Arc::new(BitbucketCloud::new(cfg)),
        "bitbucket-server" => Arc::new(BitbucketServer::new(cfg)),
        _ => return Err(crate::err!("unsupported source type {:?}", cfg.source.kind)),
    })
}

pub fn valid_mac(secret: &str, body: &[u8], got: &str, prefix: &str) -> bool {
    let mut got = got.trim();
    if !prefix.is_empty() {
        match got.strip_prefix(prefix) {
            Some(rest) => got = rest,
            None => return false,
        }
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let want = hex(&mac.finalize().into_bytes());
    constant_eq(got.as_bytes(), want.as_bytes())
}

pub(crate) fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub type Page = Arc<[Arc<Repo>]>;

#[derive(Default)]
pub struct PageCache {
    pages: Mutex<HashMap<String, CachedPage>>,
}

struct CachedPage {
    sum: [u8; 20],
    repos: Page,
    meta: String,
}

impl PageCache {
    pub fn decode(
        &self,
        key: &str,
        body: &[u8],
        f: impl FnOnce(&[u8]) -> Result<Vec<Repo>, Error>,
    ) -> Result<Page, Error> {
        let (repos, _) = self.decode_meta(key, body, |b| Ok((f(b)?, String::new())))?;
        Ok(repos)
    }

    pub fn decode_meta(
        &self,
        key: &str,
        body: &[u8],
        f: impl FnOnce(&[u8]) -> Result<(Vec<Repo>, String), Error>,
    ) -> Result<(Page, String), Error> {
        let sum: [u8; 20] = Sha1::digest(body).into();
        {
            let pages = self.pages.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = pages.get(key)
                && cached.sum == sum
            {
                return Ok((cached.repos.clone(), cached.meta.clone()));
            }
        }
        let (rows, meta) = f(body)?;
        let repos: Page = rows.into_iter().map(Arc::new).collect();
        let mut pages = self.pages.lock().unwrap_or_else(|e| e.into_inner());
        match pages.get_mut(key) {
            Some(cached) => {
                cached.sum = sum;
                cached.repos = repos.clone();
                cached.meta.clone_from(&meta);
            }
            None => {
                pages.insert(
                    key.to_string(),
                    CachedPage {
                        sum,
                        repos: repos.clone(),
                        meta: meta.clone(),
                    },
                );
            }
        }
        Ok((repos, meta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sig_ignores_update_time_but_not_fields() {
        let a = Repo {
            id: 1,
            owner: "o".into(),
            name: "x".into(),
            updated: "2026-01-01".into(),
            ..Default::default()
        };
        let b = Repo {
            updated: "2030-01-01".into(),
            ..a.clone()
        };
        let c = Repo {
            private: true,
            updated: String::new(),
            ..a.clone()
        };
        assert_eq!(a.sig(), b.sig());
        assert_ne!(a.sig(), c.sig());
    }

    #[test]
    fn sig_matches_go() {
        let r = Repo {
            owner: "alice".into(),
            name: "site".into(),
            private: true,
            description: "d".into(),
            website: "https://w".into(),
            default_branch: "main".into(),
            topics: vec!["b".into(), "a".into()],
            ..Default::default()
        };
        let s = "alice|site|true|d|https://w|false|main|a,b|false";
        let want = hex(&Sha1::digest(s.as_bytes())[..8]);
        assert_eq!(r.sig(), want);
        assert_eq!(r.sig().len(), 16);
        assert_eq!(
            Repo::default().sig(),
            hex(&Sha1::digest(b"||false|||false|||false")[..8])
        );
    }

    #[test]
    fn full_name_of_onedev_root_projects() {
        let r = Repo {
            owner: ONEDEV_ROOT.into(),
            name: "p1".into(),
            ..Default::default()
        };
        assert_eq!(r.full(), "p1");
        let r = Repo {
            owner: "o".into(),
            name: "n".into(),
            ..Default::default()
        };
        assert_eq!(r.full(), "o/n");
    }

    #[test]
    fn pages_are_not_decoded_twice() {
        let c = PageCache::default();
        let mut n = 0;
        let mut decode = |_: &[u8]| {
            n += 1;
            Ok(vec![Repo {
                id: 1,
                ..Default::default()
            }])
        };
        let a = c.decode("k", b"[1]", &mut decode).unwrap();
        let b = c.decode("k", b"[1]", &mut decode).unwrap();
        assert!(Arc::ptr_eq(&a[0], &b[0]));
        c.decode("k", b"[2]", &mut decode).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn macs_are_checked() {
        let body = b"{}";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3cret").unwrap();
        mac.update(body);
        let sig = hex(&mac.finalize().into_bytes());
        assert!(valid_mac("s3cret", body, &sig, ""));
        assert!(valid_mac(
            "s3cret",
            body,
            &format!(" sha256={sig}\n"),
            "sha256="
        ));
        assert!(!valid_mac("s3cret", body, &sig, "sha256="));
        assert!(!valid_mac("other", body, &sig, ""));
        assert!(!valid_mac("s3cret", body, "", ""));
    }

    #[test]
    fn unknown_types_are_refused() {
        let mut cfg = Config::default();
        cfg.source.kind = "cvs".into();
        let err = new(&cfg).err().unwrap();
        assert_eq!(err.to_string(), "unsupported source type \"cvs\"");
    }
}
