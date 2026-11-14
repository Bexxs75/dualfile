#!/usr/bin/env python3
"""Offline proof: python3 scripts/test_make_latest_json.py."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("make-latest-json.py")
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("make_latest_json", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.sig = Path(self.temp.name) / "Dualfile-0.1.0-x86_64.AppImage.sig"
        self.sig.write_text("signature-fixture\n", encoding="utf-8")
        self.url = "https://github.com/Bexxs75/dualfile/releases/download/v0.1.0"

    def manifest(self, **kwargs):
        args = dict(version="v0.1.0", release_url=self.url, signature_files={"linux-x86_64": self.sig},
                    notes="Änderungen\nChanges", pub_date="2026-11-13T12:00:00Z")
        args.update(kwargs)
        return MODULE.make_manifest(**args)

    def test_exact_schema(self):
        self.assertEqual(self.manifest(), {"version": "0.1.0", "notes": "Änderungen\nChanges",
            "pub_date": "2026-11-13T12:00:00Z", "platforms": {"linux-x86_64": {
                "signature": "signature-fixture", "url": self.url + "/Dualfile-0.1.0-x86_64.AppImage"}}})

    def test_two_platforms(self):
        arm = self.sig.with_name("Dualfile-0.1.0-aarch64.AppImage.sig")
        arm.write_text("arm-signature\n", encoding="utf-8")
        platforms = self.manifest(signature_files={"linux-x86_64": self.sig, "linux-aarch64": arm})["platforms"]
        self.assertEqual(platforms, {
            "linux-x86_64": {"signature": "signature-fixture", "url": self.url + "/Dualfile-0.1.0-x86_64.AppImage"},
            "linux-aarch64": {"signature": "arm-signature", "url": self.url + "/Dualfile-0.1.0-aarch64.AppImage"}})

    def test_signature_required(self):
        with self.assertRaises(ValueError):
            self.manifest(signature_files={})

    def test_bad_platform(self):
        with self.assertRaises(ValueError):
            self.manifest(signature_files={"linux-arm": self.sig})

    def test_cli_two_platforms_and_duplicate(self):
        arm = self.sig.with_name("Dualfile-0.1.0-aarch64.AppImage.sig")
        arm.write_text("arm-signature", encoding="utf-8")
        output = Path(self.temp.name) / "latest.json"
        base = [sys.executable, str(SCRIPT), "--version", "0.1.0", "--release-url", self.url, "--output", str(output)]
        ok = subprocess.run(base + ["--signature", f"linux-x86_64={self.sig}", "--signature", f"linux-aarch64={arm}"], capture_output=True, text=True)
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertEqual(sorted(json.loads(output.read_text())["platforms"]), ["linux-aarch64", "linux-x86_64"])
        for bad in (["--signature", str(self.sig)], ["--signature", f"linux-x86_64={self.sig}", "--signature", f"linux-x86_64={self.sig}"]):
            self.assertEqual(subprocess.run(base + bad, capture_output=True, text=True).returncode, 2)

    def test_semver(self):
        self.assertEqual(self.manifest(version="1.2.3-rc.1+build.7")["version"], "1.2.3-rc.1+build.7")
        for version in ("", "1.2", "01.2.3", "1.2.3-01", "1.2.3\n", "1.2.3-"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                self.manifest(version=version)

    def test_invalid_urls(self):
        for url in ("http://example.org", "https:///missing", "https://user:pass@example.org", "https://example.org?a=1", "https://example.org/#fragment", "https://bad host"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                self.manifest(release_url=url)

    def test_asset_encoding_and_trailing_slash(self):
        sig = self.sig.with_name("Dualfile test.AppImage.sig")
        sig.write_text("signed", encoding="utf-8")
        self.assertEqual(self.manifest(signature_files={"linux-x86_64": sig}, release_url=self.url + "/")["platforms"]["linux-x86_64"]["url"], self.url + "/Dualfile%20test.AppImage")

    def test_missing_signature(self):
        self.sig.unlink()
        with self.assertRaises(FileNotFoundError):
            self.manifest()

    def test_empty_signature(self):
        self.sig.write_text(" \n", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.manifest()

    def test_wrong_asset_type(self):
        with self.assertRaises(ValueError):
            self.manifest(signature_files={"linux-x86_64": self.sig.with_suffix(".txt")})

    def test_invalid_dates(self):
        for date in ("2026-11-13", "2026-11-13T12:00:00", "2026-02-30T12:00:00Z"):
            with self.subTest(date=date), self.assertRaises(ValueError):
                self.manifest(pub_date=date)

    def test_default_date(self):
        self.assertRegex(self.manifest(pub_date=None)["pub_date"], r"T\d\d:\d\d:\d\dZ$")

    def test_cli(self):
        output = Path(self.temp.name) / "latest.json"
        notes = Path(self.temp.name) / "notes.md"
        notes.write_text("Neu / New", encoding="utf-8")
        result = subprocess.run([sys.executable, str(SCRIPT), "--version", "0.1.0", "--release-url", self.url,
            "--signature", f"linux-x86_64={self.sig}", "--notes-file", str(notes), "--pub-date", "2026-11-13T12:00:00Z",
            "--output", str(output)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(output.read_text())["notes"], "Neu / New")

    def test_cli_failure_does_not_write(self):
        output = Path(self.temp.name) / "latest.json"
        result = subprocess.run([sys.executable, str(SCRIPT), "--version", "invalid", "--release-url", self.url,
            "--signature", f"linux-x86_64={self.sig}", "--output", str(output)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
