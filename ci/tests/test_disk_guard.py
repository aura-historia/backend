import importlib.util
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location('disk_guard', CI / 'guard-disk.py')
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class BoundedDiskGuard(unittest.TestCase):
    def test_low_space_signals_cancellation_and_retains_honest_path_sizes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            guard = GUARD.DiskGuard(root, root, root/'missing-profiles', root/'disk.jsonl', 100)
            with patch.object(GUARD.shutil, 'disk_usage', return_value=shutil._ntuple_diskusage(1000, 950, 50)), patch.object(
                    GUARD, 'observe', return_value='123\t/target'), patch.dict(os.environ, RUNNER_ENVIRONMENT='self-hosted'):
                self.assertEqual(guard.check(), signal.SIGTERM)
            record = json.loads((root/'disk.jsonl').read_text())
            self.assertEqual(record['free_bytes'], 50)
            self.assertIsNone(record['sizes_kib']['profiles'])
            self.assertNotIn('docker', record['sizes_kib'])

    def test_expensive_sizes_are_bounded_and_sampled_less_often_than_free_space(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            guard = GUARD.DiskGuard(root, root, root, root/'disk.jsonl', 1)
            with patch.object(GUARD.time, 'monotonic', side_effect=[0, 1, 21, 61]), patch.object(
                    GUARD, 'observe', return_value='12\t/path') as observe, patch.dict(os.environ, RUNNER_ENVIRONMENT='self-hosted'):
                for _ in range(4): self.assertIsNone(guard.check())
            self.assertEqual(observe.call_count, 4)  # Two paths at time0 and time61.
            self.assertEqual(len((root/'disk.jsonl').read_text().splitlines()), 3)
        with patch.object(GUARD.subprocess, 'run', side_effect=subprocess.TimeoutExpired('du', 5)):
            self.assertIsNone(GUARD.observe(['du'], CI))

    def test_real_command_status_is_preserved_and_initial_low_space_never_starts_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            base = [sys.executable, str(CI/'guard-disk.py'), '--target', temporary, '--profiles', temporary,
                    '--log', str(root/'disk.jsonl')]
            result = subprocess.run([*base, '--minimum-bytes', '1', '--', sys.executable, '-c', 'raise SystemExit(37)'], capture_output=True)
            self.assertEqual(result.returncode, 37)
            marker = root/'started'
            result = subprocess.run([*base, '--minimum-bytes', str(2**63), '--', sys.executable, '-c',
                                     'import pathlib,sys;pathlib.Path(sys.argv[1]).touch()', str(marker)], capture_output=True)
            self.assertEqual(result.returncode, 99)
            self.assertFalse(marker.exists())

    def test_low_space_during_child_execution_waits_for_owned_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            marker = root/'cleaned'
            child = 'import pathlib,signal,sys,time; signal.signal(signal.SIGTERM,lambda *_: (pathlib.Path(sys.argv[1]).touch(),sys.exit(0))); print("ready",flush=True); time.sleep(30)'
            calls = 0
            def check():
                nonlocal calls
                calls += 1
                return signal.SIGTERM if calls > 1 else None
            with self.assertRaises(SystemExit) as failure:
                GUARD.execute([sys.executable, '-c', child, str(marker)], root, os.environ, check)
            self.assertEqual(failure.exception.code, 128 + signal.SIGTERM)
            self.assertTrue(marker.exists())

    def test_sdk_reclamation_never_runs_locally_or_on_self_hosted_linux(self):
        for environment in ('', 'self-hosted'):
            with self.subTest(environment=environment), tempfile.TemporaryDirectory() as temporary:
                root = pathlib.Path(temporary)
                sudo = root/'sudo'
                sudo.write_text('#!/bin/sh\nexit 77\n')
                sudo.chmod(0o755)
                result = subprocess.run(['bash', str(CI/'prepare-runner-disk.sh')], env=dict(os.environ,
                    RUNNER_ENVIRONMENT=environment, RUNNER_OS='Linux', PATH=str(root)+os.pathsep+os.environ['PATH']), capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(b'reclamation skipped', result.stdout)


if __name__ == '__main__':
    unittest.main()
