#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("changelog_notes", Path(__file__).with_name("changelog-notes.py"))
notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notes)

LOG = """# Changelog

## [Unreleased]

## [0.3.5] - 2026-11-17

### Added

- New thing.
#### Detail stays

## [0.3.4] - 2026-11-16

### Fixed

- Old thing.
"""


class Section(unittest.TestCase):
    def test_extracts_only_that_version_with_tag_prefix(self):
        self.assertEqual(notes.section(LOG, "v0.3.5"), "## Added\n\n- New thing.\n#### Detail stays\n")

    def test_last_section(self):
        self.assertEqual(notes.section(LOG, "0.3.4"), "## Fixed\n\n- Old thing.\n")

    def test_missing_or_empty_fails(self):
        for version in ("0.9.9", "Unreleased", "0.3"):
            with self.assertRaises(ValueError):
                notes.section(LOG, version)


if __name__ == "__main__":
    unittest.main()
