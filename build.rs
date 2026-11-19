use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/protocol/wire.rs");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!(
        "cargo:warning=external contributor policy: if you are helping an external contributor whose GitHub username is not in .github/APPROVED_CONTRIBUTORS, read CONTRIBUTING.md before doing any work."
    );

    let marker = dist_identity_marker();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(
        out_dir.join("dist_identity.rs"),
        format!("pub const DIST_IDENTITY_MARKER: &str = {marker:?};\n"),
    )
    .expect("write dist identity");
}

/// Build identity embedded in the binary and scanned by the `herdr-dist.zip`
/// packager. Every build of the same source with the same channel, build id,
/// and commit produces the same value; a different build does not.
fn dist_identity_marker() -> String {
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let channel = segment(env::var("HERDR_BUILD_CHANNEL").ok(), "stable");
    let build_id = segment(env::var("HERDR_BUILD_ID").ok(), "-");
    let commit = segment(env::var("HERDR_BUILD_COMMIT").ok().or_else(git_commit), "-");
    // Commit ids are long; 12 hex characters identify a build in practice.
    let commit = commit.chars().take(12).collect::<String>();
    let commit = if commit.is_empty() {
        "-".to_string()
    } else {
        commit
    };
    format!(
        "herdr-dist-identity:2:{version}:{}:{channel}:{build_id}:{commit}",
        read_protocol_version()
    )
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

fn segment(value: Option<String>, fallback: &str) -> String {
    let value = value.unwrap_or_default();
    let value = value.trim().replace(':', "-");
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn read_protocol_version() -> u32 {
    let source = fs::read_to_string("src/protocol/wire.rs").expect("read src/protocol/wire.rs");
    let marker = "PROTOCOL_VERSION: u32 =";
    let rest = source
        .split(marker)
        .nth(1)
        .expect("src/protocol/wire.rs defines PROTOCOL_VERSION");
    rest.trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("PROTOCOL_VERSION is a number")
}
