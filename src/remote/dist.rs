//! Remote install distribution archive.
//!
//! `herdr-dist.zip` carries locally built Herdr payloads for platforms this
//! client may install onto. It is discovered next to this client, validated
//! against the running build, and used instead of the published release
//! manifest whenever it exists.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

pub(super) const DIST_ARCHIVE_FILE: &str = "herdr-dist.zip";
pub(super) const DIST_MANIFEST_FILE: &str = "herdr-dist.json";
pub(super) const DIST_ENV_VAR: &str = "HERDR_DIST_ARCHIVE";

const DIST_SCHEMA_VERSION: u32 = 1;
const DIST_MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const MAX_DIST_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

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
    build_identity: String,
    version: String,
    protocol: u32,
    entries: BTreeMap<String, DistEntry>,
}

#[derive(Debug)]
pub(super) struct DistArchive {
    path: PathBuf,
    manifest: DistManifest,
}

#[derive(Debug)]
pub(super) struct DistInstall {
    pub(super) path: PathBuf,
    pub(super) temporary_dir: PathBuf,
    pub(super) format: DistFormat,
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
        tracing::debug!(
            identity = crate::build_info::DIST_IDENTITY_MARKER,
            archive = %path.display(),
            "inspecting remote distribution archive"
        );
        let manifest = read_manifest(&path)?;
        if manifest.schema_version != DIST_SCHEMA_VERSION {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} uses manifest schema {} but this client supports {DIST_SCHEMA_VERSION}",
                manifest.schema_version
            )));
        }
        if manifest.build_identity != crate::build_info::DIST_IDENTITY_MARKER {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} was packaged from build {} but this client is {}; rebuild {DIST_ARCHIVE_FILE} from this build",
                manifest.build_identity,
                crate::build_info::DIST_IDENTITY_MARKER
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

    pub(super) fn install_source(&self, asset_key: &str) -> io::Result<DistInstall> {
        let dir = super::attach::private_download_dir(&format!("dist-{asset_key}"))?;
        self.extract_entry(asset_key, dir)
    }

    fn extract_entry(&self, asset_key: &str, dir: PathBuf) -> io::Result<DistInstall> {
        let result = self.extract_entry_inner(asset_key, &dir);
        match result {
            Ok((path, format)) => Ok(DistInstall {
                path,
                temporary_dir: dir,
                format,
            }),
            Err(err) => {
                let _ = fs::remove_dir_all(&dir);
                Err(err)
            }
        }
    }

    fn extract_entry_inner(
        &self,
        asset_key: &str,
        dir: &Path,
    ) -> io::Result<(PathBuf, DistFormat)> {
        let entry = self.entry(asset_key)?;
        let payload = payload_file_name(&entry.path)?;
        let sha256 = entry.sha256.clone();
        let format = entry.format;

        let file = File::open(&self.path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(|err| {
            invalid_data(format!(
                "{} is not a readable {DIST_ARCHIVE_FILE}: {err}",
                self.path.display()
            ))
        })?;

        // `by_name` rejects duplicate central-directory names itself, so a
        // crafted archive cannot smuggle a second payload under the same name.
        let mut entry = archive.by_name(payload).map_err(|err| {
            invalid_data(format!(
                "{DIST_ARCHIVE_FILE} could not read payload {payload}: {err}"
            ))
        })?;
        check_entry_size(entry.size())?;

        let path = dir.join(payload);
        let mut out = File::create(&path)?;
        let written = io::copy(&mut entry.by_ref().take(MAX_DIST_ENTRY_BYTES + 1), &mut out)?;
        if written > MAX_DIST_ENTRY_BYTES {
            return Err(entry_too_large(written));
        }

        crate::checksum::verify_sha256(&path, &sha256)?;

        if format == DistFormat::Exe
            && !file_contains(&path, crate::build_info::DIST_IDENTITY_MARKER.as_bytes())?
        {
            return Err(invalid_data(format!(
                "{DIST_ARCHIVE_FILE} payload {payload} does not carry build identity {}; package the {asset_key} binary produced by this build",
                crate::build_info::DIST_IDENTITY_MARKER
            )));
        }
        // A nested Windows package is validated by the packager, which reads
        // herdr.exe out of the inner zip and checks the same identity.

        #[cfg(unix)]
        if format == DistFormat::Exe {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }

        Ok((path, format))
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

fn payload_file_name(path: &str) -> io::Result<&str> {
    if path.contains('\\') {
        return Err(invalid_data(format!(
            "{DIST_ARCHIVE_FILE} payload path {path} must use forward slashes and name a single file"
        )));
    }
    let parsed = Path::new(path);
    let mut components = parsed.components();
    let single = components.next();
    if components.next().is_some() || !matches!(single, Some(Component::Normal(_))) {
        return Err(invalid_data(format!(
            "{DIST_ARCHIVE_FILE} payload path {path} must name a single file inside the archive"
        )));
    }
    parsed
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            invalid_data(format!(
                "{DIST_ARCHIVE_FILE} payload path {path} is not valid UTF-8"
            ))
        })
}

