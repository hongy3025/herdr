//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Compiled-in identity read by the `herdr-dist.zip` packager.
///
/// `concat!` only accepts literals, so the protocol number is duplicated here;
/// `dist_identity_marker_tracks_version_and_protocol` keeps it in sync with
/// `PROTOCOL_VERSION`.
/// Written by `build.rs`; `remote::dist` compares it with archives and scans
/// payloads for it, so the bytes stay in the binary.
include!(concat!(env!("OUT_DIR"), "/dist_identity.rs"));

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
    fn dist_identity_marker_tracks_the_build_identity() {
        let parts = super::DIST_IDENTITY_MARKER.split(':').collect::<Vec<_>>();
        assert_eq!(parts.len(), 7, "{}", super::DIST_IDENTITY_MARKER);
        assert_eq!(parts[0], "herdr-dist-identity");
        assert_eq!(parts[1], "2");
        assert_eq!(parts[2], env!("CARGO_PKG_VERSION"));
        assert_eq!(parts[3], crate::protocol::PROTOCOL_VERSION.to_string());
        assert_eq!(parts[4], super::channel());
        assert_eq!(parts[5], super::build_id().unwrap_or("-"));
    }
}
