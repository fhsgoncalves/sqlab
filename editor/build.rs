use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=SQLAB_COMMIT_HASH");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");

    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| ".".into()));
    let repo_dir = manifest_dir.parent().unwrap_or(&manifest_dir);
    let git_dir = repo_dir.join(".git");
    let git_head = git_dir.join("HEAD");

    if git_head.exists() {
        println!("cargo:rerun-if-changed={}", git_head.display());
        if let Ok(head) = std::fs::read_to_string(&git_head)
            && let Some(reference) = head.strip_prefix("ref: ").map(str::trim)
        {
            let ref_path = git_dir.join(reference);
            println!("cargo:rerun-if-changed={}", ref_path.display());
        }
    }

    let build_timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default()
        });
    println!("cargo:rustc-env=SQLAB_BUILD_UNIX_TIMESTAMP={build_timestamp}");

    let commit_hash = std::env::var("SQLAB_COMMIT_HASH")
        .ok()
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .or_else(|| {
            Command::new("git")
                .arg("-C")
                .arg(repo_dir)
                .arg("rev-parse")
                .arg("HEAD")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|hash| hash.trim().to_string())
                .filter(|hash| !hash.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SQLAB_COMMIT_HASH={commit_hash}");
}
