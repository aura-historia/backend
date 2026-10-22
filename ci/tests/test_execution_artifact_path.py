import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest

CI = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('artifact_path', CI / 'execution-artifact-path.py')
ARTIFACTS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ARTIFACTS)


def metadata(*names):
    return [dict(name=name, expired=False) for name in names]


class ExactExecutionArtifactLayout(unittest.TestCase):
    def test_single_legacy_artifact_keeps_its_verified_attempt_scoped_name(self):
        name = 'tests-legacy-money-execution-attempt-2'
        self.assertEqual(ARTIFACTS.execution_artifact_path(metadata(name), [dict(id='legacy-money')], 2),
                         'execution-artifacts/' + name)

    def test_one_layout_keeps_its_verified_layout_repetition_and_attempt(self):
        name = 'lane-measurement-one-all-3-attempt-2'
        self.assertEqual(ARTIFACTS.execution_artifact_path(metadata(name), [dict(id='all')], 2,
                         layout='one', repetition=3), 'execution-artifacts/' + name)

    def test_multiple_artifacts_keep_the_actions_named_subdirectories(self):
        for layout, rows, names in [
                (None, [dict(id='legacy-money'), dict(id='legacy-test-api')],
                 ['tests-legacy-money-execution-attempt-2', 'tests-legacy-test-api-execution-attempt-2']),
                ('four', [dict(id='api'), dict(id='remaining')],
                 ['lane-measurement-four-api-3-attempt-2', 'lane-measurement-four-remaining-3-attempt-2'])]:
            with self.subTest(layout=layout):
                self.assertEqual(ARTIFACTS.execution_artifact_path(metadata(*reversed(names)), rows, 2,
                    layout=layout, repetition=3 if layout else None), 'execution-artifacts')

    def test_missing_foreign_duplicate_wrong_attempt_or_expired_artifacts_fail(self):
        name = 'tests-legacy-money-execution-attempt-2'
        foreign = 'tests-legacy-test-api-execution-attempt-2'
        cases = [[], metadata(foreign), metadata(name, foreign), metadata(name, name),
                 metadata('tests-legacy-money-execution-attempt-1'), [dict(name=name, expired=True)],
                 metadata('tests-legacy-../../money-execution-attempt-2')]
        for artifacts in cases:
            with self.subTest(artifacts=artifacts), self.assertRaisesRegex(ValueError, 'exact selected'):
                ARTIFACTS.execution_artifact_path(artifacts, [dict(id='legacy-money')], 2)

    def test_foreign_one_layout_or_wrong_repetition_cannot_be_relabelled(self):
        for names in [('lane-measurement-one-foreign-3-attempt-2',),
                      ('lane-measurement-one-all-4-attempt-2',),
                      ('lane-measurement-one-all-3-attempt-2', 'lane-measurement-one-foreign-3-attempt-2')]:
            with self.subTest(names=names), self.assertRaisesRegex(ValueError, 'exact selected'):
                ARTIFACTS.execution_artifact_path(metadata(*names), [dict(id='all')], 2, layout='one', repetition=3)

    def test_empty_duplicate_or_path_injection_selections_fail(self):
        for rows in ([], [dict(id='legacy-money')] * 2, [dict(id='../legacy-money')], [dict(id='legacy-money\npath=evil')]):
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, 'canonical IDs'):
                ARTIFACTS.execution_artifact_path([], rows, 2)

    def test_paginated_metadata_includes_late_missing_or_foreign_owners(self):
        name = 'tests-legacy-money-execution-attempt-2'
        unrelated = metadata(*(f'tests-legacy-package-{index}-attempt-2' for index in range(110)))
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / 'artifacts.jsonl'
            command = [sys.executable, str(CI / 'execution-artifact-path.py'), '--input', str(source),
                       '--matrix', '[{"id":"legacy-money"}]', '--attempt', '2']
            for artifacts, success in [(unrelated + metadata(name), True), (unrelated, False),
                    (unrelated + metadata(name, 'tests-legacy-foreign-execution-attempt-2'), False)]:
                source.write_text(''.join(json.dumps(artifact) + '\n' for artifact in artifacts))
                process = subprocess.run(command, text=True, capture_output=True)
                self.assertEqual(process.returncode == 0, success, process.stderr)
                self.assertEqual(process.stdout, 'execution-artifacts/' + name + '\n' if success else '')

    def test_workflow_shell_preserves_exact_names_across_metadata_pages(self):
        workflows = CI.parent / '.github/workflows'
        for workflow, rows, names, values in [
                ('compare-legacy-ci.yml', [dict(id='legacy-money')],
                 ['tests-legacy-money-execution-attempt-2'], {}),
                ('benchmark-lanes.yml', [dict(id='all')],
                 ['lane-measurement-one-all-3-attempt-2'], dict(LAYOUT='one', REPETITION='3'))]:
            block = (workflows / workflow).read_text().split(
                '      - name: Require exact artifact names and choose download layout\n', 1)[1].split(
                '      - uses: actions/download-artifact@', 1)[0]
            script = textwrap.dedent(block.split('        run: |\n', 1)[1])
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                gh = root / 'gh'
                gh.write_text('#!' + sys.executable + '\n' + textwrap.dedent('''\
                    import json, os, sys
                    assert sys.argv[1:] == ['api', '--paginate',
                        'repos/example/repo/actions/runs/42/artifacts?per_page=100',
                        '--jq', '.artifacts[] | {name, expired} | @json']
                    for page in json.loads(os.environ['ARTIFACT_PAGES']):
                        for artifact in page: print(json.dumps(artifact))
                    '''))
                gh.chmod(0o755)
                output = root / 'output'
                first_page = metadata(*(f'unrelated-{index}' for index in range(100)))
                for extra in ([], metadata(names[0].replace(rows[0]['id'], rows[0]['id'] + '-foreign'))):
                    output.unlink(missing_ok=True)
                    environment = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                        MATRIX=json.dumps(rows), GITHUB_RUN_ATTEMPT='2', GITHUB_REPOSITORY='example/repo',
                        GITHUB_RUN_ID='42', RUNNER_TEMP=str(root), GITHUB_OUTPUT=str(output),
                        ARTIFACT_PAGES=json.dumps([first_page, metadata(*names) + extra]), **values)
                    process = subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', script], cwd=CI.parent,
                                             env=environment, text=True, capture_output=True)
                    with self.subTest(workflow=workflow, extra=extra):
                        self.assertEqual(process.returncode == 0, not extra, process.stderr)
                        self.assertEqual(output.read_text() if output.exists() else '',
                                         'path=execution-artifacts/' + names[0] + '\n' if not extra else '')


if __name__ == '__main__':
    unittest.main()
