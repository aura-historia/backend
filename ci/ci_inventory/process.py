"""Owned child process groups and cancellation for CI commands."""

import os
import pathlib
import shlex
import shutil
import signal
import subprocess
import sys
import time


def preserve_junit(report, destination):
    if destination is not None and report.is_file():
        try:
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(report, destination)
        except OSError as error:
            print(f"test lane: could not preserve JUnit: {error}", file=sys.stderr)
            return error
    return None


def execute(argv, cwd, environment, check=None, junit=None):
    print(f"test lane: {shlex.join(map(str, argv))}", flush=True)
    started = time.monotonic()
    report = pathlib.Path(cwd) / "target/nextest" / environment.get("NEXTEST_PROFILE", "default") / "junit.xml"
    if junit is not None:
        report.unlink(missing_ok=True)
    process, interrupted, report_error = None, None, None

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
        if check is None:
            code = process.wait()
        else:
            while True:
                if process.poll() is None:
                    try:
                        number = check()
                    except BaseException:
                        forward(signal.SIGTERM, None)
                        process.wait()
                        raise
                    if number is not None and interrupted is None and process.poll() is None:
                        forward(number, None)
                try:
                    code = process.wait(timeout=1)
                    break
                except subprocess.TimeoutExpired:
                    pass
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)
        report_error = preserve_junit(report, junit)
        print(f"test lane elapsed: {time.monotonic() - started:.3f}s: {shlex.join(map(str, argv))}", flush=True)
    if interrupted is not None:
        raise SystemExit(128 + interrupted)
    if code:
        raise SystemExit(128 - code if code < 0 else code)
    if report_error is not None:
        raise report_error
