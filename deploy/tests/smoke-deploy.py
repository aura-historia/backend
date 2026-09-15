#!/usr/bin/env python3
"""One <=30min R4 host-CLI rehearsal, reusing R3's complete fresh fixture setup.

Fixed local socket/preloaded A+B. Only build: labelled, local-B-derived exit42
fault wrapper. No Cargo, pulls, real secrets, cloud calls or durable queue claim.
Unknown command outcomes retain exact owned projects; never retry automatically.
"""
import contextlib
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
from test_deploy import load_deploy

spec = importlib.util.spec_from_file_location("r4_r3", Path(__file__).with_name("smoke-compose.py"))
r3 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r3)
r2 = r3.r2
deploy = load_deploy()
require = r2.require
A = dict(source_sha=r3.provider.SHA, images={k: r3.IMAGES[k] for k in deploy.BINS})
B = dict(source_sha="08173849aa0a5f5849a340a1030b94af2162aee8", images=dict(
    api="sha256:829b7073da83e6be4e17843e18267c822a073b19fa60ae6dac159a1e4ab16655",
    worker="sha256:9a54660aeff8bf2ba00cd258e013d464bdbb1e8e7c44c11103280a6344aa2a9e",
    cron="sha256:1e3a0d90c9537d9ea0e3e368752c9b543a2dd4c04059779666892a11aad6a63b",
    crawler="sha256:063c0f50186e12c49ea77990f43c6c1c7518c029db3be1545e8d6cde87f9fa5b"))
BASE_TAG = "aura-r4-fault-base:" + B["source_sha"]
FAULT_LABEL = "r4-api-exit42-wrapper-not-source-built"
TOOLING_FILES = ("deploy/bin/deploy", "deploy/catalog.json", "deploy/compose/compose.application.yml",
                 "deploy/compose/compose.replace.yml", "deploy/compose/compose.edge.yml", "deploy/compose/Caddyfile.replace")
TOOLING_DIRS = ("", "deploy", "deploy/bin", "deploy/compose")


def check_tooling(root, hashes):
    r3.checked_path(root)
    require(set(hashes) == set(TOOLING_FILES), "INSTALLED_TOOLING_SET")
    require({str(p.relative_to(root)) for p in root.rglob("*")} ==
            set(TOOLING_FILES) | set(TOOLING_DIRS[1:]), "INSTALLED_TOOLING_TREE")
    for name in TOOLING_DIRS:
        info = (root / name).stat()
        require(info.st_uid == os.getuid() and info.st_mode & 0o777 == 0o700, "INSTALLED_DIRECTORY_MODE")
    for name in TOOLING_FILES:
        path = root / name
        info = path.stat()
        require(info.st_uid == os.getuid() and info.st_mode & 0o777 ==
                (0o755 if name == "deploy/bin/deploy" else 0o644), "INSTALLED_FILE_MODE")
        require(hashlib.sha256(path.read_bytes()).hexdigest() == hashes[name]
                == hashlib.sha256((r3.ROOT / name).read_bytes()).hexdigest(), "INSTALLED_TOOLING_BYTES")


def install_tooling(directory):
    # Test analogue of a protected helper install: copy bytes, never chmod checkout.
    r3.checked_path(directory)
    require(directory.stat().st_uid == os.getuid() and directory.stat().st_mode & 0o777 == 0o700, "INSTALL_PARENT")
    root = directory / "installed-tooling"
    hashes = {name: hashlib.sha256((r3.ROOT / name).read_bytes()).hexdigest() for name in TOOLING_FILES}
    for name in TOOLING_DIRS:
        (root / name).mkdir(mode=0o700)
    for name in TOOLING_FILES:
        shutil.copyfile(r3.ROOT / name, root / name)
        (root / name).chmod(0o755 if name == "deploy/bin/deploy" else 0o644)
    check_tooling(root, hashes)
    return root, hashes


class Completed(Exception):
    """Stop inherited R3 run after its first TLS check; don't repeat R3 restart."""


