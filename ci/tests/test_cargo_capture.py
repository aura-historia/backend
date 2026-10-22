import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest

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
                        '{"reason":"compiler-artifact"}\n{"reason":"build-finished","success":false}\n'):
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    record_artifacts(raw, root / "phase.jsonl", root / "objects")

    def test_failed_child_retains_stdout_and_original_status_without_claiming_capture_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            destination = root / "phase.jsonl"
            with contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(SystemExit) as failure:
                execute_cargo([sys.executable, '-c', "print('partial build'); raise SystemExit(37)"], root,
                              dict(os.environ, AURA_CI_COVERAGE_DIR=str(root)), destination)
            self.assertEqual(failure.exception.code, 37)
            self.assertEqual(destination.with_suffix('.stdout.log').read_text(), 'partial build\n')
            self.assertIn('partial build\n', output.getvalue())
            self.assertFalse(destination.exists())


if __name__ == '__main__':
    unittest.main()
