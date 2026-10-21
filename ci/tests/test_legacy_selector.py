import importlib.util
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location('legacy_selector', CI / 'replay-legacy-selector.py')
REPLAY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPLAY)


def package(name, path):
    return dict(path=path, targets=[dict(id=name+'::lib::'+name, lane='REMAINING')])


class LegacySchedulingPlan(unittest.TestCase):
    def setUp(self):
        self.manifest = dict(packages={'root': package('root', '.'), 'money': package('money', 'src/money'),
            'macros': package('macros', 'src/test-api/src/test-api-macros')})

    def test_full_control_adds_only_explicit_original_omissions(self):
        result = REPLAY.plan(self.manifest, self.manifest, {'integration_test': ['src/money']}, True)
        self.assertEqual(result['packages'], ['macros', 'money', 'root'])
        self.assertEqual(result['original_selection'], {'integration_test': ['src/money']})
        self.assertEqual(result['parity_added_paths'], ['.', 'src/test-api/src/test-api-macros'])

    def test_small_control_preserves_actual_affected_selection(self):
        result = REPLAY.plan(self.manifest, self.manifest, {'integration_test': ['src/money']}, False)
        self.assertEqual(result['matrix'], [{'package': 'money', 'id': 'legacy-money'}])
        self.assertEqual(result['parity_added_paths'], [])

    def test_missing_duplicate_unknown_and_empty_selections_fail_closed(self):
        for paths, full in [([], True), ([], False), (['src/money', 'src/money'], False), (['foreign'], False)]:
            with self.subTest(paths=paths, full=full), self.assertRaises(ValueError):
                REPLAY.plan(self.manifest, self.manifest, {'integration_test': paths}, full)

    def test_current_manifest_drift_is_not_automatically_accepted(self):
        with self.assertRaisesRegex(ValueError, 'contract changed'):
            REPLAY.plan(self.manifest, {'packages': {}}, {'integration_test': ['src/money']}, True)

    def test_original_selector_build_uses_the_explicit_cached_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            target = root / 'cache'
            lock = 'version = 4\n[[package]]\nname = "ci-determinator"\nversion = "0.1.0"\n'
            with patch.object(REPLAY.subprocess, 'check_output', return_value=b'original'), patch.object(
                    REPLAY, 'command', side_effect=['[workspace.dependencies]\nserde_json = "1.0.150"', lock, '{}']), patch.object(
                    REPLAY.subprocess, 'run') as run:
                binary = REPLAY.prepare_selector(root, root / 'output', target)
            self.assertEqual(binary, target / 'release/ci-determinator')
            self.assertEqual(run.call_args.args[0][-2:], ['--target-dir', str(target)])


if __name__ == '__main__':
    unittest.main()
