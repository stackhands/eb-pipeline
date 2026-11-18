#!/usr/bin/env python3
"""Focused tests for the built-site checker."""

import pathlib
import tempfile
import unittest

import check_site


class CheckSiteTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
