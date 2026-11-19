//! Remote install distribution archive.
//!
//! `herdr-dist.zip` carries locally built Herdr payloads for platforms this
//! client may install onto. It is discovered next to this client, validated
//! against the running build, and used instead of the published release
//! manifest whenever it exists.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::Deserialize;

pub(super) const DIST_ARCHIVE_FILE: &str = "herdr-dist.zip";
pub(super) const DIST_MANIFEST_FILE: &str = "herdr-dist.json";
pub(super) const DIST_ENV_VAR: &str = "HERDR_DIST_ARCHIVE";

const DIST_SCHEMA_VERSION: u32 = 1;
const DIST_MANIFEST_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum DistFormat {
    Exe,
    Zip,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct DistEntry {
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) format: DistFormat,
}

#[derive(Debug, Clone, Deserialize)]
struct DistManifest {
    schema_version: u32,
    version: String,
    protocol: u32,
    entries: BTreeMap<String, DistEntry>,
}

#[derive(Debug)]
pub(super) struct DistArchive {
    path: PathBuf,
    manifest: DistManifest,
}

impl DistArchive {
    pub(super) fn locate() -> io::Result<Option<PathBuf>> {
        let env = std::env::var_os(DIST_ENV_VAR).map(PathBuf::from);
        let config = crate::config::config_dir().join(DIST_ARCHIVE_FILE);
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        locate_from(env, &config, exe_dir.as_deref())
    }

    pub(super) fn open(path: PathBuf) -> io::Result<Self> {
        let manifest = read_manifest(&path)?;
        if manifest.schema_version != DIST_SCHEMA_VERSION {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} uses manifest schema {} but this client supports {DIST_SCHEMA_VERSION}",
                manifest.schema_version
            )));
        }
        let current_version = crate::build_info::version();
        if manifest.version != current_version {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} was built for herdr {} but this client is {current_version}; rebuild {DIST_ARCHIVE_FILE} from this build",
                manifest.version
            )));
        }
        if manifest.protocol != crate::protocol::PROTOCOL_VERSION {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} was built for protocol {} but this client speaks {}; rebuild {DIST_ARCHIVE_FILE} from this build",
                manifest.protocol,
                crate::protocol::PROTOCOL_VERSION
            )));
        }
        Ok(Self { path, manifest })
    }

    pub(super) fn describe(&self) -> String {
        format!("{DIST_ARCHIVE_FILE} ({})", self.path.display())
    }

    pub(super) fn entry(&self, asset_key: &str) -> io::Result<&DistEntry> {
        if let Some(entry) = self.manifest.entries.get(asset_key) {
            return Ok(entry);
        }
        let available = self
            .manifest
            .entries
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        Err(invalid_data(format!(
            "{DIST_ARCHIVE_FILE} has no entry for {asset_key} (available: {available}); build that platform or remove the archive to use the published release"
        )))
    }
}

/// Reports the archive path for `asset_key` without extracting anything.
pub(super) fn probe(asset_key: &str) -> io::Result<Option<String>> {
    match DistArchive::locate()? {
        Some(path) => {
            let archive = DistArchive::open(path)?;
            archive.entry(asset_key)?;
            Ok(Some(archive.describe()))
        }
        None => Ok(None),
    }
}

