use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-env-changed=WKM_RELEASE");

    let base = env!("CARGO_PKG_VERSION");

    let version = if std::env::var("WKM_RELEASE").is_ok() {
        base.to_string()
    } else {
        match git_dev_suffix() {
            Some(suffix) => format!("{base}-dev ({suffix})"),
            None => base.to_string(),
        }
    };

    println!("cargo:rustc-env=WKM_VERSION={version}");
}

fn git_dev_suffix() -> Option<String> {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;

    let dirty = Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .ok()
        .is_some_and(|s| !s.success());

    let date = Command::new("git")
        .args(["log", "-1", "--format=%cs"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;

    let hash = if dirty { format!("{hash}-dirty") } else { hash };

    Some(format!("{hash} {date}"))
}