class Rehearsal(r3.Harness):
    host = None
    bad = None
    completed_current = None
    installed_root = None

    def cli(self, action, selected=None, *, slot="api", confirm=False, failure=None):
        check_tooling(self.installed_root, self.installed_hashes)
        args = [sys.executable, str(self.installed_root / "deploy/bin/deploy"), "--config", str(self.directory / "host.json"), action]
        if selected is not None:
            path = self.directory / "release.json"
            deploy.atomic(path, json.dumps(selected))
            args += ["--release", str(path), "--api-slot", slot]
        if confirm:
            args.append("--commands-finished")
        try:
            with tempfile.TemporaryFile() as output:
                result = subprocess.run(args, stdout=output, stderr=subprocess.STDOUT,
                    env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"},
                    timeout=max(1, self.deadline - time.monotonic()))
                output.seek(0)
                raw = output.read(1048577)
            require(len(raw) <= 1048576, "CLI_OUTPUT_LIMIT")
            text = raw.decode()
            r2.safe_output(text)
            records = [json.loads(line) for line in text.splitlines()]
        except (OSError, subprocess.TimeoutExpired):
            self.uncertain = True
            self.journal()
            raise r2.Failure("HOST_COMMAND_OUTCOME_UNKNOWN") from None
        for record in records:
            if "phase" in record or "result" in record:
                print("HOST " + json.dumps(record, sort_keys=True), flush=True)
        if failure is None:
            require(result.returncode == 0, "HOST_CLI_FAILED_" + action)
        else:
            require(result.returncode != 0 and records[-1].get("code") in failure, "EXPECTED_CLI_FAILURE_" + action)
        return records

    def snapshot(self):
        result = {}
        for kind in ("platform", "edge"):
            found, volumes, _ = self.refresh(kind, r3.SERVICES[kind])
            result[kind] = ({n: (v["Id"], v["State"]["StartedAt"]) for n, v in found.items()},
                            sorted(volumes, key=lambda v: v["Name"]))
        return result

    def sequin_snapshot(self):
        catalog = json.loads((r3.ROOT / "deploy/catalog.json").read_text())
        self.sequin_ready(catalog)
        sinks = json.loads(self.sql("smoke_sequin", "SELECT coalesce(json_agg(t),'[]') FROM sequin_config.sink_consumers t;"))
        require(all("max_retry_count" in s and s["max_retry_count"] is None for s in sinks), "SEQUIN_BOUNDED_RETRY")
        endpoints = json.loads(self.sql("smoke_sequin", "SELECT coalesce(json_agg(t),'[]') FROM sequin_config.http_endpoints t;"))
        return (sorted([(s["id"], s["name"], s["status"], s["sink"], s["max_retry_count"], s["load_shedding_policy"]) for s in sinks]),
                sorted(endpoints, key=lambda e: e["id"]))

    def reconcile_caddy(self, selected, slot):
        # Only a completed, independently probed current may advance this one seal.
        current = self.host.stored("current")
        require(self.host.stored("incomplete") is None and current == self.host.converged(selected, slot), "UNVERIFIED_CADDY_RECONCILIATION")
        expected = deploy.caddyfile(slot, selected["source_sha"])
        require(self.host.caddy.read_text() == expected, "CADDY_BYTES")
        self.host.caddy.chmod(0o444)  # Restore R3's synthetic public-bind permission.
        hashes = r3.fixture_hashes(self.directory)
        require({k: v for k, v in hashes.items() if k != "caddy/Caddyfile"} ==
                {k: v for k, v in self.fixture_inputs.items() if k != "caddy/Caddyfile"}, "NON_CADDY_FIXTURE_DRIFT")
        self.fixture_inputs["caddy/Caddyfile"] = hashes["caddy/Caddyfile"]
        r3.CADDYFILE = expected
        r3.validate_caddyfile(self.directory)
        self.completed_current = current

    @contextlib.contextmanager
    def events(self):
        # Streaming avoids Docker's small retrospective event buffer. No raw output.
        with tempfile.TemporaryFile() as output:
            command = ["/usr/bin/docker", "--host", "unix:///var/run/docker.sock", "--config", self.docker.config,
                "events", "--since", str(int(time.time())), "--until", str(int(time.time() + max(1, self.deadline - time.monotonic()))),
                "--filter", "type=container", "--filter", "label=com.docker.compose.project=" + self.projects["application"],
                "--format", "{{json .}}"]
            process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, env={"PATH": "/usr/bin:/bin"})
            records = []
            try:
                time.sleep(0.2)
                require(process.poll() is None, "EVENT_WATCHER_NOT_RUNNING")
                yield records
            finally:
                process.terminate()
                process.wait(timeout=5)
                output.seek(0)
                raw = output.read(8388609)
                require(len(raw) <= 8388608, "EVENT_OUTPUT_LIMIT")
                r2.safe_output(raw.decode())
                records.extend(json.loads(line) for line in raw.splitlines())

    def handover_evidence(self, events, old, new):
        def event(identifier, action):
            matches = [e["timeNano"] for e in events if e["Actor"]["ID"] == identifier and e["Action"] == action]
            require(len(matches) == 1, "EXACT_EVENT_" + action)
            return matches[0]
        for name in (*deploy.WORKERS, *deploy.JOBS):
            stopped = event(old["containers"][name], "die")
            removed = event(old["containers"][name], "destroy")
            created = event(new["containers"][name], "create")
            require(stopped < removed < created, "SINGLETON_HANDOVER_" + name)
            print("EVIDENCE " + json.dumps(dict(service=name, old=old["containers"][name], new=new["containers"][name],
                old_die_ns=stopped, old_destroy_ns=removed, new_create_ns=created)), flush=True)
        candidate_started = event(new["containers"]["api-candidate"], "start")
        require(candidate_started < self.host.caddy.stat().st_mtime_ns < event(old["containers"]["api"], "destroy"), "CANDIDATE_PROXY_OLD_API_ORDER")
        require("api" not in self.host.containers("application"), "OLD_API_REMAINS")
        print("PASS Docker events: 12 old singleton deaths/removals before successor creation; candidate start before new proxy file, old API removed after", flush=True)

    def build_fault(self):
        base = json.loads(self.call("image", "inspect", B["images"]["api"]))[0]
        existing = self.call("image", "inspect", BASE_TAG, check=False)
        require(existing.returncode != 0 or json.loads(existing.stdout)[0]["Id"] == base["Id"], "FAULT_BASE_TAG_COLLISION")
        self.call("image", "tag", B["images"]["api"], BASE_TAG)
        require(json.loads(self.call("image", "inspect", BASE_TAG))[0]["Id"] == B["images"]["api"], "FAULT_BASE_PIN")
        iidfile = self.directory / "fault.iid"
        self.call("build", "--pull=false", "--network=none", "--iidfile", str(iidfile), "-f",
                  str(Path(__file__).with_name("Dockerfile.bad-api")), str(Path(__file__).parent), seconds=120)
        identifier = iidfile.read_text().strip()
        item = json.loads(self.call("image", "inspect", identifier))[0]
        require(item["RootFS"]["Layers"][:len(base["RootFS"]["Layers"])] == base["RootFS"]["Layers"]
                and item["Config"]["Labels"].get("org.aura-historia.test.fault") == FAULT_LABEL, "FAULT_DERIVATION")
        self.bad = dict(source_sha=B["source_sha"], images=dict(B["images"], api=identifier))
        self.host.images(self.bad)
        print("EVIDENCE fault-injection ONLY (not source-built release) " + identifier + " base=" + base["Id"], flush=True)

    def remove_slot(self, slot, selected, *, fault=False):
        items = self.host.containers("application")
        item = items[slot]
        r3.owned_container(item, self.projects["application"], {slot})
        require(item["Image"] == selected["images"]["api"], "CLEANUP_SLOT_IMAGE")
        r3.validate_mounts("api", item["Mounts"], self.projects["application"], self.directory, actual=True)
        model = self.host.model(selected)
        baked = self.host.images(selected)["api"]
        require(dict(v.split("=", 1) for v in item["Config"]["Env"]) ==
                {**baked, **model["services"][slot]["environment"]}, "CLEANUP_SLOT_ENV")
        self.call("stop", "--time", "60", item["Id"], seconds=90)
        terminal = json.loads(self.call("inspect", item["Id"]))[0]["State"]
        require(not terminal["Running"] and not terminal["Restarting"] and terminal["Pid"] == 0
                and terminal["ExitCode"] == (42 if fault else 0), "CLEANUP_SLOT_TERMINAL")
        self.call("rm", item["Id"])
        require(slot not in self.host.containers("application"), "CLEANUP_SLOT_REMAINS")

    def tls_get(self, port):
        # Inherited run reaches here only after complete R3 image/model/mount/start gates.
        super().tls_get(port)
        self.phase_start("R4 host CLI adopt/apply/failure/recover")
        state = self.directory / "state"
        state.mkdir(mode=0o700)
        ca = self.directory / "https-ca.pem"
        deploy.atomic(ca, self.call("exec", self.ids["caddy"], "cat", "/data/caddy/pki/authorities/local/root.crt"))
        config = dict(stage="test", application_project=self.projects["application"], edge_project=self.projects["edge"],
            config_dir=str(self.directory), compose_env=str(self.directory / "compose.env"), state_dir=str(state),
            https_port=port, https_ca=str(ca))
        deploy.atomic(self.directory / "host.json", json.dumps(config))
        self.host = deploy.Host(config)
        self.host.deadline = self.deadline
        self.host.images(B)
        self.cli("status")
        self.cli("adopt", A)
        old = self.host.stored("current")
        require(old == self.host.converged(A, "api"), "ADOPTED_A")
        self.completed_current = old
        stable = self.snapshot()
        sequin = self.sequin_snapshot()
        before = self.stats()
        with self.events() as events:
            self.cli("apply", B)
        new = self.host.stored("current")
        require(new["release"] == B and new["api_slot"] == "api-candidate" and self.host.stored("previous") == old, "RELEASE_STATE_AB")
        self.reconcile_caddy(B, "api-candidate")
        self.handover_evidence(events, old, new)
        self.witnesses(before)
        require(self.snapshot() == stable and self.sequin_snapshot() == sequin, "PLATFORM_EDGE_SEQUIN_CHANGED")
        catalog = json.loads((r3.ROOT / "deploy/catalog.json").read_text())
        self.empty_and_unchanged(catalog)
        print("PASS host CLI A -> B; all four exact B IDs/13 ready+source probes; current B/previous A; platform/edge volumes/start times and Sequin unchanged", flush=True)
        self.build_fault()
        before_items = self.host.containers("application")
        with self.events() as failed_events:
            self.cli("apply", self.bad, failure={"PROCESS_NOT_STABLE", "READINESS_DEADLINE"})
        marker = self.host.stored("incomplete")
        require(marker["phase"] == "candidate-start" and marker["from"] == new
                and marker["previous"] == old and marker["target"] == self.bad
                and self.host.stored("current") == new, "FAILED_CANDIDATE_MARKER")
        after_items = self.host.containers("application")
        require(all(after_items[n]["Id"] == v["Id"] and after_items[n]["State"]["Running"]
                    and after_items[n]["State"]["StartedAt"] == v["State"]["StartedAt"] for n, v in before_items.items()), "FAILED_CANDIDATE_TOUCHED_ACTIVE")
        active_ids = {v["Id"] for v in before_items.values()}
        require(not any(e["Actor"]["ID"] in active_ids and e["Action"] in {"stop", "die", "destroy", "kill"} for e in failed_events), "FAILED_CANDIDATE_STOPPED_ACTIVE")
        self.host.edge("api-candidate", B)
        self.cli("apply", B, failure={"INCOMPLETE_REQUIRES_RECOVERY"})
        self.cli("recover", B, slot="api-candidate", failure={"RECOVERY_REQUIRES_FINISHED_COMMANDS_CONFIRMATION"})
        self.cli("recover", B, slot="api-candidate", confirm=True, failure={"MIXED_APPLICATION_STATE"})
        # Explicit test-operator convergence; host recover must not perform rollback.
        self.remove_slot("api", self.bad, fault=True)
        self.host.converged(B, "api-candidate")
        self.cli("recover", B, slot="api-candidate", confirm=True)
        require(self.host.stored("last-incomplete") == marker and self.host.stored("current") == new
                and self.host.stored("previous") == old
                and self.host.stored("incomplete") is None, "RECOVERY_STATE")
        self.reconcile_caddy(B, "api-candidate")
        lock = os.open(state / "lock", os.O_RDWR)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.cli("status", failure={"DEPLOYMENT_BUSY"})
        finally:
            os.close(lock)
        require(self.snapshot() == stable and self.sequin_snapshot() == sequin, "FAILURE_PATH_STATEFUL_DRIFT")
        self.empty_and_unchanged(catalog)
        self.stats()
        print("PASS failed fixture candidate: active B untouched, incomplete blocks apply, explicit remove/convergence then confirmed recover; real flock contention", flush=True)
        raise Completed()

    def cleanup(self):
        require(not self.uncertain, "UNKNOWN_OUTCOME_RETAINED")
        if self.host is not None:
            items = self.host.containers("application")
            if "api-candidate" in items:
                # Only remove the exact verified completed slot, never broaden R3 globals.
                current = self.host.stored("current")
                require(current == self.completed_current and self.host.stored("incomplete") is None
                        and current == self.host.converged(current["release"], current["api_slot"]), "UNVERIFIED_CANDIDATE_TEARDOWN")
                self.remove_slot("api-candidate", current["release"])
        super().cleanup()


