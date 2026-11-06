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
    rust = code if all(path.endswith(".rs") for path in paths) else ()
    return {job: decision for job, (decision, _) in changes.select(paths, cargo, code, rust).items()}


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
        self.assertEqual(chosen.pop("rust-windows"), "base+server+live+crates")
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
        self.assertEqual(decisions(server, code=["    std::fs::write(&path, data)?;"])["rust-windows"],
                         "base+server+live")
        self.assertEqual(decisions(server, code=['#[cfg(windows)]'])["rust-windows"], "base+server+live+crates")
        self.assertEqual(decisions(["crates/client/Cargo.toml"], cargo=['+[target.\'cfg(windows)\'.dependencies]'])
                         ["rust-windows"], "run")
        for path in ["tools/windows/lock.py", "packaging/windows.py", "crates/media/build.rs",
                     "crates/client-core/src/paths.rs", "crates/server/src/storage/free.rs", "Cargo.lock",
                     "tools/with_native_livekit.py", "tools/with_native_ffmpeg.py", "tools/native_downloads.py"]:
            with self.subTest(path=path):
                decision = decisions([path])["rust-windows"]
                self.assertTrue(decision == "run" or decision.startswith("base"), decision)

    def test_windows_builds_the_client_in_release_form_only_for_a_change_of_the_client_gpui_or_cargo(self):
        self.assertEqual(decisions(["crates/client/src/panics.rs"], code=["#[cfg(windows)]"])["rust-windows"],
                         "base+release+live+crates")
        for path in ["vendor/gpui-pre-0.3.8/src/window.rs", "Cargo.lock", "crates/media/Cargo.toml",
                     ".cargo/config.toml", "rust-toolchain.toml", "tools/ci/rust-other-systems.sh"]:
            with self.subTest(path=path):
                self.assertEqual(decisions([path], code=["#[cfg(windows)]"])["rust-windows"], "run")
        for path in ["tools/windows/lock.py", "crates/server/src/storage/free.rs", "packaging/windows.py"]:
            with self.subTest(path=path):
                self.assertNotIn("release", decisions([path])["rust-windows"])

    def test_the_server_tests_the_calls_and_the_lint_of_each_crate_run_on_windows_for_their_code(self):
        def windows(paths, code=("    std::fs::write(&path, data)?;",)):
            return decisions(paths, code=list(code))["rust-windows"]
        self.assertEqual(windows(["crates/server/src/routes/messages.rs"]), "base+server+live")
        self.assertEqual(windows(["crates/setup/src/apply.rs"]), "base+server")
        self.assertEqual(windows(["crates/media/src/audio/devices.rs"]), "base+live")
        self.assertEqual(windows(["crates/client-core/src/transfers.rs"]), "base+live")
        self.assertEqual(windows(["crates/client/src/state/account.rs"]), "base+release+live")
        self.assertEqual(windows(["crates/icon/src/main.rs"]), "base")
        self.assertEqual(windows(["packaging/windows.py"], code=()), "base")
        self.assertEqual(windows(["tools/windows/lock.py"], code=()), "base+server+live")
        self.assertEqual(windows(["tools/with_native_ffmpeg.py"], code=()), "base+server")
        self.assertEqual(windows(["tools/with_native_livekit.py"], code=()), "base+live")
        # A Windows source file, and a line of Rust with a cfg of the system or of a feature.
        self.assertEqual(windows(["crates/media/src/audio/windows_devices.rs"], code=()), "base+live+crates")
        self.assertEqual(windows(["crates/icon/src/main.rs"], code=['#[cfg(feature = "png")]', "std::fs::write(a, b)"]),
                         "base+crates")
        self.assertEqual(windows(["crates/icon/src/main.rs"], code=["#[cfg(not(unix))]"]), "base+crates")
        # Each part runs for tools/ci and the Cargo files, so they run the whole job.
        self.assertEqual(windows(["tools/ci/lanes.py"], code=()), "run")
        self.assertEqual(changes.PARTS, ["release", "server", "live", "crates"])
        reason = changes.select(["crates/setup/src/apply.rs"], (), ["std::fs::write(a, b)"])["rust-windows"][1]
        self.assertIn("parts server: crates/setup/src/apply.rs", reason)

    def test_an_edit_inside_existing_windows_code_runs_windows_and_the_lint_of_each_crate(self):
        # The line names no cfg: the file holds code under one.
        path = "crates/media/src/share/mod.rs"
        line = ["        let size = std::fs::metadata(&path)?.len();"]
        self.assertEqual(changes.select([path], (), line, line)["rust-windows"][0], "base+live")
        chosen = changes.select([path], (), line, line, [path])["rust-windows"]
        self.assertEqual(chosen[0], "base+live+crates")
        self.assertIn(f"code under a cfg of the system in {path}", chosen[1])
        # Also when nothing else of the change can differ on Windows.
        quiet = ["        let total = first + second;"]
        self.assertEqual(changes.select([path], (), quiet, quiet)["rust-windows"][0], "skip")
        self.assertEqual(changes.select([path], (), quiet, quiet, [path])["rust-windows"][0], "base+live+crates")

    def test_the_files_with_code_under_a_cfg_of_the_system_come_from_both_ends_of_the_change(self):
        with tempfile.TemporaryDirectory() as directory:
            git = ["git", "-C", directory]
            identity = dict(os.environ, GIT_AUTHOR_NAME="Example", GIT_AUTHOR_EMAIL="ci@example.com",
                            GIT_COMMITTER_NAME="Example", GIT_COMMITTER_EMAIL="ci@example.com")
            subprocess.run([*git, "init", "--quiet"], check=True)
            source = Path(directory) / "crates/media/src/share"
            source.mkdir(parents=True)
            windows = source / "mod.rs"
            windows.write_text('#[cfg(windows)]\nfn open() {\n    let a = 1;\n}\n')
            plain = source / "plain.rs"
            plain.write_text("fn a() {}\n")
            gone = source / "gone.rs"
            gone.write_text('#[cfg(target_os = "windows")]\nfn b() {}\n')
            # Conditions written over several lines, and an edit inside them that names none.
            spread = source / "spread.rs"
            spread.write_text('#[cfg(all(\n    target_os = "windows",\n    feature = "x"\n))]\nfn c() {\n    let a = 1;\n}\n')
            listed = source / "listed.rs"
            listed.write_text("#[cfg(any(\n    windows,\n    unix\n))]\nfn d() {\n    let a = 1;\n}\n")
            alone = source / "alone.rs"
            alone.write_text("#[cfg(\n    windows\n)]\nfn e() {\n    let a = 1;\n}\n")
            neither = source / "neither.rs"
            neither.write_text("// windows of the call\nfn f() {\n    let unix_time = 1;\n}\n")
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "one"], check=True, env=identity)
            windows.write_text('#[cfg(windows)]\nfn open() {\n    let a = 2;\n}\n')
            for path in (spread, listed, alone, neither):
                path.write_text(path.read_text().replace("= 1;", "= 2;"))
            plain.write_text("fn a() { let b = 1; }\n")
            gone.unlink()
            subprocess.run([*git, "add", "-A"], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "two"], check=True, env=identity)
            here = os.getcwd()
            os.chdir(directory)
            try:
                names = ["mod", "plain", "gone", "spread", "listed", "alone", "neither"]
                found = changes.system_files("HEAD~1", "HEAD", [f"crates/media/src/share/{name}.rs" for name in names])
                chosen = changes.from_git("HEAD~1", "HEAD")
            finally:
                os.chdir(here)
            self.assertEqual(found, [f"crates/media/src/share/{name}.rs"
                                     for name in ["alone", "gone", "listed", "mod", "spread"]])
            self.assertEqual(chosen["rust-windows"][0], "base+live+crates")

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
            self.assertEqual(chosen["rust-windows"][0], "base+server+live")
            self.assertIn("std::process::", chosen["rust-windows"][1])


if __name__ == "__main__":
    unittest.main()
