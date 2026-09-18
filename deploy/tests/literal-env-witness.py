#!/usr/bin/env python3
"""Private literal-environment witness; success output is fixed and non-sensitive."""
import json
import os
from pathlib import Path
import sys


def main():
    try:
        expected = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
        if type(expected) is not dict or any(type(key) is not str or type(value) is not str
                                             for key, value in expected.items()):
            return 1
        if any(os.environ.get(key) != value for key, value in expected.items()):
            return 1
    except (IndexError, OSError, UnicodeError, ValueError):
        return 1
    print("LITERAL_ENV_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
