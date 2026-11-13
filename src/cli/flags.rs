use crate::error::Error;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    Str,
}

struct Def {
    name: &'static str,
    usage: &'static str,
    kind: Kind,
    default: String,
    slot: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct BoolFlag(usize);

#[derive(Clone, Copy)]
pub(crate) struct StrFlag(usize);

pub(crate) struct FlagSet {
    name: &'static str,
    defs: Vec<Def>,
    values: Vec<String>,
    args: Vec<String>,
}

pub(crate) enum Failure {
    Help,
    Bad(String),
}

impl FlagSet {
    pub(crate) fn new(name: &'static str) -> FlagSet {
        FlagSet {
            name,
            defs: Vec::new(),
            values: Vec::new(),
            args: Vec::new(),
        }
    }

    pub(crate) fn string(
        &mut self,
        names: &[&'static str],
        default: &str,
        usage: &'static str,
    ) -> StrFlag {
        let slot = self.values.len();
        self.values.push(default.to_string());
        for name in names {
            self.defs.push(Def {
                name,
                usage,
                kind: Kind::Str,
                default: default.to_string(),
                slot,
            });
        }
        StrFlag(slot)
    }

    pub(crate) fn bool(&mut self, name: &'static str, usage: &'static str) -> BoolFlag {
        let slot = self.values.len();
        self.values.push("false".into());
        self.defs.push(Def {
            name,
            usage,
            kind: Kind::Bool,
            default: "false".into(),
            slot,
        });
        BoolFlag(slot)
    }

    pub(crate) fn get(&self, f: StrFlag) -> &str {
        &self.values[f.0]
    }

    pub(crate) fn on(&self, f: BoolFlag) -> bool {
        self.values[f.0] == "true"
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn arg(&self, i: usize) -> &str {
        self.args.get(i).map_or("", String::as_str)
    }

    pub(crate) fn nargs(&self) -> usize {
        self.args.len()
    }

    pub(crate) fn usage(&self) -> String {
        let mut defs: Vec<&Def> = self.defs.iter().collect();
        defs.sort_by_key(|d| d.name);
        let mut out = format!("Usage of {}:\n", self.name);
        for d in defs {
            let mut s = format!("  -{}", d.name);
            if d.kind == Kind::Str {
                s.push_str(" string");
            }
            if s.len() <= 4 {
                s.push('\t');
            } else {
                s.push_str("\n    \t");
            }
            s.push_str(&d.usage.replace('\n', "\n    \t"));
            if d.kind == Kind::Str && !d.default.is_empty() {
                s.push_str(&format!(" (default {:?})", d.default));
            }
            out.push_str(&s);
            out.push('\n');
        }
        out
    }

    pub(crate) fn try_parse(&mut self, args: &[String]) -> Result<(), Failure> {
        let mut i = 0;
        while i < args.len() {
            let s = args[i].as_str();
            if s.len() < 2 || !s.starts_with('-') {
                break;
            }
            let mut minuses = 1;
            if s.as_bytes()[1] == b'-' {
                minuses = 2;
                if s.len() == 2 {
                    i += 1;
                    break;
                }
            }
            let name = &s[minuses..];
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return Err(Failure::Bad(format!("bad flag syntax: {s}")));
            }
            i += 1;
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (name, None),
            };
            let Some(def) = self.defs.iter().find(|d| d.name == name) else {
                if name == "help" || name == "h" {
                    return Err(Failure::Help);
                }
                return Err(Failure::Bad(format!(
                    "flag provided but not defined: -{name}"
                )));
            };
            let slot = def.slot;
            match def.kind {
                Kind::Bool => {
                    let v = match value {
                        None => true,
                        Some(v) => parse_bool(v).ok_or_else(|| {
                            Failure::Bad(format!(
                                "invalid boolean value {v:?} for -{name}: parse error"
                            ))
                        })?,
                    };
                    self.values[slot] = v.to_string();
                }
                Kind::Str => {
                    let v = match value {
                        Some(v) => v.to_string(),
                        None if i < args.len() => {
                            i += 1;
                            args[i - 1].clone()
                        }
                        None => {
                            return Err(Failure::Bad(format!("flag needs an argument: -{name}")));
                        }
                    };
                    self.values[slot] = v;
                }
            }
        }
        self.args = args[i..].to_vec();
        Ok(())
    }

    pub(crate) fn parse(&mut self, args: &[String]) -> Result<(), Error> {
        match self.try_parse(args) {
            Ok(()) => Ok(()),
            Err(Failure::Help) => {
                eprint!("{}", self.usage());
                Err(Error::Config("flag: help requested".into()))
            }
            Err(Failure::Bad(msg)) => {
                eprintln!("{msg}");
                eprint!("{}", self.usage());
                Err(Error::Config(msg))
            }
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_like_go_flag() {
        let mut fs = FlagSet::new("run");
        let c = fs.string(&["c", "config"], "", "config file");
        let dry = fs.bool("dry-run", "log what would happen, change nothing");
        fs.try_parse(&strings(&["--config", "/x.toml", "-dry-run", "a", "-b"]))
            .ok()
            .unwrap();
        assert_eq!(fs.get(c), "/x.toml");
        assert!(fs.on(dry));
        assert_eq!(fs.args(), strings(&["a", "-b"]));

        let mut fs = FlagSet::new("run");
        let c = fs.string(&["c", "config"], "keep", "config file");
        let dry = fs.bool("dry-run", "x");
        fs.try_parse(&strings(&["-c=y", "--dry-run=false", "--", "-z"]))
            .ok()
            .unwrap();
        assert_eq!(fs.get(c), "y");
        assert!(!fs.on(dry));
        assert_eq!(fs.args(), strings(&["-z"]));
    }

    #[test]
    fn reports_errors_like_go_flag() {
        let msg = |a: &[&str]| {
            let mut fs = FlagSet::new("x");
            fs.string(&["c"], "", "config file");
            fs.bool("force", "f");
            match fs.try_parse(&strings(a)) {
                Err(Failure::Bad(m)) => m,
                Err(Failure::Help) => "help".into(),
                Ok(()) => "ok".into(),
            }
        };
        assert_eq!(msg(&["-nope"]), "flag provided but not defined: -nope");
        assert_eq!(msg(&["-c"]), "flag needs an argument: -c");
        assert_eq!(msg(&["---c"]), "bad flag syntax: ---c");
        assert_eq!(
            msg(&["-force=maybe"]),
            "invalid boolean value \"maybe\" for -force: parse error"
        );
        assert_eq!(msg(&["-h"]), "help");
        assert_eq!(msg(&["-", "-nope"]), "ok");
    }

    #[test]
    fn usage_matches_go_layout() {
        let mut fs = FlagSet::new("run");
        fs.string(&["c", "config"], "/etc/x.toml", "config file");
        fs.bool("dry-run", "log what would happen, change nothing");
        assert_eq!(
            fs.usage(),
            "Usage of run:\n  -c string\n    \tconfig file (default \"/etc/x.toml\")\n  -config string\n    \tconfig file (default \"/etc/x.toml\")\n  -dry-run\n    \tlog what would happen, change nothing\n"
        );
    }
}