fn locate_from(
    env: Option<PathBuf>,
    config: &Path,
    exe_dir: Option<&Path>,
) -> io::Result<Option<PathBuf>> {
    if let Some(path) = env {
        if path.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{DIST_ENV_VAR} must name a {DIST_ARCHIVE_FILE} file"),
            ));
        }
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{DIST_ENV_VAR} points to {} which is not a file",
                    path.display()
                ),
            ));
        }
        return Ok(Some(path));
    }
    if config.is_file() {
        return Ok(Some(config.to_path_buf()));
    }
    if let Some(dir) = exe_dir {
        let path = dir.join(DIST_ARCHIVE_FILE);
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn read_manifest(path: &Path) -> io::Result<DistManifest> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| {
        invalid_data(format!(
            "{} is not a readable {DIST_ARCHIVE_FILE}: {err}",
            path.display()
        ))
    })?;
    let entry = archive.by_name(DIST_MANIFEST_FILE).map_err(|_| {
        invalid_data(format!(
            "{} is missing {DIST_MANIFEST_FILE}",
            path.display()
        ))
    })?;
    if entry.size() > DIST_MANIFEST_MAX_BYTES {
        return Err(invalid_data(format!(
            "{DIST_MANIFEST_FILE} is larger than {DIST_MANIFEST_MAX_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::new();
    entry
        .take(DIST_MANIFEST_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > DIST_MANIFEST_MAX_BYTES {
        return Err(invalid_data(format!(
            "{DIST_MANIFEST_FILE} is larger than {DIST_MANIFEST_MAX_BYTES} bytes"
        )));
    }
    serde_json::from_slice(&bytes)
        .map_err(|err| invalid_data(format!("{DIST_MANIFEST_FILE} is invalid: {err}")))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::Write,
        path::{Path, PathBuf},
    };

    use serde_json::json;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-dist-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn sha_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn valid_manifest(version: &str, protocol: u32) -> serde_json::Value {
        json!({
            "schema_version": 1,
            "version": version,
            "channel": "stable",
            "build_id": null,
            "protocol": protocol,
            "entries": {
                "linux-x86_64": {
                    "path": "herdr-linux-x86_64",
                    "sha256": sha_hex(b"payload"),
                    "format": "exe",
                },
                "windows-x86_64": {
                    "path": "herdr-windows-x86_64.zip",
                    "sha256": sha_hex(b"windows"),
                    "format": "zip",
                },
            },
        })
    }

    fn write_archive(
        dir: &Path,
        manifest: serde_json::Value,
        payloads: &[(&str, &[u8])],
    ) -> PathBuf {
        let path = dir.join(DIST_ARCHIVE_FILE);
        let file = File::create(&path).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(DIST_MANIFEST_FILE, zip::write::SimpleFileOptions::default())
            .expect("manifest entry");
        writer
            .write_all(serde_json::to_string(&manifest).expect("serialize").as_bytes())
            .expect("write manifest");
        for (name, bytes) in payloads {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("payload entry");
            writer.write_all(bytes).expect("write payload");
        }
        writer.finish().expect("finish archive");
        path
    }

    #[test]
    fn manifest_round_trips() {
        let dir = temp_dir("round-trip");
        let path = write_archive(
            &dir,
            valid_manifest(&crate::build_info::version(), crate::protocol::PROTOCOL_VERSION),
            &[("herdr-linux-x86_64", b"payload"), ("herdr-windows-x86_64.zip", b"windows")],
        );

        let archive = DistArchive::open(path).expect("open archive");
        let entry = archive.entry("linux-x86_64").expect("linux entry");
        assert_eq!(entry.format, DistFormat::Exe);
        assert_eq!(entry.path, "herdr-linux-x86_64");
        assert_eq!(entry.sha256, sha_hex(b"payload"));
        assert_eq!(archive.entry("windows-x86_64").expect("windows entry").format, DistFormat::Zip);
    }

    #[test]
    fn rejects_stale_version() {
        let dir = temp_dir("stale-version");
        let path = write_archive(
            &dir,
            valid_manifest("0.0.1", crate::protocol::PROTOCOL_VERSION),
            &[("herdr-linux-x86_64", b"payload")],
        );

        let err = DistArchive::open(path).expect_err("stale version must fail");
        let message = err.to_string();
        assert!(message.contains("0.0.1"), "{message}");
        assert!(message.contains(&crate::build_info::version()), "{message}");
        assert!(message.contains("rebuild"), "{message}");
    }

    #[test]
    fn rejects_wrong_protocol() {
        let dir = temp_dir("wrong-protocol");
        let protocol = crate::protocol::PROTOCOL_VERSION + 1;
        let path = write_archive(
            &dir,
            valid_manifest(&crate::build_info::version(), protocol),
            &[("herdr-linux-x86_64", b"payload")],
        );

        let err = DistArchive::open(path).expect_err("wrong protocol must fail");
        let message = err.to_string();
        assert!(message.contains(&protocol.to_string()), "{message}");
        assert!(
            message.contains(&crate::protocol::PROTOCOL_VERSION.to_string()),
            "{message}"
        );
    }

    #[test]
    fn rejects_unknown_schema() {
        let dir = temp_dir("unknown-schema");
        let mut manifest = valid_manifest(&crate::build_info::version(), crate::protocol::PROTOCOL_VERSION);
        manifest["schema_version"] = json!(2);
        let path = write_archive(&dir, manifest, &[("herdr-linux-x86_64", b"payload")]);

        let err = DistArchive::open(path).expect_err("unknown schema must fail");
        assert!(err.to_string().contains("schema"), "{err}");
    }

    #[test]
    fn reports_missing_platform() {
        let dir = temp_dir("missing-platform");
        let mut manifest = valid_manifest(&crate::build_info::version(), crate::protocol::PROTOCOL_VERSION);
        manifest["entries"] = json!({
            "linux-x86_64": {
                "path": "herdr-linux-x86_64",
                "sha256": sha_hex(b"payload"),
                "format": "exe",
            },
        });
        let path = write_archive(&dir, manifest, &[("herdr-linux-x86_64", b"payload")]);

        let archive = DistArchive::open(path).expect("open archive");
        let err = archive
            .entry("windows-x86_64")
            .expect_err("missing platform must fail");
        let message = err.to_string();
        assert!(message.contains("windows-x86_64"), "{message}");
        assert!(message.contains("linux-x86_64"), "{message}");
    }

    #[test]
    fn missing_manifest_is_invalid() {
        let dir = temp_dir("missing-manifest");
        let path = dir.join(DIST_ARCHIVE_FILE);
        let file = File::create(&path).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("herdr-linux-x86_64", zip::write::SimpleFileOptions::default())
            .expect("payload entry");
        writer.write_all(b"payload").expect("write payload");
        writer.finish().expect("finish archive");

        let err = DistArchive::open(path).expect_err("missing manifest must fail");
        assert!(err.to_string().contains(DIST_MANIFEST_FILE), "{err}");
    }

    #[test]
    fn explicit_env_path_must_exist() {
        let dir = temp_dir("env-missing");
        let missing = dir.join("nope.zip");
        let config = dir.join("config-herdr-dist.zip");
        let err = locate_from(Some(missing), &config, None)
            .expect_err("explicit env path must exist");
        assert!(err.to_string().contains("HERDR_DIST_ARCHIVE"), "{err}");
    }

    #[test]
    fn config_dir_precedes_exe_dir() {
        let dir = temp_dir("precedence");
        let config = dir.join(DIST_ARCHIVE_FILE);
        let exe_dir = dir.join("bin");
        fs::create_dir_all(&exe_dir).expect("create exe dir");
        let exe_archive = exe_dir.join(DIST_ARCHIVE_FILE);
        fs::write(&config, b"config").expect("write config archive");
        fs::write(&exe_archive, b"exe").expect("write exe archive");

        let located = locate_from(None, &config, Some(&exe_dir)).expect("locate");
        assert_eq!(located.as_deref(), Some(config.as_path()));
    }

    #[test]
    fn falls_back_to_exe_dir_then_none() {
        let dir = temp_dir("exe-fallback");
        let config = dir.join("config-herdr-dist.zip");
        let exe_dir = dir.join("bin");
        fs::create_dir_all(&exe_dir).expect("create exe dir");
        let exe_archive = exe_dir.join(DIST_ARCHIVE_FILE);
        fs::write(&exe_archive, b"exe").expect("write exe archive");

        let located = locate_from(None, &config, Some(&exe_dir)).expect("locate");
        assert_eq!(located.as_deref(), Some(exe_archive.as_path()));

        let absent = locate_from(None, &config, None).expect("locate");
        assert!(absent.is_none());
    }
}