//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Compiled-in identity read by the `herdr-dist.zip` packager.
///
/// `concat!` only accepts literals, so the protocol number is duplicated here;
/// `dist_identity_marker_tracks_version_and_protocol` keeps it in sync with
/// `PROTOCOL_VERSION`.
/// `remote::dist` logs this value so the bytes stay in the binary; the
/// packager scans payloads for it.
pub const DIST_IDENTITY_MARKER: &str =
    concat!("herdr-dist-identity:1:", env!("CARGO_PKG_VERSION"), ":22");

pub fn channel() -> &'static str {
    non_empty(option_env!("HERDR_BUILD_CHANNEL")).unwrap_or("stable")
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID"))
}

pub fn version() -> String {
    match channel() {
        "stable" => BASE_VERSION.to_string(),
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }

    #[test]
    fn dist_identity_marker_tracks_version_and_protocol() {
        let expected = format!(
            "herdr-dist-identity:1:{}:{}",
            env!("CARGO_PKG_VERSION"),
            crate::protocol::PROTOCOL_VERSION
        );
        assert_eq!(super::DIST_IDENTITY_MARKER, expected);
    }
}
