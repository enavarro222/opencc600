//! The version and the commit the firmware is built from, from git: `VERSION` the last `v*` tag, `+N` commits after
//! it (`v0.0.2`, `v0.0.2+3`), `GIT_SHA` the short commit, `-dirty` with local changes. Out of a git checkout, or
//! without the tags (a shallow clone), the crate's version and `unknown`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    // a new commit or tag, or a change staged, builds it again (an edit elsewhere only shows at the next one)
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        for f in ["HEAD", "index", "refs", "packed-refs"] {
            println!("cargo:rerun-if-changed={dir}/{f}");
        }
    }
    // `v0.0.2-3-gae18d22`: the tag, the commits since, the commit
    let version = git(&["describe", "--tags", "--long", "--match", "v*"])
        .and_then(|d| {
            let mut parts = d.rsplitn(3, '-');
            let (_, n, tag) = (parts.next()?, parts.next()?, parts.next()?);
            Some(if n == "0" { tag.to_string() } else { format!("{tag}+{n}") })
        })
        .unwrap_or_else(|| format!("v{}", std::env::var("CARGO_PKG_VERSION").unwrap()));
    let sha = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(sha) if git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty()) => {
            format!("{sha}-dirty")
        }
        Some(sha) => sha,
        None => "unknown".into(),
    };
    println!("cargo:rustc-env=VERSION={version}");
    println!("cargo:rustc-env=GIT_SHA={sha}");
}
