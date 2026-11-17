from __future__ import annotations

import os
import subprocess
import tempfile
from pathlib import Path


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _binary() -> str:
    bin_path = os.environ.get("BENCH_EB_PIPELINE_BIN")
    if not bin_path:
        raise RuntimeError("BENCH_EB_PIPELINE_BIN is not set")
    return bin_path


class TimeCliHotPaths:
    def setup(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        tmp = Path(self._tmp.name)
        self.site = _repo_root() / "templates" / "small-cluster.toml"
        self.list_path = tmp / "list.txt"
        self.list_path.write_text(
            "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb --rebuild\n",
            encoding="utf-8",
        )

    def teardown(self) -> None:
        self._tmp.cleanup()

    def _run(self, args: list[str]) -> None:
        subprocess.run(
            [_binary(), *args],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )

    def time_check_site(self) -> None:
        self._run(["check", "--site", str(self.site)])

    def time_targets_json(self) -> None:
        self._run(["targets", "--site", str(self.site), "--format", "json"])

    def time_explain_target_json(self) -> None:
        self._run(
            [
                "explain",
                "--site",
                str(self.site),
                "--target",
                "cpu",
                "--build-type",
                "production",
                "--format",
                "json",
            ]
        )

    def time_plan_target_list(self) -> None:
        self._run(
            [
                "plan",
                "--site",
                str(self.site),
                "--list",
                str(self.list_path),
                "--target",
                "cpu",
                "--build-type",
                "production",
            ]
        )
