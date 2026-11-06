#!/usr/bin/env python3
"""Tests of tools/ci/changes.py, the selection of the CI jobs that a change needs."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import changes  # noqa: E402

LINUX = ["rust-linux", "licences", "transcription-worker", "layout-page"]


def decisions(paths, cargo=(), code=()):
    return {job: decision for job, (decision, _) in changes.select(paths, cargo, code).items()}


class Select(unittest.TestCase):
    def test_a_change_of_documents_only_needs_no_job(self):
        self.assertEqual(set(decisions(["README.md", "docs/adr/0905-continuous-integration.md",
                                        "crates/server/README.md"]).values()), {"skip"})

    def test_the_api_documents_are_compared_with_the_code(self):
        self.assertEqual(decisions(["docs/api/openapi.json"])["rust-linux"], "run")

    def test_a_python_unit_test_needs_only_the_python_tests_of_the_rust_jobs(self):
        chosen = decisions(["tools/test_test_services.py"])
        self.assertEqual((chosen["rust-linux"], chosen["rust-macos"]), ("python", "python"))
        self.assertEqual(chosen["rust-windows"], "skip")
        self.assertEqual({chosen[job] for job in ("licences", "deployment", "layout-page")}, {"skip"})
        self.assertEqual(decisions(["tools/windows/test_lock.py"])["rust-windows"], "python")

    def test_tools_ci_and_cargo_files_run_every_job(self):
        for path in ["tools/ci/test_budget.py", "tools/ci/run-jobs.sh", "Cargo.lock", "Cargo.toml",
                     "crates/media/Cargo.toml", "rust-toolchain.toml"]:
            with self.subTest(path=path):
                self.assertEqual(set(decisions([path]).values()), {"run"})
        # A copy other than those of gpui: the Windows job without its release build.
        chosen = decisions(["vendor/cpal/src/lib.rs"])
        self.assertEqual(chosen.pop("rust-windows"), "debug")
        self.assertEqual(set(chosen.values()), {"run"})

    def test_unknown_paths_run_every_job_but_windows_by_its_rules(self):
        for path in ["tools/linux/Dockerfile", "deploy/livekit-common.yaml", "a/new/file.txt"]:
            with self.subTest(path=path):
                chosen = decisions([path])
                self.assertEqual(chosen.pop("rust-windows"), "skip")
                self.assertEqual(set(chosen.values()), {"run"})

    def test_the_parts_of_the_repository_with_jobs_of_their_own(self):
        self.assertEqual({job for job, d in decisions(["recording-layout/src/main.ts"]).items() if d != "skip"},
                         {"layout-page", "deployment"})
        self.assertEqual({job for job, d in decisions(["workers/transcription/worker.py"]).items() if d != "skip"},
                         {"transcription-worker", "deployment"})
        self.assertEqual({job for job, d in decisions(["deploy/one-host.yaml"]).items() if d != "skip"},
                         {"deployment"})

    def test_a_code_change_runs_the_job_that_needs_most(self):
        chosen = decisions(["tools/test_test_services.py", "crates/server/src/routes/messages.rs"])
        self.assertEqual({chosen[job] for job in LINUX + ["rust-macos", "deployment"]}, {"run"})

    def test_windows_runs_only_for_a_change_that_can_behave_differently_there(self):
        server = ["crates/server/src/routes/messages.rs"]
        self.assertEqual(decisions(server)["rust-windows"], "skip")
        self.assertEqual(decisions(server, code=["    std::fs::write(&path, data)?;"])["rust-windows"], "debug")
        self.assertEqual(decisions(server, code=['#[cfg(windows)]'])["rust-windows"], "debug")
        self.assertEqual(decisions(["crates/client/Cargo.toml"], cargo=['+[target.\'cfg(windows)\'.dependencies]'])
                         ["rust-windows"], "run")
        for path in ["tools/windows/lock.py", "packaging/windows.py", "crates/media/build.rs",
                     "crates/client-core/src/paths.rs", "crates/server/src/storage/free.rs", "Cargo.lock",
                     "tools/with_native_livekit.py", "tools/with_native_ffmpeg.py", "tools/native_downloads.py"]:
            with self.subTest(path=path):
                self.assertIn(decisions([path])["rust-windows"], ("run", "debug"))

    def test_windows_builds_the_client_in_release_form_only_for_a_change_of_the_client_gpui_or_cargo(self):
        for path in ["crates/client/src/panics.rs", "vendor/gpui-pre-0.3.8/src/window.rs", "Cargo.lock",
                     "crates/media/Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml",
                     "tools/ci/rust-other-systems.sh"]:
            with self.subTest(path=path):
                self.assertEqual(decisions([path], code=["#[cfg(windows)]"])["rust-windows"], "run")
        for path in ["tools/windows/lock.py", "crates/server/src/storage/free.rs", "packaging/windows.py"]:
            with self.subTest(path=path):
                self.assertEqual(decisions([path])["rust-windows"], "debug")
        self.assertEqual(decisions(["crates/server/src/routes/messages.rs"],
                                   code=["    std::fs::write(&path, data)?;"])["rust-windows"], "debug")

    def test_the_lines_and_paths_of_a_commit_range_come_from_git(self):
        with tempfile.TemporaryDirectory() as directory:
            git = ["git", "-C", directory]
            identity = dict(os.environ, GIT_AUTHOR_NAME="Example", GIT_AUTHOR_EMAIL="ci@example.com",
                            GIT_COMMITTER_NAME="Example", GIT_COMMITTER_EMAIL="ci@example.com")
            subprocess.run([*git, "init", "--quiet"], check=True)
            (Path(directory) / "crates/server/src").mkdir(parents=True)
            source = Path(directory) / "crates/server/src/lib.rs"
            source.write_text("fn a() {}\n")
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "one"], check=True, env=identity)
            source.write_text("fn a() {}\nfn b() { std::process::exit(0) }\n")
            (Path(directory) / "notes.md").write_text("text\n")
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "two"], check=True, env=identity)
            here = os.getcwd()
            os.chdir(directory)
            try:
                chosen = changes.from_git("HEAD~1", "HEAD")
            finally:
                os.chdir(here)
            self.assertEqual(chosen["rust-linux"][0], "run")
            self.assertEqual(chosen["rust-windows"][0], "debug")
            self.assertIn("std::process::", chosen["rust-windows"][1])


if __name__ == "__main__":
    unittest.main()
