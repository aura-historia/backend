"""Owned child process groups and cancellation for CI commands."""

import os
import shlex
import signal
import subprocess


def execute(argv, cwd, environment):
    print(f"test lane: {shlex.join(map(str, argv))}", flush=True)
    process, interrupted = None, None

    def forward(number, frame):
        nonlocal interrupted
        interrupted = number
        if process is not None and process.poll() is None:
            try:
                os.killpg(process.pid, number)
            except ProcessLookupError:
                pass

    previous = {number: signal.signal(number, forward) for number in (signal.SIGINT, signal.SIGTERM)}
    try:
        process = subprocess.Popen(argv, cwd=cwd, env=environment, start_new_session=True)
        if interrupted is not None:
            forward(interrupted, None)
        code = process.wait()
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)
    if interrupted is not None:
        raise SystemExit(128 + interrupted)
    if code:
        raise SystemExit(128 - code if code < 0 else code)

