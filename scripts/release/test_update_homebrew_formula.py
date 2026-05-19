#!/usr/bin/env python3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


UPDATER = Path(__file__).with_name("update-homebrew-formula.py")
ARM_SHA = "a" * 64
X86_SHA = "b" * 64


class UpdateHomebrewFormulaTest(unittest.TestCase):
    def run_updater(self, formula: Path, *extra_args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(UPDATER), str(formula), *extra_args],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def test_rewrites_source_formula_to_binary_formula(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            formula = Path(tmpdir) / "lucarned.rb"
            formula.write_text(
                "class Lucarned < Formula\n"
                "  desc \"old\"\n"
                "  url \"https://example.invalid/source.tar.gz\"\n"
                "end\n",
                encoding="utf-8",
            )

            result = self.run_updater(
                formula,
                "--version",
                "0.2.3",
                "--arm64-sha",
                ARM_SHA,
                "--x86-64-sha",
                X86_SHA,
            )

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            output = formula.read_text(encoding="utf-8")
            self.assertIn("class Lucarned < Formula", output)
            self.assertIn('desc "Local lucarne daemon for channel adapters and agent sessions"', output)
            self.assertIn('homepage "https://github.com/tuchg/Lucarne"', output)
            self.assertIn('version "0.2.3"', output)
            self.assertIn('license "MIT"', output)
            self.assertIn("depends_on :macos", output)
            self.assertIn("on_arm do", output)
            self.assertIn(
                'url "https://github.com/tuchg/Lucarne/releases/download/v0.2.3/lucarned-v0.2.3-aarch64-apple-darwin.tar.gz"',
                output,
            )
            self.assertIn(f'sha256 "{ARM_SHA}"', output)
            self.assertIn("on_intel do", output)
            self.assertIn(
                'url "https://github.com/tuchg/Lucarne/releases/download/v0.2.3/lucarned-v0.2.3-x86_64-apple-darwin.tar.gz"',
                output,
            )
            self.assertIn(f'sha256 "{X86_SHA}"', output)
            self.assertIn("head do", output)
            self.assertIn('depends_on "pkg-config" => :build', output)
            self.assertIn('depends_on "rust" => :build', output)
            self.assertIn('depends_on "openssl@3"', output)
            self.assertIn("if build.head?", output)
            self.assertIn('bin.install "bin/lucarned"', output)
            self.assertIn("brew services start lucarned", output)
            self.assertIn('assert_match "enabled: false", config.read', output)

    def test_rejects_bad_arm64_sha(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            formula = Path(tmpdir) / "lucarned.rb"
            formula.write_text("class Lucarned < Formula\nend\n", encoding="utf-8")

            result = self.run_updater(
                formula,
                "--version",
                "0.2.3",
                "--arm64-sha",
                "not-a-sha",
                "--x86-64-sha",
                X86_SHA,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--arm64-sha must be 64 lowercase hex characters", result.stderr)

    def test_rejects_bad_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            formula = Path(tmpdir) / "lucarned.rb"
            formula.write_text("class Lucarned < Formula\nend\n", encoding="utf-8")

            result = self.run_updater(
                formula,
                "--version",
                "release-0.2.3",
                "--arm64-sha",
                ARM_SHA,
                "--x86-64-sha",
                X86_SHA,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--version must look like 0.1.0", result.stderr)


if __name__ == "__main__":
    unittest.main()
