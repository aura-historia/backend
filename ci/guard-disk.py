#!/usr/bin/env python3
"""Cancel the owned CI command before runner disk exhaustion."""

import argparse
import json
import os
import pathlib
import shutil
import signal
import sys
import time

from ci_inventory.process import execute


class DiskGuard:
    def __init__(self, root, target, log, minimum):
        self.paths = {"workspace": root, "target": target}
        self.log, self.minimum = log, minimum
        self.last_record, self.low = -float("inf"), False
        log.parent.mkdir(parents=True, exist_ok=True)

    def check(self):
        try:
            free_by_path = {}
            for name, path in self.paths.items():
                while not path.exists():
                    path = path.parent
                free_by_path[name] = shutil.disk_usage(path).free
        except OSError as error:
            print(f"Could not read disk capacity: {error}", file=sys.stderr)
            self.low = True
            return signal.SIGTERM
        free = min(free_by_path.values())
        self.low = self.low or free < self.minimum
        if time.monotonic() - self.last_record >= 20 or free < self.minimum:
            record = {"time": time.time(), "free_bytes": free, "free_by_path": free_by_path,
                      "minimum_bytes": self.minimum}
            line = json.dumps(record, sort_keys=True)
            print("DISK_OBSERVATION " + line, flush=True)
            try:
                with self.log.open("a") as stream:
                    stream.write(line + "\n")
            except OSError as error:
                print(f"Could not retain disk observation: {error}", file=sys.stderr)
            self.last_record = time.monotonic()
        return signal.SIGTERM if free < self.minimum else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=pathlib.Path, required=True)
    parser.add_argument("--log", type=pathlib.Path, required=True)
    parser.add_argument("--minimum-bytes", type=int, default=3 * 1024 ** 3)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.minimum_bytes <= 0 or not argv:
        parser.error("positive free-space floor and a command are required")
    root = pathlib.Path.cwd()
    guard = DiskGuard(root, args.target, args.log, args.minimum_bytes)
    if guard.check() is not None:
        print("Insufficient disk before command; see disk observations.", file=sys.stderr)
        return 99
    try:
        execute(argv, root, os.environ, guard.check)
    except SystemExit as error:
        return 99 if guard.low and error.code == 128 + signal.SIGTERM else error.code
    finally:
        guard.last_record = -float("inf")
        guard.check()
    return 99 if guard.low else 0


if __name__ == "__main__":
    sys.exit(main())
