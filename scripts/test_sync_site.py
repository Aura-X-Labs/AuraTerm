#!/usr/bin/env python3
"""Tests for scripts/sync_site.py.

Run from the repo root:

    python3 -m unittest discover -s scripts -p 'test_*.py'

These cover the release step that writes into the *other* repo (the AuraXLabs
site checkout). A silent failure there is expensive: the site keeps serving
the previous version's changelog and version number, and nothing in AuraTerm's
own build goes red.
"""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import sync_site  # noqa: E402


@contextlib.contextmanager
def temp_repo(version="1.2.3", changelog="# Changelog\n\n## 1.2.3\n\n- x\n"):
    """A throwaway AuraTerm checkout, entered as the working directory."""
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp) / "AuraTerm"
        repo.mkdir()
        (repo / "Changelog.md").write_text(changelog, encoding="utf-8")
        (repo / "package.json").write_text(
            json.dumps({"name": "auraterm", "version": version}),
            encoding="utf-8")
        cwd = os.getcwd()
        os.chdir(repo)
        try:
            yield repo
        finally:
            os.chdir(cwd)


@contextlib.contextmanager
def temp_site(exists=True):
    """A throwaway AuraXLabs checkout, exported via AURAXLABS_DIR."""
    with tempfile.TemporaryDirectory() as tmp:
        site = Path(tmp) / "AuraXLabs"
        if exists:
            (site / "app" / "data").mkdir(parents=True)
        previous = os.environ.get("AURAXLABS_DIR")
        os.environ["AURAXLABS_DIR"] = str(site)
        try:
            yield site
        finally:
            if previous is None:
                os.environ.pop("AURAXLABS_DIR", None)
            else:
                os.environ["AURAXLABS_DIR"] = previous


class RenderTest(unittest.TestCase):

    def test_changelog_copy_is_marked_as_generated(self):
        out = sync_site.render_changelog("# Changelog\n")
        self.assertTrue(out.startswith("<!--"))
        self.assertIn("do not edit here", out)
        # The body must survive verbatim; the site renders it as markdown.
        self.assertTrue(out.endswith("# Changelog\n"))

    def test_release_pin_is_json_with_the_version_and_a_trailing_newline(self):
        out = sync_site.render_release_pin("0.3.5")
        self.assertEqual(json.loads(out)["latest_version"], "0.3.5")
        self.assertIn("do not edit here", json.loads(out)["_comment"])
        # No "\ No newline at end of file" noise in the site's diff.
        self.assertTrue(out.endswith("}\n"))

    def test_release_pin_is_stable_across_runs(self):
        # An unstable rendering (dict ordering, a timestamp) would dirty the
        # site checkout on every release even when nothing changed.
        self.assertEqual(sync_site.render_release_pin("0.3.5"),
                         sync_site.render_release_pin("0.3.5"))


class WriteIfChangedTest(unittest.TestCase):

    def test_writes_a_missing_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "out.txt"
            self.assertTrue(sync_site.write_if_changed(target, "hello"))
            self.assertEqual(target.read_text(encoding="utf-8"), "hello")

    def test_rewrites_a_stale_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "out.txt"
            target.write_text("old", encoding="utf-8")
            self.assertTrue(sync_site.write_if_changed(target, "new"))
            self.assertEqual(target.read_text(encoding="utf-8"), "new")

    def test_leaves_an_identical_file_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "out.txt"
            target.write_text("same", encoding="utf-8")
            before = target.stat().st_mtime_ns
            self.assertFalse(sync_site.write_if_changed(target, "same"))
            self.assertEqual(target.stat().st_mtime_ns, before)


class ReadRepoVersionTest(unittest.TestCase):

    def test_reads_the_declared_version(self):
        with temp_repo(version="9.9.9"):
            self.assertEqual(sync_site.read_repo_version(), "9.9.9")

    def test_missing_package_json_is_empty_not_an_exception(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(
                sync_site.read_repo_version(Path(tmp) / "nope.json"), "")

    def test_unparseable_package_json_is_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            broken = Path(tmp) / "package.json"
            broken.write_text("{not json", encoding="utf-8")
            self.assertEqual(sync_site.read_repo_version(broken), "")

    def test_a_non_string_version_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            odd = Path(tmp) / "package.json"
            odd.write_text(json.dumps({"version": 3}), encoding="utf-8")
            self.assertEqual(sync_site.read_repo_version(odd), "")


class SiteDataDirTest(unittest.TestCase):

    def test_absent_checkout_reads_as_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(sync_site.site_data_dir(Path(tmp) / "nope"))

    def test_present_checkout_resolves_to_app_data(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "app" / "data").mkdir(parents=True)
            self.assertEqual(sync_site.site_data_dir(Path(tmp)),
                             Path(tmp) / "app" / "data")


class MainTest(unittest.TestCase):

    @staticmethod
    def run_main():
        """Run main() with its release narration swallowed."""
        with contextlib.redirect_stdout(io.StringIO()):
            return sync_site.main()

    def test_a_release_writes_both_files(self):
        with temp_repo(version="0.4.0",
                       changelog="# Changelog\n\n## 0.4.0\n\n- new\n"):
            with temp_site() as site:
                self.assertEqual(self.run_main(), 0)
                data = site / "app" / "data"
                self.assertIn("## 0.4.0", (data / "auraterm_changelog.md")
                              .read_text(encoding="utf-8"))
                pin = json.loads((data / "auraterm_release.json")
                                 .read_text(encoding="utf-8"))
                self.assertEqual(pin["latest_version"], "0.4.0")

    def test_running_twice_leaves_the_site_checkout_clean(self):
        with temp_repo(version="0.4.0",
                       changelog="# Changelog\n\n## 0.4.0\n"):
            with temp_site() as site:
                self.run_main()
                data = site / "app" / "data"
                stamps = {p: p.stat().st_mtime_ns for p in data.iterdir()}
                self.assertEqual(self.run_main(), 0)
                for path, before in stamps.items():
                    self.assertEqual(path.stat().st_mtime_ns, before,
                                     "%s was rewritten with identical content"
                                     % path.name)

    def test_a_missing_site_checkout_warns_instead_of_failing(self):
        # Release builds run on machines that only have AuraTerm.
        with temp_repo():
            with temp_site(exists=False) as site:
                self.assertEqual(self.run_main(), 0)
                self.assertFalse(site.exists())

    def test_a_missing_changelog_entry_still_syncs(self):
        # The warning is a nudge, not a gate: the version pin still has to
        # move or the site would advertise the previous release.
        with temp_repo(version="0.5.0",
                       changelog="# Changelog\n\n## 0.4.0\n"):
            with temp_site() as site:
                self.assertEqual(self.run_main(), 0)
                pin = json.loads(
                    (site / "app" / "data" / "auraterm_release.json")
                    .read_text(encoding="utf-8"))
                self.assertEqual(pin["latest_version"], "0.5.0")

    def test_a_missing_changelog_is_an_error(self):
        with temp_repo() as repo:
            (repo / "Changelog.md").unlink()
            with temp_site():
                self.assertEqual(self.run_main(), 1)

    def test_a_versionless_package_json_is_an_error(self):
        with temp_repo() as repo:
            (repo / "package.json").write_text("{}", encoding="utf-8")
            with temp_site():
                self.assertEqual(self.run_main(), 1)


if __name__ == "__main__":
    unittest.main()
