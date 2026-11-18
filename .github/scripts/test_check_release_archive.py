#!/usr/bin/env python3
"""Focused tests for release archive validation."""

import importlib.util
import io
import pathlib
import tarfile
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check-release-archive.py")
SPEC = importlib.util.spec_from_file_location("check_release_archive", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ReleaseArchiveTests(unittest.TestCase):
    def test_only_executable_files_count_as_release_binaries(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = pathlib.Path(tmp) / "release.tar.xz"
            with tarfile.open(archive, "w:xz") as tar:
                for name, mode in [
                    ("release/eb-pipeline", 0o755),
                    ("release/eb-junit", 0o644),
                ]:
                    member = tarfile.TarInfo(name)
                    member.mode = mode
                    member.size = 1
                    tar.addfile(member, io.BytesIO(b"x"))

            self.assertEqual(CHECK.archive_bins(archive), {"eb-pipeline"})


if __name__ == "__main__":
    unittest.main()