def main():
    global deploy
    require(len(sys.argv) == 1, "NO_LIVE_INPUTS_ACCEPTED")
    # Exact reviewed R4 A config, set before materialization. Baseline module untouched.
    r3.CADDYFILE = deploy.caddyfile("api", A["source_sha"])
    directory = Path(tempfile.mkdtemp(prefix="aura-r4-test-"))
    harness = Rehearsal(directory)
    def interrupted(signum, _frame):
        harness.uncertain = True
        raise r2.Failure("INTERRUPTED_" + str(signum))
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, interrupted)
    passed = False
    try:
        harness.installed_root, harness.installed_hashes = install_tooling(directory)
        deploy = load_deploy(harness.installed_root / "deploy/bin/deploy")
        require(deploy.ROOT == harness.installed_root and deploy.caddyfile("api", A["source_sha"]) == r3.CADDYFILE, "INSTALLED_ROOT_AND_TEMPLATE")
        print("PASS private installed tooling: six exact source hashes, files0644/script0755, checkout untouched", flush=True)
        harness.run(False)
    except Completed:
        passed = True
    except Exception as error:
        code = str(error) if isinstance(error, (r2.Failure, deploy.Failure)) else type(error).__name__
        frames = [(Path(f.filename).name, f.lineno) for f in traceback.extract_tb(error.__traceback__)]
        print("FAIL phase=" + harness.phase + " code=" + code + " frames=" + json.dumps(frames), flush=True)
    finally:
        try:
            harness.cleanup()
            shutil.rmtree(directory)
        except Exception as error:
            passed = False
            harness.uncertain = True
            harness.journal()
            code = str(error) if isinstance(error, (r2.Failure, deploy.Failure)) else type(error).__name__
            print("RETAINED " + str(directory) + " projects=" + json.dumps(harness.projects) + " code=" + code, flush=True)
    print("PASS R4 isolated idle host replacement (not queue custody/live acceptance)" if passed else "FAIL R4 (no acceptance claim)", flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
