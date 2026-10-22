import copy
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location('test_reports', CI / 'validate-test-reports.py')
REPORTS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPORTS)


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))


class SelectedExecutionReports(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.revision = 'a' * 40
        self.target = 'money::lib::money'
        self.manifest = {'packages': {'money': {'path': 'src/money', 'targets': [dict(
            id=self.target, lane='REMAINING', test=True, harness=True, doctest=True)]}}}
        self.selection = REPORTS.combined_selection(self.manifest, self.manifest, ['REMAINING'])
        self.rows = [dict(id='remaining', lanes='REMAINING', profile='none')]

    def discovery(self, directory, selection):
        write_json(directory / 'selection.json', selection)
        write_json(directory / 'nextest.json', {'test-count': 2, 'rust-suites': {'money': {
            'package-name': 'money', 'kind': 'lib', 'binary-name': 'money',
            'testcases': {'active': {'ignored': False}, 'probe': {'ignored': True}}}}})
        (directory / 'money.list.txt').write_text('active: test\nprobe: test\n')
        (directory / 'compile.log').touch()

    def benchmark(self, prime=None):
        directory = self.root / 'lane-measurement-one-remaining-1-attempt-1'
        directory.mkdir()
        (directory / 'disk-preparation.log').write_text('observed')
        for cache, revision in [('COLD_TARGET', prime or self.revision), ('WARM_TARGET', self.revision)]:
            output = directory / cache
            self.discovery(output / 'compile-discovery', self.selection)
            self.discovery(output / 'discovery', self.selection)
            write_json(output / 'benchmark.json', dict(status='PASSED', revision=revision, dirty=False,
                cache=cache, coverage_mode='none', coverage=None, lanes=['REMAINING'], profile='none',
                stages=[dict(name=name, status='PASSED', exit_code=0) for name in ['COMPILE_LINK', 'TESTS_AND_FIXTURES']]))
            (output / 'tests_and_fixtures.log').write_text('actual tests and doctests')
            (output / 'disk.jsonl').write_text('observed')
        return directory

    def validate_benchmark(self, prime=None):
        REPORTS.validate_reports(self.root, self.rows, self.manifest, self.revision, 1,
                                 layout='one', repetition=1, prime=prime)

    def test_complete_none_pair_accepts_real_case_inventory_without_any_coverage_files(self):
        self.benchmark()
        self.validate_benchmark()
        self.assertEqual(list(self.root.rglob('*.lcov')), [])

    def test_small_change_requires_real_base_then_head_sources(self):
        self.benchmark(prime='b' * 40)
        self.validate_benchmark(prime='b' * 40)
        with self.assertRaisesRegex(ValueError, 'measurement'):
            self.validate_benchmark()

    def test_missing_extra_and_duplicate_selected_owners_fail(self):
        directory = self.benchmark()
        with self.assertRaisesRegex(ValueError, 'exact selected'):
            REPORTS.validate_reports(self.root, self.rows + [dict(id='api')], self.manifest, self.revision, 1,
                                     layout='one', repetition=1)
        with self.assertRaisesRegex(ValueError, 'distinct'):
            REPORTS.validate_reports(self.root, self.rows * 2, self.manifest, self.revision, 1)
        (self.root / 'unselected').mkdir()
        with self.assertRaisesRegex(ValueError, 'exact selected'):
            self.validate_benchmark()
        (self.root / 'unselected').rmdir()
        directory.rename(self.root / 'wrong-attempt')
        with self.assertRaisesRegex(ValueError, 'exact selected'):
            self.validate_benchmark()

    def test_failed_incomplete_instrumented_or_wrong_source_reports_fail(self):
        output = self.benchmark() / 'WARM_TARGET'
        path = output / 'benchmark.json'
        original = REPORTS.read_json(path)
        changes = [dict(status='FAILED'), dict(revision='b'*40), dict(coverage_mode='llvm'), dict(coverage={}),
                   dict(stages=original['stages'][:1]), dict(dirty=True)]
        failed = copy.deepcopy(original['stages'])
        failed[-1].update(status='FAILED', exit_code=1)
        changes.append(dict(stages=failed))
        for change in changes:
            with self.subTest(change=change), self.assertRaises(ValueError):
                write_json(path, dict(original, **change))
                self.validate_benchmark()

    def test_missing_targets_cases_ignored_flags_or_selection_are_rejected(self):
        directory = self.benchmark() / 'WARM_TARGET/discovery'
        path = directory / 'nextest.json'
        original = REPORTS.read_json(path)
        for data in ({'test-count': 0, 'rust-suites': {}}, dict(original, **{'test-count': 3})):
            write_json(path, data)
            with self.assertRaises(ValueError): self.validate_benchmark()
        data = copy.deepcopy(original)
        data['rust-suites']['money']['testcases']['probe']['ignored'] = 'true'
        write_json(path, data)
        with self.assertRaises(ValueError): self.validate_benchmark()
        data = copy.deepcopy(original)
        data['rust-suites']['money']['testcases']['probe']['ignored'] = False
        write_json(path, data)
        with self.assertRaisesRegex(ValueError, 'Compile/execution'): self.validate_benchmark()
        write_json(path, original)
        (directory / 'money.list.txt').write_text('active: test\n')
        with self.assertRaisesRegex(ValueError, 'case list'): self.validate_benchmark()
        (directory / 'money.list.txt').write_text('active: test\nprobe: test\n')
        write_json(directory / 'selection.json', dict(self.selection, doctest_targets=[]))
        with self.assertRaisesRegex(ValueError, 'packages/targets'): self.validate_benchmark()

    def test_legacy_requires_exact_selected_package_inventory_and_source(self):
        row = dict(id='legacy-money', package='money')
        directory = self.root / 'tests-legacy-money-execution-attempt-1'
        selection = REPORTS.legacy_package_selection(self.manifest, self.manifest, 'money')
        self.discovery(directory / 'discovery', selection)
        write_json(directory / 'job.json', dict(checkout_sha=self.revision, coverage_mode='none'))
        for name in ('tests.log', 'disk.jsonl', 'disk-preparation.log', 'fixture-images.json'):
            (directory / name).write_text('observed')
        REPORTS.validate_reports(self.root, [row], self.manifest, self.revision, 1)
        write_json(directory / 'job.json', dict(checkout_sha='b'*40, coverage_mode='none'))
        with self.assertRaisesRegex(ValueError, 'legacy source'):
            REPORTS.validate_reports(self.root, [row], self.manifest, self.revision, 1)


if __name__ == '__main__':
    unittest.main()
