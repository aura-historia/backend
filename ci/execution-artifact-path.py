#!/usr/bin/env python3
"""Require exact artifact metadata before adapting singleton download layout."""

import argparse
import fnmatch
import json
import pathlib
import re


def execution_artifact_path(artifacts, rows, attempt, *, layout=None, repetition=None):
    ids = [row['id'] for row in rows]
    if not ids or len(set(ids)) != len(ids) or any(not re.fullmatch(r'[a-z][a-z0-9_-]*', identity) for identity in ids):
        raise ValueError('Selected reports must have distinct, nonempty canonical IDs')
    if attempt < 1 or bool(layout) != bool(repetition) or (repetition is not None and repetition < 1):
        raise ValueError('Positive attempt and paired layout/repetition are required')
    if layout:
        if layout not in ('four', 'three', 'one'):
            raise ValueError('Unknown measurement layout')
        pattern = f'lane-measurement-{layout}-*-{repetition}-attempt-{attempt}'
        names = [f'lane-measurement-{layout}-{identity}-{repetition}-attempt-{attempt}' for identity in ids]
    else:
        pattern = f'tests-legacy-*-execution-attempt-{attempt}'
        names = [f'tests-{identity}-execution-attempt-{attempt}' for identity in ids]
    matching = [artifact for artifact in artifacts if fnmatch.fnmatchcase(artifact['name'], pattern)]
    if sorted(artifact['name'] for artifact in matching) != sorted(names) or any(
            artifact['expired'] is not False for artifact in matching):
        raise ValueError('Artifact metadata differs from the exact selected job set')
    # The pinned download action flattens a single match even with merge-multiple disabled.
    return 'execution-artifacts' + ('/' + names[0] if len(names) == 1 else '')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=pathlib.Path, required=True)
    parser.add_argument('--matrix', type=json.loads, required=True)
    parser.add_argument('--attempt', type=int, required=True)
    parser.add_argument('--layout', choices=['four', 'three', 'one'])
    parser.add_argument('--repetition', type=int)
    args = parser.parse_args()
    artifacts = [json.loads(line) for line in args.input.read_text().splitlines()]
    print(execution_artifact_path(artifacts, args.matrix, args.attempt, layout=args.layout, repetition=args.repetition))


if __name__ == '__main__':
    main()
