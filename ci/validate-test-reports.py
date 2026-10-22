#!/usr/bin/env python3
"""Require exact selected execution artifacts and their compiled case inventories."""

import argparse
import json
import pathlib
import re

from ci_inventory.catalog import legacy_package_selection
from ci_inventory.discovery import discovery_builds_supervisor, listed_cases
from ci_inventory.phases import combined_selection, suite_identity, validate_special_cases


def read_json(path):
    return json.loads(path.read_text())


def validate_discovery(directory, selection, manifest):
    if read_json(directory / 'selection.json') != selection:
        raise ValueError(f'Selected packages/targets differ: {directory}')
    data = read_json(directory / 'nextest.json')
    suites, cases = {}, {}
    required = {target['id'] for name in selection['packages'] for target in manifest['packages'][name]['targets']
                if target['test'] and target['harness']}
    for binary, suite in data['rust-suites'].items():
        identity = suite_identity(suite)
        if identity in suites or identity not in selection['targets']:
            raise ValueError(f'Duplicate or foreign suite: {identity}')
        suites[identity] = suite
        cases[identity] = suite['testcases']
        if any(type(case['ignored']) is not bool for case in cases[identity].values()):
            raise ValueError(f'Invalid ignored flags: {identity}')
        listing = directory / (re.sub(r'[^\w.-]', '_', binary) + '.list.txt')
        if listed_cases(listing.read_text()) != set(cases[identity]):
            raise ValueError(f'Compiled case list differs: {identity}')
    if not required <= set(suites) or data['test-count'] != sum(len(cases) for cases in cases.values()):
        raise ValueError(f'Missing suites or inconsistent case count: {directory}')
    validate_special_cases(selection, suites)
    (directory / 'compile.log').read_text()
    return cases


def validate_benchmark(directory, row, manifest, revision, prime):
    lanes = row['lanes'].split()
    selection = combined_selection(manifest, manifest, lanes)
    separate_supervisor = row['profile'] != 'none' and not discovery_builds_supervisor(lanes, None, 'none')
    expected_stages = ['COMPILE_LINK', *(['SUPERVISOR_COMPILE'] if separate_supervisor else []), 'TESTS_AND_FIXTURES']
    for cache, source in [('COLD_TARGET', prime or revision), ('WARM_TARGET', revision)]:
        output = directory / cache
        report = read_json(output / 'benchmark.json')
        if (report['status'] != 'PASSED' or report['revision'] != source or report['dirty'] or
                report['cache'] != cache or report['coverage_mode'] != 'none' or report['coverage'] is not None or
                report['lanes'] != row['lanes'].split() or report['profile'] != row['profile'] or
                [stage['name'] for stage in report['stages']] != expected_stages or
                any(stage['exit_code'] != 0 or stage['status'] != 'PASSED' for stage in report['stages'])):
            raise ValueError(f'Incomplete or unexpected measurement: {output}')
        compiled = validate_discovery(output / 'compile-discovery', selection, manifest)
        if validate_discovery(output / 'discovery', selection, manifest) != compiled:
            raise ValueError(f'Compile/execution case or ignored inventory differs: {output}')
        (output / 'tests_and_fixtures.log').read_text()
        (output / 'disk.jsonl').read_text()


def validate_legacy(directory, row, manifest, revision):
    selection = legacy_package_selection(manifest, manifest, row['package'], row.get('parity_packages', '').split())
    report = read_json(directory / 'job.json')
    if report['checkout_sha'] != revision or report['coverage_mode'] != 'none':
        raise ValueError(f'Unexpected legacy source or coverage mode: {directory}')
    validate_discovery(directory / 'discovery', selection, manifest)
    for name in ('tests.log', 'disk.jsonl', 'fixture-images.json'):
        (directory / name).read_text()


def validate_reports(directory, rows, manifest, revision, attempt, *, layout=None, repetition=None, prime=None):
    ids = [row['id'] for row in rows]
    if not ids or len(set(ids)) != len(ids) or any(not re.fullmatch(r'[a-z][a-z0-9_-]*', identity) for identity in ids):
        raise ValueError('Selected reports must have distinct, nonempty canonical IDs')
    names = {row['id']: (f'lane-measurement-{layout}-{row["id"]}-{repetition}-attempt-{attempt}' if layout else
                         f'tests-{row["id"]}-execution-attempt-{attempt}') for row in rows}
    if set(path.name for path in directory.iterdir()) != set(names.values()):
        raise ValueError('Execution artifacts differ from the exact selected job set')
    for row in rows:
        output = directory / names[row['id']]
        (output / 'disk-preparation.log').read_text()
        if layout:
            validate_benchmark(output, row, manifest, revision, prime)
        else:
            validate_legacy(output, row, manifest, revision)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=pathlib.Path, required=True)
    parser.add_argument('--matrix', type=json.loads, required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--attempt', type=int, required=True)
    parser.add_argument('--layout', choices=['four', 'three', 'one'])
    parser.add_argument('--repetition', type=int)
    parser.add_argument('--prime-revision')
    args = parser.parse_args()
    if args.attempt < 1 or bool(args.layout) != bool(args.repetition) or (args.repetition is not None and args.repetition < 1):
        parser.error('Positive attempt and paired layout/repetition are required')
    validate_reports(args.input, args.matrix, read_json(pathlib.Path('ci/test-lanes.json')), args.revision, args.attempt,
                     layout=args.layout, repetition=args.repetition, prime=args.prime_revision)
    print('Validated every selected execution artifact and compiled case inventory')


if __name__ == '__main__':
    main()
