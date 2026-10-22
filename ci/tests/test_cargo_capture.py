import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from ci_inventory.coverage import execute_cargo, record_artifacts


class CargoCoverageCapture(unittest.TestCase):
    def test_mutable_executable_is_retained_before_a_later_build_replaces_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            binary = root / "supervisor"
            binary.write_bytes(b"original executable")
            binary.chmod(0o755)
            messages = [{"reason": "compiler-artifact", "executable": str(binary), "profile": {"test": False}},
                        {"reason": "build-finished", "success": True}]
            raw = "".join(json.dumps(item) + "\n" for item in messages) + 'test output\n{"reason":"test output"}\n'
            destination = root / "phase.jsonl"
            record_artifacts(raw, destination, root / "objects")
            captured = json.loads(destination.read_text().splitlines()[0])
            retained = pathlib.Path(captured["executable"])
            record_artifacts(raw, root / 'repeat.jsonl', root / 'objects')
            repeated = json.loads((root / 'repeat.jsonl').read_text().splitlines()[0])
            self.assertEqual(repeated['executable'], str(retained))
            self.assertEqual(repeated['coverage_sha256'], captured['coverage_sha256'])
            binary.write_bytes(b"replacement executable")
            record_artifacts(raw, root / 'replacement.jsonl', root / 'objects')
            replacement = json.loads((root / 'replacement.jsonl').read_text().splitlines()[0])
            self.assertNotEqual(replacement['executable'], str(retained))
            self.assertEqual(retained.read_bytes(), b"original executable")
            self.assertTrue(os.access(retained, os.X_OK))
            self.assertEqual(captured["original_executable"], str(binary))
            self.assertEqual(len(destination.read_text().splitlines()), 2)

    def test_success_requires_a_complete_cargo_build_even_when_the_child_exits_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            for raw in ('', '{}\n', '{"reason":"compiler-artifact"}\n',
                        '{"reason":"build-finished","success":true}\n',
                        '{"reason":"compiler-artifact"}\n{"reason":"build-finished","success":false}\n',
                        b'{"reason":"compiler-artifact","package_id":"\xff"}\n{"reason":"build-finished","success":true}\n'):
                destination = root / "phase.jsonl"
                destination.write_text('stale successful capture')
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    record_artifacts(raw, destination, root / "objects")
                self.assertFalse(destination.exists())

    def test_arbitrary_test_stdout_preserves_raw_bytes_after_the_cargo_prefix(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            destination = root / "phase.jsonl"
            prefix = b'{"reason":"compiler-artifact"}\n{"reason":"build-finished","success":true}\n'
            raw = prefix + b'test stdout: \xff\n'
            child = f"import sys; sys.stdout.buffer.write({raw!r})"
            with contextlib.redirect_stdout(io.StringIO()) as output:
                execute_cargo([sys.executable, '-c', child], root,
                              dict(os.environ, AURA_CI_COVERAGE_DIR=str(root)), destination)
            self.assertEqual(destination.with_suffix('.stdout.log').read_bytes(), raw)
            self.assertEqual(destination.read_text(), ''.join(json.dumps(json.loads(line)) + '\n' for line in prefix.splitlines()))
            self.assertIn('test stdout: \ufffd\n', output.getvalue())

    def test_replay_failure_cannot_replace_the_original_child_exit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            destination = root / "phase.jsonl"
            destination.write_text('stale successful capture')
            with patch('ci_inventory.coverage.shutil.copyfileobj', side_effect=OSError('replay denied')), \
                    contextlib.redirect_stderr(io.StringIO()) as errors, self.assertRaises(SystemExit) as failure:
                execute_cargo([sys.executable, '-c', "print('partial build'); raise SystemExit(37)"], root,
                              dict(os.environ, AURA_CI_COVERAGE_DIR=str(root)), destination)
            self.assertEqual(failure.exception.code, 37)
            self.assertIn('could not replay Cargo stdout: replay denied', errors.getvalue())
            self.assertEqual(destination.with_suffix('.stdout.log').read_bytes(), b'partial build\n')
            self.assertFalse(destination.exists())

    def test_failed_child_retains_stdout_and_original_status_without_claiming_capture_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            destination = root / "phase.jsonl"
            destination.write_text('stale successful capture')
            with contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(SystemExit) as failure:
                execute_cargo([sys.executable, '-c', "import sys; sys.stdout.buffer.write(b'partial build\\xff\\n'); raise SystemExit(37)"], root,
                              dict(os.environ, AURA_CI_COVERAGE_DIR=str(root)), destination)
            self.assertEqual(failure.exception.code, 37)
            self.assertEqual(destination.with_suffix('.stdout.log').read_bytes(), b'partial build\xff\n')
            self.assertIn('partial build\ufffd\n', output.getvalue())
            self.assertFalse(destination.exists())


if __name__ == '__main__':
    unittest.main()
