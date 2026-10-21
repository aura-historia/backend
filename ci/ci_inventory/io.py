"""Small, explicit process and artifact boundaries for inventory collection."""

import json
import subprocess


def command(argv, cwd=None):
    result = subprocess.run(argv, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"{' '.join(argv)} failed ({result.returncode}): {result.stderr[-6000:]}")
    return result.stdout


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
