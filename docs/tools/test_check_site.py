#!/usr/bin/env python3
"""Focused tests for the built-site checker."""

import pathlib
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO

import check_site


class CheckSiteTests(unittest.TestCase):
    def test_malformed_url_is_reported_without_stopping_the_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "index.html").write_text(
                '<a href="https://[bad/path">Broken</a>', encoding="utf-8"
            )
            errors, count = check_site.check_site(root, [])
            self.assertEqual(count, 1)
            self.assertEqual(
                errors,
                ["index.html:1: invalid URL: https://[bad/path (Invalid IPv6 URL)"],
            )

    def test_two_dot_file_name_stays_inside_the_built_site(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "index.html").write_text(
                '<a href="..guide.html">Guide</a>', encoding="utf-8"
            )
            (root / "..guide.html").write_text("<p>Guide</p>", encoding="utf-8")
            errors, count = check_site.check_site(root, [])
            self.assertEqual(count, 2)
            self.assertEqual(errors, [])

    def test_root_relative_links_stay_inside_the_built_site(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "index.html").write_text(
                '<a href="/guide.html#topic">Guide</a>', encoding="utf-8"
            )
            (root / "guide.html").write_text(
                '<h1 id="topic">Topic</h1>', encoding="utf-8"
            )
            errors, count = check_site.check_site(root, [])
            self.assertEqual(count, 2)
            self.assertEqual(errors, [])

    def test_invalid_forbidden_pattern_is_a_usage_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "index.html").write_text("<p>Clean</p>", encoding="utf-8")
            stderr = StringIO()
            with redirect_stderr(stderr):
                status = check_site.main([str(root), "--forbid", "["])
            self.assertEqual(status, 2)
            self.assertIn("invalid --forbid regular expression", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
