#!/usr/bin/env python3
"""Opt-in old scheduling control: build the immutable selector against current source."""

import argparse
import json
import pathlib
import subprocess
import tomllib

from ci_inventory.catalog import build_catalog, package_owners
from ci_inventory.io import command, write_json

LEGACY_REVISION = '5c9bccac4e5c3d1e34ee021b7bbadcf78db592d4'


def prepare_selector(root, output, target):
    workspace = output / 'workspace'
    package = workspace / 'ci-determinator'
    (package / 'src').mkdir(parents=True)
    for source, destination in [('Cargo.toml', package / 'Cargo.toml'), ('src/main.rs', package / 'src/main.rs')]:
        destination.write_bytes(subprocess.check_output(['git', 'show', f'{LEGACY_REVISION}:src/ci-determinator/{source}'], cwd=root))
    old_manifest = tomllib.loads(command(['git', 'show', f'{LEGACY_REVISION}:Cargo.toml'], root))
    serde_json = old_manifest['workspace']['dependencies']['serde_json']
    (workspace / 'Cargo.toml').write_text('[workspace]\nmembers = ["ci-determinator"]\nresolver = "3"\n\n[workspace.dependencies]\nserde_json = ' + json.dumps(serde_json) + '\n')
    original_lock = command(['git', 'show', f'{LEGACY_REVISION}:Cargo.lock'], root)
    (workspace / 'Cargo.lock').write_text(original_lock)
    # Cargo prunes unrelated workspace nodes; retain every selected locked version.
    command(['cargo', 'metadata', '--format-version', '1'], workspace)
    before = {(p['name'], p['version'], p.get('source')): p.get('checksum') for p in tomllib.loads(original_lock)['package']}
    after = tomllib.loads((workspace / 'Cargo.lock').read_text())['package']
    if any((p['name'], p['version'], p.get('source')) not in before or
           p.get('checksum') != before[(p['name'], p['version'], p.get('source'))] for p in after):
        raise ValueError('Legacy selector dependency versions/checksums changed')
    subprocess.run(['cargo', 'build', '--locked', '--release', '--target-dir', str(target)], cwd=workspace, check=True)
    return target / 'release/ci-determinator'


def plan(manifest, expected, selected, full):
    package_owners(manifest, expected)
    by_path = {package['path']: name for name, package in manifest['packages'].items()}
    paths = selected['integration_test']
    if not isinstance(paths, list) or len(paths) != len(set(paths)) or any(path not in by_path for path in paths):
        raise ValueError('Original selector returned missing, duplicate or unknown package paths')
    additions = []
    if full:
        omitted = {'.', 'src/test-api/src/test-api-macros'}
        if set(paths) != set(by_path) - omitted:
            raise ValueError('Full control must select every legacy-eligible current package')
        additions = sorted(omitted & set(by_path))
    names = sorted(by_path[path] for path in [*paths, *additions])
    if not names:
        raise ValueError('Comparison selected no packages')
    return {'legacy_revision': LEGACY_REVISION, 'original_selection': selected, 'parity_added_paths': additions,
            'packages': names, 'matrix': [{'package': name, 'id': 'legacy-' + name} for name in names]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--target-dir', type=pathlib.Path, required=True)
    parser.add_argument('--full', action='store_true')
    args = parser.parse_args()
    root = pathlib.Path(command(['git', 'rev-parse', '--show-toplevel'], pathlib.Path.cwd()).strip())
    base = command(['git', 'rev-parse', '--verify', args.base + '^{commit}'], root).strip()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    selector = prepare_selector(root, output, args.target_dir.resolve())
    selected = json.loads(command([str(selector), base], root))
    metadata = json.loads(command(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], root))
    _, expected = build_catalog(metadata, root)
    report = plan(json.loads((root / 'ci/test-lanes.json').read_text()), expected, selected, args.full)
    report.update(base=base, revision=command(['git', 'rev-parse', 'HEAD'], root).strip(),
                  changed_paths=command(['git', 'diff', '--name-only', base], root).splitlines())
    write_json(output / 'selection.json', report)
    print(json.dumps(report, sort_keys=True))


if __name__ == '__main__':
    main()
