#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("crn", Path(__file__).with_name("check-release-notes.py"))
crn = importlib.util.module_from_spec(spec)
spec.loader.exec_module(crn)

REAL = (Path(__file__).parent.parent / "src" / "release-notes.ts").read_text(encoding="utf-8")


def entry(de, en, date='date: "2026-01-02",'):
    return f'export const n = {{\n  "9.9.9": {{\n    {date}\n    de: {de},\n    en: {en},\n  }},\n}};\n'


OK = '{ new: ["a"], improved: [], fixed: [] }'


class Notes(unittest.TestCase):
    def test_real_entries(self):
        for v in ("0.3.3", "0.3.4", "0.4.3"):
            self.assertEqual(crn.check_notes(REAL, v), [], v)

    def test_missing_entry(self):
        self.assertTrue(crn.check_notes(REAL, "99.0.0"))

    def test_word_version_is_fine_but_markers_are_not(self):
        self.assertEqual(crn.check_notes(entry(OK, '{ new: ["The window title shows the version."] }'), "9.9.9"), [])
        for bad in ("TODO later", "<describe change>", "VERSION", "{{x}}", "XXX"):
            self.assertTrue(crn.check_notes(entry(OK, '{ new: ["%s"] }' % bad), "9.9.9"), bad)

    def test_empty_or_missing_language_and_date(self):
        self.assertTrue(crn.check_notes(entry(OK, "{ new: [], improved: [], fixed: [] }"), "9.9.9"))
        self.assertTrue(crn.check_notes(entry(OK, '{ new: [" "] }'), "9.9.9"))
        self.assertTrue(crn.check_notes('x = {\n  "9.9.9": {\n    date: "2026-01-02",\n    de: { new: ["a"] },\n  },\n};', "9.9.9"))
        self.assertTrue(crn.check_notes(entry(OK, OK, date='date: "soon",'), "9.9.9"))

    def test_braces_in_strings(self):
        self.assertEqual(crn.check_notes(entry('{ new: ["a } b"] }', OK), "9.9.9"), [])


class Changelog(unittest.TestCase):
    def test_changelog(self):
        self.assertEqual(crn.check_changelog("## [1.0.0]\n\n- Fixed it.\n", "1.0.0"), [])
        self.assertTrue(crn.check_changelog("## [1.0.0]\n\n- TODO\n", "1.0.0"))
        self.assertTrue(crn.check_changelog("## [1.0.0]\n\n", "1.0.0"))
        self.assertTrue(crn.check_changelog("## [1.0.0]\n\n- x\n", "2.0.0"))


if __name__ == "__main__":
    unittest.main()
