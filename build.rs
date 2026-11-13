use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_string())
}

fn own_repo() -> bool {
    let Some(top) = git(&["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let here = canon(Path::new(env!("CARGO_MANIFEST_DIR")));
    here.is_some() && canon(Path::new(&top)) == here
}

fn watch_git() {
    let Some(dir) = git(&["rev-parse", "--git-dir"]) else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    for name in ["HEAD", "index", "packed-refs", "refs/tags", "refs/heads"] {
        let p = dir.join(name);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn main() {
    println!("cargo:rerun-if-env-changed=GITSYNC_VERSION");
    let version = match std::env::var("GITSYNC_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => own_repo()
            .then(|| {
                watch_git();
                git(&["describe", "--tags", "--always", "--dirty"])
            })
            .flatten()
            .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default()),
    };
    println!("cargo:rustc-env=GITSYNC_BUILD_VERSION={version}");
}