fn check_entry_size(size: u64) -> io::Result<()> {
    if size > MAX_DIST_ENTRY_BYTES {
        return Err(entry_too_large(size));
    }
    Ok(())
}

fn entry_too_large(size: u64) -> io::Error {
    invalid_data(format!(
        "{DIST_ARCHIVE_FILE} payload entry is too large to install ({size} bytes, limit {MAX_DIST_ENTRY_BYTES})"
    ))
}

fn file_contains(path: &Path, needle: &[u8]) -> io::Result<bool> {
    if needle.is_empty() {
        return Ok(true);
    }
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut window = Vec::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(false);
        }
        window.extend_from_slice(&buffer[..read]);
        if window
            .windows(needle.len())
            .any(|candidate| candidate == needle)
        {
            return Ok(true);
        }
        let keep = needle.len().saturating_sub(1);
        let start = window.len().saturating_sub(keep);
        window.drain(..start);
    }
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
        let dir =
            std::env::temp_dir().join(format!("herdr-dist-test-{name}-{}", std::process::id()));
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

    fn linux_payload() -> Vec<u8> {
        let mut payload = b"payload:".to_vec();
        payload.extend_from_slice(crate::build_info::DIST_IDENTITY_MARKER.as_bytes());
        payload
    }

    fn valid_manifest(version: &str, protocol: u32) -> serde_json::Value {
        json!({
            "schema_version": 1,
            "build_identity": crate::build_info::DIST_IDENTITY_MARKER,
            "version": version,
            "channel": "stable",
            "build_id": null,
            "protocol": protocol,
            "entries": {
                "linux-x86_64": {
                    "path": "herdr-linux-x86_64",
                    "sha256": sha_hex(&linux_payload()),
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
            .write_all(
                serde_json::to_string(&manifest)
                    .expect("serialize")
                    .as_bytes(),
            )
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
            valid_manifest(
                &crate::build_info::version(),
                crate::protocol::PROTOCOL_VERSION,
            ),
            &[
                ("herdr-linux-x86_64", linux_payload().as_slice()),
                ("herdr-windows-x86_64.zip", b"windows"),
            ],
        );

        let archive = DistArchive::open(path).expect("open archive");
        let entry = archive.entry("linux-x86_64").expect("linux entry");
        assert_eq!(entry.format, DistFormat::Exe);
        assert_eq!(entry.path, "herdr-linux-x86_64");
        assert_eq!(entry.sha256, sha_hex(&linux_payload()));
        assert_eq!(
            archive
                .entry("windows-x86_64")
                .expect("windows entry")
                .format,
            DistFormat::Zip
        );
    }

    #[test]
    fn rejects_stale_version() {
        let dir = temp_dir("stale-version");
        let path = write_archive(
            &dir,
            valid_manifest("0.0.1", crate::protocol::PROTOCOL_VERSION),
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
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
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
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
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["schema_version"] = json!(2);
        let path = write_archive(
            &dir,
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );

        let err = DistArchive::open(path).expect_err("unknown schema must fail");
        assert!(err.to_string().contains("schema"), "{err}");
    }

    #[test]
    fn rejects_archive_from_another_build() {
        let dir = temp_dir("other-build");
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["build_identity"] = json!("herdr-dist-identity:2:0.0.0:1:stable:-:-");
        let path = write_archive(
            &dir,
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );

        let err = DistArchive::open(path).expect_err("other build must fail");
        let message = err.to_string();
        assert!(
            message.contains("herdr-dist-identity:2:0.0.0:1:stable:-:-"),
            "{message}"
        );
        assert!(
            message.contains(crate::build_info::DIST_IDENTITY_MARKER),
            "{message}"
        );
    }

    #[test]
    fn rejects_payload_without_build_identity() {
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["entries"]["linux-x86_64"]["sha256"] = json!(sha_hex(b"payload"));
        let archive = archive_with(
            "payload-without-identity-archive",
            manifest,
            &[("herdr-linux-x86_64", b"payload")],
        );

        let err = archive
            .extract_entry("linux-x86_64", temp_dir("payload-without-identity-out"))
            .expect_err("payload from another build must fail");
        assert!(err.to_string().contains("build identity"), "{err}");
    }

    #[test]
    fn reports_missing_platform() {
        let dir = temp_dir("missing-platform");
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["entries"] = json!({
            "linux-x86_64": {
                "path": "herdr-linux-x86_64",
                "sha256": sha_hex(&linux_payload()),
                "format": "exe",
            },
        });
        let path = write_archive(
            &dir,
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );

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
            .start_file(
                "herdr-linux-x86_64",
                zip::write::SimpleFileOptions::default(),
            )
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
        let err =
            locate_from(Some(missing), &config, None).expect_err("explicit env path must exist");
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

    fn archive_with(
        name: &str,
        manifest: serde_json::Value,
        payloads: &[(&str, &[u8])],
    ) -> DistArchive {
        let dir = temp_dir(name);
        DistArchive::open(write_archive(&dir, manifest, payloads)).expect("open archive")
    }

    #[test]
    fn extracts_linux_payload_with_checksum() {
        let archive = archive_with(
            "extract-linux",
            valid_manifest(
                &crate::build_info::version(),
                crate::protocol::PROTOCOL_VERSION,
            ),
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );
        let dir = temp_dir("extract-linux-out");

        let install = archive
            .extract_entry("linux-x86_64", dir.clone())
            .expect("extract");
        assert_eq!(install.format, DistFormat::Exe);
        assert_eq!(
            fs::read(&install.path).expect("read payload"),
            linux_payload()
        );
        assert_eq!(
            crate::checksum::file_sha256(&install.path).expect("hash"),
            sha_hex(&linux_payload())
        );
        assert_eq!(install.temporary_dir, dir);
    }

    #[test]
    fn rejects_checksum_mismatch_and_cleans_up() {
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["entries"]["linux-x86_64"]["sha256"] = json!("0".repeat(64));
        let archive = archive_with(
            "extract-bad-sha",
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );
        let dir = temp_dir("extract-bad-sha-out");

        let err = archive
            .extract_entry("linux-x86_64", dir.clone())
            .expect_err("checksum mismatch must fail");
        assert!(err.to_string().contains("sha256"), "{err}");
        assert!(!dir.exists(), "temporary directory must be removed");
    }

    #[test]
    fn check_entry_size_enforces_limit() {
        assert!(check_entry_size(MAX_DIST_ENTRY_BYTES).is_ok());
        let err = check_entry_size(MAX_DIST_ENTRY_BYTES + 1).expect_err("over limit");
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn rejects_absolute_path_in_manifest() {
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["entries"]["linux-x86_64"]["path"] = json!("/etc/passwd");
        let archive = archive_with(
            "extract-absolute",
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );

        let err = archive
            .extract_entry("linux-x86_64", temp_dir("extract-absolute-out"))
            .expect_err("absolute path must fail");
        assert!(err.to_string().contains("path"), "{err}");
    }

    #[test]
    fn rejects_parent_component() {
        let mut manifest = valid_manifest(
            &crate::build_info::version(),
            crate::protocol::PROTOCOL_VERSION,
        );
        manifest["entries"]["linux-x86_64"]["path"] = json!("../herdr");
        let archive = archive_with(
            "extract-parent",
            manifest,
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );

        let err = archive
            .extract_entry("linux-x86_64", temp_dir("extract-parent-out"))
            .expect_err("parent component must fail");
        assert!(err.to_string().contains("path"), "{err}");
    }

    #[test]
    fn windows_payload_lands_as_zip() {
        let archive = archive_with(
            "extract-windows",
            valid_manifest(
                &crate::build_info::version(),
                crate::protocol::PROTOCOL_VERSION,
            ),
            &[("herdr-windows-x86_64.zip", b"windows")],
        );
        let dir = temp_dir("extract-windows-out");

        let install = archive
            .extract_entry("windows-x86_64", dir)
            .expect("extract windows package");
        assert_eq!(install.format, DistFormat::Zip);
        assert_eq!(
            install.path.file_name().and_then(|name| name.to_str()),
            Some("herdr-windows-x86_64.zip")
        );
        assert_eq!(fs::read(&install.path).expect("read payload"), b"windows");
    }

    #[cfg(unix)]
    #[test]
    fn exe_payload_is_executable() {
        use std::os::unix::fs::PermissionsExt;

        let archive = archive_with(
            "extract-executable",
            valid_manifest(
                &crate::build_info::version(),
                crate::protocol::PROTOCOL_VERSION,
            ),
            &[("herdr-linux-x86_64", linux_payload().as_slice())],
        );
        let install = archive
            .extract_entry("linux-x86_64", temp_dir("extract-executable-out"))
            .expect("extract");
        let mode = fs::metadata(&install.path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }
}
