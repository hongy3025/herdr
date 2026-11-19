from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
import zipfile
from pathlib import Path

from scripts.changelog import read_protocol_version
from scripts.package_remote_dist import (
    DIST_MANIFEST_ENTRY,
    LINUX_ENTRY,
    WINDOWS_ENTRY,
    PackageError,
    build_version,
    package_dist,
    read_package_version,
)

BASE_VERSION = read_package_version()
PROTOCOL = read_protocol_version()


def identity(version: str = BASE_VERSION, protocol: int = PROTOCOL, channel: str = "stable") -> bytes:
    return f"herdr-dist-identity:2:{version}:{protocol}:{channel}:-:-".encode()


MARKER = identity()


def fake_linux(marker: bytes = MARKER, machine: int = 62, elf: bool = True) -> bytes:
    ident = b"\x7fELF\x02\x01\x01\x00" + b"\x00" * 8 if elf else b"not an elf binary!!"
    header = ident + (2).to_bytes(2, "little") + machine.to_bytes(2, "little") + b"\x00" * 40
    return header + marker


def fake_windows(marker: bytes = MARKER, include_conpty: bool = True) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("herdr.exe", b"MZ" + marker)
        if include_conpty:
            archive.writestr("conpty/herdr-conpty.json", b"{}")
    return buffer.getvalue()


class PackageDistTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.linux = self.root / "herdr-linux-x86_64"
        self.windows = self.root / "herdr-windows-x86_64.zip"
        self.linux.write_bytes(fake_linux())
        self.windows.write_bytes(fake_windows())

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def package(self, output: Path | None = None, **kwargs) -> Path:
        return package_dist(
            self.linux,
            self.windows,
            output or self.root / "herdr-dist.zip",
            **kwargs,
        )

    def test_writes_manifest_and_payloads(self) -> None:
        output = self.package()
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read(DIST_MANIFEST_ENTRY))
            self.assertEqual(manifest["schema_version"], 1)
            self.assertEqual(manifest["build_identity"], MARKER.decode())
            self.assertEqual(manifest["version"], BASE_VERSION)
            self.assertEqual(manifest["protocol"], PROTOCOL)
            linux_entry = manifest["entries"]["linux-x86_64"]
            windows_entry = manifest["entries"]["windows-x86_64"]
            self.assertEqual(linux_entry["format"], "exe")
            self.assertEqual(windows_entry["format"], "zip")
            self.assertEqual(archive.read(LINUX_ENTRY), self.linux.read_bytes())
            self.assertEqual(archive.read(WINDOWS_ENTRY), self.windows.read_bytes())
            self.assertEqual(len(linux_entry["sha256"]), 64)
            self.assertEqual(len(windows_entry["sha256"]), 64)

    def test_identity_scan_stops_at_adjacent_text(self) -> None:
        with_suffix = MARKER + b"trailing-literal"
        self.linux.write_bytes(fake_linux(marker=with_suffix))
        self.windows.write_bytes(fake_windows(marker=with_suffix))
        output = self.package()
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read(DIST_MANIFEST_ENTRY))
        self.assertEqual(manifest["build_identity"], MARKER.decode())

    def test_output_is_deterministic(self) -> None:
        first = self.package(self.root / "first.zip").read_bytes()
        second = self.package(self.root / "second.zip").read_bytes()
        self.assertEqual(first, second)

    def test_windows_entry_is_stored(self) -> None:
        output = self.package()
        with zipfile.ZipFile(output) as archive:
            self.assertEqual(
                archive.getinfo(WINDOWS_ENTRY).compress_type, zipfile.ZIP_STORED
            )
            self.assertEqual(
                archive.getinfo(LINUX_ENTRY).compress_type, zipfile.ZIP_DEFLATED
            )

    def test_rejects_non_elf_linux(self) -> None:
        self.linux.write_bytes(fake_linux(elf=False))
        with self.assertRaisesRegex(PackageError, "ELF"):
            self.package()

    def test_rejects_wrong_elf_machine(self) -> None:
        self.linux.write_bytes(fake_linux(machine=183))
        with self.assertRaisesRegex(PackageError, "x86_64"):
            self.package()

    def test_rejects_windows_zip_without_conpty(self) -> None:
        self.windows.write_bytes(fake_windows(include_conpty=False))
        with self.assertRaisesRegex(PackageError, "ConPTY"):
            self.package()

    def test_rejects_payload_without_identity_marker(self) -> None:
        self.linux.write_bytes(fake_linux(marker=b"other-build"))
        with self.assertRaisesRegex(PackageError, "herdr-dist-identity"):
            self.package()

    def test_rejects_windows_executable_from_another_build(self) -> None:
        self.windows.write_bytes(fake_windows(marker=b"other-build"))
        with self.assertRaisesRegex(PackageError, "herdr.exe"):
            self.package()

    def test_rejects_payloads_from_different_builds(self) -> None:
        self.windows.write_bytes(fake_windows(marker=identity(channel="custom")))
        with self.assertRaisesRegex(PackageError, "build identities differ"):
            self.package()

    def test_rejects_payload_for_another_source_version(self) -> None:
        self.linux.write_bytes(fake_linux(marker=identity(version="0.0.1")))
        self.windows.write_bytes(fake_windows(marker=identity(version="0.0.1")))
        with self.assertRaisesRegex(PackageError, "version 0.0.1"):
            self.package()

    def test_rejects_payload_for_another_protocol(self) -> None:
        self.linux.write_bytes(fake_linux(marker=identity(protocol=PROTOCOL + 1)))
        self.windows.write_bytes(fake_windows(marker=identity(protocol=PROTOCOL + 1)))
        with self.assertRaisesRegex(PackageError, "protocol"):
            self.package()

    def test_rejects_payload_for_another_channel(self) -> None:
        self.linux.write_bytes(fake_linux(marker=identity(channel="custom")))
        self.windows.write_bytes(fake_windows(marker=identity(channel="custom")))
        with self.assertRaisesRegex(PackageError, "channel"):
            self.package()

    def test_custom_channel_version(self) -> None:
        self.assertEqual(build_version("1.2.3", "preview", "abc"), "1.2.3-preview.abc")
        self.assertEqual(build_version("1.2.3", "preview", None), "1.2.3-preview")
        self.assertEqual(build_version("1.2.3", "stable", "abc"), "1.2.3")

        marker = identity(channel="preview")
        self.linux.write_bytes(fake_linux(marker=marker))
        self.windows.write_bytes(fake_windows(marker=marker))
        output = self.package(channel="preview", build_id="abc")
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read(DIST_MANIFEST_ENTRY))
        self.assertEqual(manifest["version"], f"{BASE_VERSION}-preview.abc")
        self.assertEqual(manifest["build_id"], "abc")

    def test_packages_from_another_working_directory(self) -> None:
        cwd = os.getcwd()
        os.chdir(self.root)
        try:
            output = self.package(self.root / "cwd.zip")
        finally:
            os.chdir(cwd)
        self.assertTrue(output.is_file())


if __name__ == "__main__":
    unittest.main()