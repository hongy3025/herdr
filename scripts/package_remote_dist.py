#!/usr/bin/env python3
"""Package locally built Herdr payloads for remote installs.

The resulting ``herdr-dist.zip`` is the sidecar archive Herdr looks up before
falling back to the published release manifest when installing onto a remote
machine. This script only packages existing builds; it never compiles or
cross-compiles them.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import struct
import sys
import zipfile
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
if __package__ in (None, ""):
    sys.path.insert(0, str(PROJECT_ROOT))

from scripts.changelog import read_protocol_version  # noqa: E402

CARGO_TOML = PROJECT_ROOT / "Cargo.toml"

DIST_MANIFEST_ENTRY = "herdr-dist.json"
LINUX_ENTRY = "herdr-linux-x86_64"
WINDOWS_ENTRY = "herdr-windows-x86_64.zip"
IDENTITY_MARKER_PREFIX = "herdr-dist-identity:1:"
SCHEMA_VERSION = 1
ZIP_EPOCH = (1980, 1, 1, 0, 0, 0)
ELF_MACHINE_X86_64 = 62
WINDOWS_REQUIRED_ENTRIES = ("herdr.exe", "conpty/herdr-conpty.json")

_PACKAGE_SECTION_RE = re.compile(r"(?ms)^\[package\]\s*$(.*?)(?=^\[)")
_VERSION_RE = re.compile(r'(?m)^version\s*=\s*"([^"]+)"\s*$')


class PackageError(ValueError):
    """Raised when an input payload cannot be packaged."""


def read_package_version(cargo_toml: Path = CARGO_TOML) -> str:
    text = cargo_toml.read_text(encoding="utf-8")
    section = _PACKAGE_SECTION_RE.search(text)
    if section is None:
        raise PackageError(f"{cargo_toml} has no [package] section")
    match = _VERSION_RE.search(section.group(1))
    if match is None:
        raise PackageError(f"{cargo_toml} has no [package] version")
    return match.group(1)


def build_version(base_version: str, channel: str, build_id: str | None) -> str:
    if channel == "stable":
        return base_version
    if build_id:
        return f"{base_version}-{channel}.{build_id}"
    return f"{base_version}-{channel}"


def identity_marker(base_version: str, protocol: int) -> bytes:
    return f"{IDENTITY_MARKER_PREFIX}{base_version}:{protocol}".encode()


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def validate_linux_payload(path: Path, marker: bytes) -> bytes:
    data = path.read_bytes()
    if len(data) < 20 or data[:4] != b"\x7fELF":
        raise PackageError(f"{path} is not an ELF binary")
    if data[4] != 2:
        raise PackageError(f"{path} is not a 64-bit ELF binary")
    (machine,) = struct.unpack_from("<H", data, 18)
    if machine != ELF_MACHINE_X86_64:
        raise PackageError(
            f"{path} targets ELF machine {machine}; expected x86_64 ({ELF_MACHINE_X86_64})"
        )
    if marker not in data:
        raise PackageError(
            f"{path} does not contain {marker.decode()} from this build; "
            "pass the binary produced by the build you are packaging"
        )
    return data


def validate_windows_payload(path: Path, marker: bytes) -> bytes:
    data = path.read_bytes()
    try:
        with zipfile.ZipFile(path) as archive:
            names = {name.replace("\\", "/") for name in archive.namelist()}
            missing = [name for name in WINDOWS_REQUIRED_ENTRIES if name not in names]
            if missing:
                raise PackageError(
                    f"{path} is missing {', '.join(missing)}; "
                    "remote installs need the complete ConPTY package"
                )
            executable = archive.read("herdr.exe")
    except zipfile.BadZipFile as error:
        raise PackageError(f"{path} is not a readable zip archive: {error}") from error
    if marker not in executable:
        raise PackageError(
            f"{path} contains a herdr.exe without {marker.decode()} from this build"
        )
    return data


def write_entry(
    archive: zipfile.ZipFile,
    name: str,
    data: bytes,
    *,
    compress_type: int,
    mode: int,
) -> None:
    info = zipfile.ZipInfo(name, date_time=ZIP_EPOCH)
    info.compress_type = compress_type
    info.create_system = 3
    info.external_attr = mode << 16
    archive.writestr(info, data)


def build_manifest(
    *,
    version: str,
    channel: str,
    build_id: str | None,
    protocol: int,
    linux_sha256: str,
    windows_sha256: str,
) -> dict[str, object]:
    return {
        "schema_version": SCHEMA_VERSION,
        "version": version,
        "channel": channel,
        "build_id": build_id,
        "protocol": protocol,
        "entries": {
            "linux-x86_64": {
                "path": LINUX_ENTRY,
                "sha256": linux_sha256,
                "format": "exe",
            },
            "windows-x86_64": {
                "path": WINDOWS_ENTRY,
                "sha256": windows_sha256,
                "format": "zip",
            },
        },
    }


def package_dist(
    linux_path: Path,
    windows_path: Path,
    output: Path,
    *,
    version: str | None = None,
    channel: str = "stable",
    build_id: str | None = None,
    protocol: int | None = None,
) -> Path:
    base_version = read_package_version()
    protocol = (
        read_protocol_version(PROJECT_ROOT / "src/protocol/wire.rs")
        if protocol is None
        else protocol
    )
    marker = identity_marker(base_version, protocol)
    linux_data = validate_linux_payload(linux_path, marker)
    windows_data = validate_windows_payload(windows_path, marker)
    full_version = version or build_version(base_version, channel, build_id)

    manifest = build_manifest(
        version=full_version,
        channel=channel,
        build_id=build_id,
        protocol=protocol,
        linux_sha256=sha256_hex(linux_data),
        windows_sha256=sha256_hex(windows_data),
    )
    manifest_bytes = (
        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + ".tmp")
    with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        write_entry(
            archive,
            DIST_MANIFEST_ENTRY,
            manifest_bytes,
            compress_type=zipfile.ZIP_DEFLATED,
            mode=0o644,
        )
        write_entry(
            archive,
            LINUX_ENTRY,
            linux_data,
            compress_type=zipfile.ZIP_DEFLATED,
            mode=0o755,
        )
        write_entry(
            archive,
            WINDOWS_ENTRY,
            windows_data,
            compress_type=zipfile.ZIP_STORED,
            mode=0o644,
        )
    os.replace(temporary, output)
    return output


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--linux", required=True, type=Path, help="x86_64 Linux herdr binary")
    parser.add_argument(
        "--windows",
        required=True,
        type=Path,
        help="herdr-windows-x86_64.zip produced by package_windows_conpty.py",
    )
    parser.add_argument("--out", type=Path, default=Path("dist/herdr-dist.zip"))
    parser.add_argument("--version", help="override the base version from Cargo.toml")
    parser.add_argument("--channel", default="stable")
    parser.add_argument("--build-id")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        output = package_dist(
            args.linux,
            args.windows,
            args.out,
            version=args.version,
            channel=args.channel,
            build_id=args.build_id,
        )
    except (OSError, PackageError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())