"""Measure commands without modifying their test selection or fixture lifecycle."""

import os
import pathlib
import platform
import re
import shutil
import subprocess
import time


def optional_command(argv, root, env=None, timeout=None):
    try:
        process = subprocess.run(argv, cwd=root, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 timeout=timeout)
        return process.stdout.strip() if process.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def environment_metadata(root, env):
    memory = pathlib.Path("/proc/meminfo")
    cpu = pathlib.Path("/proc/cpuinfo")
    build_keys = ("CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "CARGO_INCREMENTAL", "CARGO_PROFILE_TEST_DEBUG",
                  "CARGO_PROFILE_DEV_DEBUG", "RUSTUP_TOOLCHAIN", "RUSTFLAGS", "RUSTDOCFLAGS",
                  "CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS", "RUSTDOC", "AURA_CI_RUSTDOC",
                  "LLVM_PROFILE_FILE", "AURA_CI_COVERAGE_DIR")
    return {"os": platform.platform(), "architecture": platform.machine(), "logical_cpus": os.cpu_count(),
            "cpu_model": next((line.split(":", 1)[1].strip() for line in cpu.read_text().splitlines() if line.startswith("model name")), None) if cpu.exists() else None,
            "memory_total_kib": int(memory.read_text().split("MemTotal:", 1)[1].split()[0]) if memory.exists() else None,
            "github_runner_os": os.environ.get("RUNNER_OS"), "github_runner_arch": os.environ.get("RUNNER_ARCH"),
            "github_runner_image": os.environ.get("ImageOS"), "github_runner_image_version": os.environ.get("ImageVersion"),
            "rustc": optional_command(["rustc", "-vV"], root, env), "cargo": optional_command(["cargo", "--version"], root, env),
            "grcov": optional_command(["grcov", "--version"], root), "docker": optional_command(["docker", "--version"], root),
            "build_environment": {key: env.get(key) for key in build_keys}}


def fixture_references(root):
    directory = root / "src/test-api/src"
    postgres = re.search(r'const POSTGRES_IMAGE: &str = "([^"]+)";', (directory / "postgres.rs").read_text())
    floci = re.search(r'const IMAGE_TAG: &str = "([^"]+)";', (directory / "aws.rs").read_text())
    opensearch = re.search(r'\.engine_version\("OpenSearch_([0-9.]+)"\)', (directory / "opensearch.rs").read_text())
    if not all((postgres, floci, opensearch)):
        raise ValueError("Fixture image policy changed; review source-derived image inventory")
    # Floci does not universally append .0 (e.g. OpenSearch_2.19 maps to 2.19.5).
    # Review its release mapping when either fixture version changes:
    # https://github.com/floci-io/floci/blob/2.3.0/src/main/java/io/github/hectorvent/floci/services/opensearch/OpenSearchVersions.java
    reviewed_images = {("2.3.0", "3.5"): "opensearchproject/opensearch:3.5.0"}
    image = reviewed_images.get((floci[1], opensearch[1]))
    if image is None:
        raise ValueError("Unreviewed Floci/OpenSearch engine image mapping")
    references = [os.environ.get("AURA_TEST_POSTGRES_IMAGE", postgres[1]), "floci/floci:" + floci[1], image]
    managed = directory / "environment/services.rs"
    if managed.exists():
        # Managed lifecycle smoke uses immutable images alongside unmanaged regressions.
        for name in ("POSTGRES_IMAGE", "FLOCI_IMAGE", "OPENSEARCH_IMAGE"):
            match = re.search(r"const " + name + r':\s*&str\s*=\s*"([^"]+)";', managed.read_text())
            if not match or not re.fullmatch(r"[^@]+@sha256:[0-9a-f]{64}", match[1]):
                raise ValueError("Managed fixture images must have reviewed immutable digests")
            references.append(match[1])
    return list(dict.fromkeys(references))


def images(root):
    # Inspect fixture references only; never enumerate or mutate other containers.
    references = fixture_references(root)
    return [{"reference": reference, "identity": optional_command(
        ["docker", "image", "inspect", "--format", "{{json .RepoDigests}} {{.Id}}", reference], root, timeout=5)} for reference in references]


def directory_bytes(path):
    total = 0
    for item in path.rglob("*"):
        try:
            if item.is_file():
                total += item.stat().st_size
        except FileNotFoundError:
            pass  # The compiler can rename/remove a temporary output during sampling.
    return total


def measure(name, argv, cwd, env, output, target):
    log = output / (name.lower() + ".log")
    rss = output / (name.lower() + ".rss.txt")
    before = directory_bytes(target)
    disk = shutil.disk_usage(target)
    started = time.monotonic()
    peak_disk = before
    last_sample = started
    with log.open("w") as stream:
        process = subprocess.Popen(["/usr/bin/time", "-f", "%M", "-o", str(rss), "--", *argv],
                                   cwd=cwd, env=env, stdout=stream, stderr=subprocess.STDOUT)
        while True:
            try:
                process.wait(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                if time.monotonic() - last_sample >= 10:
                    peak_disk = max(peak_disk, directory_bytes(target))
                    last_sample = time.monotonic()
    seconds = time.monotonic() - started
    rss_lines = rss.read_text().splitlines()
    peak_rss = int(rss_lines[-1]) if rss_lines and rss_lines[-1].isdigit() else None
    after = directory_bytes(target)
    return {"name": name, "command": argv, "cwd": str(cwd), "exit_code": process.returncode,
            "status": "PASSED" if process.returncode == 0 else "FAILED", "seconds": round(seconds, 6),
            "peak_rss_kib": peak_rss, "rss_method": "GNU time maximum resident set size of command and children; not aggregate process-tree RSS",
            "target_disk_before_bytes": before, "target_disk_after_bytes": after,
            "target_disk_peak_sampled_bytes": max(peak_disk, after), "disk_sampling_seconds": 10,
            "disk_method": "Logical Cargo target file sizes before/after and sampled at most every 10s; brief peaks, profiles and Docker storage excluded",
            "filesystem_free_before_bytes": disk.free, "filesystem_free_after_bytes": shutil.disk_usage(target).free,
            "log": log.name}
