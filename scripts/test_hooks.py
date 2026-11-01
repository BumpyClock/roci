import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[1]
ZERO = '0' * 40


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='roci hook ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'repo with spaces'
        self.root.mkdir()
        self.log = Path(self.temp.name) / 'commands.log'
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1', CHECK_LOG=str(self.log))
        self.git('init', '--initial-branch=main')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'user.name', 'Hook Fixture')
        for name in ['.githooks/pre-push', 'scripts/install-hooks.sh']:
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(SOURCE / name, destination)
            destination.chmod(0o755)
        (self.root / 'source.rs').write_text('fn fixture() {}\n')
        self.base = self.commit()
        stubs = Path(self.temp.name) / 'stubs'
        stubs.mkdir()
        rustup = stubs / 'rustup'
        rustup.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$CHECK_LOG"
if [ "${MISSING_TOOLCHAIN:-0}" = 1 ]; then exit 1; fi
case "$*" in
  *--version) exit 0 ;;
  'run 1.96.1 cargo fmt '*) exit "${FMT_EXIT:-0}" ;;
  'run 1.96.1 cargo check '*) exit "${CHECK_EXIT:-0}" ;;
  'run 1.96.1 cargo clippy '*) exit "${CLIPPY_EXIT:-0}" ;;
  *) exit 99 ;;
esac
''')
        rustup.chmod(0o755)
        self.env['PATH'] = str(stubs) + os.pathsep + self.env['PATH']

    def run_command(self, args, input=None):
        return subprocess.run(args, input=input, cwd=self.root, env=self.env, text=True, capture_output=True)

    def git(self, *args):
        result = self.run_command(['git', *args])
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-m', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def push(self, remote=None, input=None):
        if input is None:
            head = self.git('rev-parse', 'HEAD')
            input = f'refs/heads/main {head} refs/heads/main {remote or self.base}\n'
        return self.run_command(['.githooks/pre-push', 'origin', 'unused'], input=input)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_all_uncertain_ranges_run_complete_graph(self):
        head = self.git('rev-parse', 'HEAD')
        for data in ['', f'main {head} main {ZERO}\n', f'main {head} main {head}\n', f'main {head} main {"1" * 40}\n', f'main {head} main {self.base}\nnew {head} new {ZERO}\n']:
            with self.subTest(input=data):
                if self.log.exists():
                    self.log.unlink()
                result = self.push(input=data)
                self.assertEqual(result.returncode, 0, result.stderr)
                commands = [line for line in self.calls() if '--version' not in line]
                self.assertEqual(commands, [
                    'run 1.96.1 cargo fmt --all -- --check',
                    'run 1.96.1 cargo check --workspace --locked --offline',
                    'run 1.96.1 cargo clippy --workspace --all-features --locked --offline',
                ])

    def test_deletion_only_needs_no_toolchain(self):
        self.env['MISSING_TOOLCHAIN'] = '1'
        result = self.push(input=f'delete {ZERO} main {self.base}\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [])

    def test_renamed_spaced_source_and_deletion_check_whole_workspace(self):
        (self.root / 'source.rs').rename(self.root / 'renamed source.rs')
        self.commit()
        self.assertEqual(self.push().returncode, 0)
        self.assertIn('run 1.96.1 cargo check --workspace --locked --offline', self.calls())
        (self.root / 'renamed source.rs').unlink()
        self.commit()
        self.assertEqual(self.push().returncode, 0)

    def test_dirty_and_non_head_pushes_fail_before_tools(self):
        (self.root / 'untracked.rs').write_text('dirty')
        self.assertNotEqual(self.push().returncode, 0)
        self.assertEqual(self.calls(), [])
        self.commit()
        self.assertNotEqual(self.push(input=f'main {self.base} main {ZERO}\n').returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_git_status_failure_stops_before_rust_checks(self):
        (self.root / '.git/index').write_bytes(b'broken index')
        result = self.push()
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [])

    def test_malformed_stdin_fails(self):
        self.assertNotEqual(self.push(input='bad ref\n').returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_missing_tools_fail_without_installing(self):
        self.env['MISSING_TOOLCHAIN'] = '1'
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('prepare tools explicitly', result.stderr)
        self.assertEqual(self.calls(), ['run 1.96.1 cargo --version'])

    def test_each_check_failure_propagates(self):
        for variable in ['FMT_EXIT', 'CHECK_EXIT', 'CLIPPY_EXIT']:
            with self.subTest(stage=variable):
                self.env[variable] = '23'
                result = self.push()
                self.assertEqual(result.returncode, 23, result.stderr)
                del self.env[variable]

    def test_installer_is_explicit_and_idempotent(self):
        self.assertNotEqual(self.run_command(['git', 'config', '--get', 'core.hooksPath']).returncode, 0)
        for _ in range(2):
            self.assertEqual(self.run_command(['scripts/install-hooks.sh']).returncode, 0)
        self.assertEqual(self.git('config', '--local', '--get', 'core.hooksPath'), '.githooks')
        self.assertEqual(self.calls(), [])

    def test_installer_refuses_custom_config(self):
        self.git('config', '--local', 'core.hooksPath', 'existing hooks')
        self.assertNotEqual(self.run_command(['scripts/install-hooks.sh']).returncode, 0)
        self.assertEqual(self.git('config', '--get', 'core.hooksPath'), 'existing hooks')

    def test_installer_refuses_explicit_empty_config(self):
        self.git('config', '--local', 'core.hooksPath', '')
        result = self.run_command(['scripts/install-hooks.sh'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Refusing', result.stderr)
        self.assertEqual(self.git('config', '--get', 'core.hooksPath'), '')
        self.assertEqual(self.calls(), [])

    def test_installer_preserves_active_hooks(self):
        hook = self.root / '.git/hooks/pre-push'
        hook.write_text('#!/bin/sh\nexit 17\n')
        hook.chmod(0o755)
        self.assertNotEqual(self.run_command(['scripts/install-hooks.sh']).returncode, 0)
        self.assertIn('exit 17', hook.read_text())


if __name__ == '__main__':
    unittest.main()
