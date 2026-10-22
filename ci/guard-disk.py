#!/usr/bin/env python3
"""Observe job-owned build/profile growth and cancel before runner disk exhaustion."""

import argparse
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import time

from ci_inventory.process import execute


def observe(argv, root):
    try:
        result = subprocess.run(argv, cwd=root, text=True, capture_output=True, timeout=5)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def memory_kib():
    result = {'total': None, 'available': None}
    fields = {'MemTotal': 'total', 'MemAvailable': 'available'}
    try:
        lines = pathlib.Path('/proc/meminfo').read_text().splitlines()
    except (OSError, UnicodeError):
        return result
    for line in lines:
        name, _, value = line.partition(':')
        parts = value.split()
        if name in fields and len(parts) == 2 and parts[0].isdecimal() and parts[1] == 'kB':
            result[fields[name]] = int(parts[0])
    return result


class DiskGuard:
    def __init__(self, root, target, profiles, log, minimum):
        self.root, self.log, self.minimum = root, log, minimum
        self.paths = {'target': target}
        if profiles is not None:
            self.paths['profiles'] = profiles
        self.last_print = self.last_sizes = -float('inf')
        self.sizes, self.docker, self.low, self.command = dict.fromkeys(self.paths), None, False, None
        log.parent.mkdir(parents=True, exist_ok=True)

    def check(self):
        now = time.monotonic()
        try:
            free_by_path = {}
            for name, path in {'workspace': self.root, **self.paths}.items():
                while not path.exists():
                    path = path.parent
                free_by_path[name] = shutil.disk_usage(path).free
            free = min(free_by_path.values())
        except OSError:
            self.low = True
            return signal.SIGTERM
        self.low = self.low or free < self.minimum
        if now - self.last_print >= 20 or free < self.minimum:
            if free >= self.minimum and now - self.last_sizes >= 60:
                self.sizes = {name: observe(['du', '-sk', str(path)], self.root) if path.exists() else None
                              for name, path in self.paths.items()}
                if os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted':
                    self.docker = observe(['docker', 'system', 'df', '--format', '{{json .}}'], self.root)
                self.last_sizes = now
            record = {'time': time.time(), 'free_bytes': free, 'free_by_path': free_by_path, 'minimum_bytes': self.minimum,
                      'memory_kib': memory_kib(),
                      'sizes_kib': self.sizes, 'hosted_docker_storage': self.docker,
                      'sizes_sampled_monotonic': None if self.last_sizes == -float('inf') else self.last_sizes,
                      'command': self.command}
            line = json.dumps(record, sort_keys=True)
            print('DISK_OBSERVATION ' + line, flush=True)
            try:
                with self.log.open('a') as stream:
                    stream.write(line + '\n')
            except OSError as error:
                print('Could not retain disk observation: ' + str(error), file=sys.stderr)
            self.last_print = now
        if free < self.minimum:
            self.low = True
            return signal.SIGTERM
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', type=pathlib.Path, required=True)
    parser.add_argument('--profiles', type=pathlib.Path, help='Optional coverage profile directory')
    parser.add_argument('--log', type=pathlib.Path, required=True)
    parser.add_argument('--minimum-bytes', type=int, default=3 * 1024 ** 3)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.command[1:] if args.command[:1] == ['--'] else args.command
    if args.minimum_bytes <= 0 or not argv:
        parser.error('positive free-space floor and a command are required')
    root = pathlib.Path.cwd()
    guard = DiskGuard(root, args.target, args.profiles, args.log, args.minimum_bytes)
    guard.command = argv
    if guard.check() is not None:
        print('Insufficient disk before command; see disk observations.', file=sys.stderr)
        return 99
    try:
        execute(argv, root, os.environ, guard.check)
    except SystemExit as error:
        return 99 if guard.low and error.code == 128 + signal.SIGTERM else error.code
    finally:
        guard.last_print = -float('inf')
        if not guard.low:
            guard.last_sizes = -float('inf')
        guard.check()
    return 99 if guard.low else 0


if __name__ == '__main__':
    sys.exit(main())
