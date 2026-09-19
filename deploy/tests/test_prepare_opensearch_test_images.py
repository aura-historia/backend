"""Run the real image-preparation script against a non-forwarding Docker stub."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest


DOCKER_STUB = r'''
import json
import os
import pathlib
import sys

root = pathlib.Path(os.environ["FAKE_DOCKER_STATE"])
fixture = json.loads((root / "metadata.json").read_text())
args = sys.argv[1:]
with (root / "commands.jsonl").open("a") as stream:
    stream.write(json.dumps(args) + "\n")


def reject():
    raise SystemExit(97)


if args and args[0] == "build":
    if fixture.get("fail_build"):
        raise SystemExit(31)
    if "--target" not in args or "--platform" not in args or "--iidfile" not in args:
        reject()
    if args[args.index("--target") + 1] != "opensearch-test-helper":
        reject()
    if args[args.index("--platform") + 1] != "linux/amd64":
        reject()
    output = pathlib.Path(args[args.index("--iidfile") + 1])
    if output.parent.resolve() != root.resolve():
        reject()
    output.write_text(fixture["helper_lookup_id"] + "\n")
elif args and args[0] == "pull":
    if fixture.get("fail_pull"):
        raise SystemExit(32)
    if args != ["pull", "--platform", "linux/amd64", fixture["reference"]]:
        reject()
elif args[:2] == ["image", "inspect"]:
    if "--format" not in args:
        reject()
    fmt = args[args.index("--format") + 1]
    selected = args[-1]
    if selected == fixture["helper_lookup_id"]:
        metadata = fixture["helper"]
        lookup_id = fixture["helper_lookup_id"]
    elif selected == fixture["reference"]:
        metadata = fixture["engine"]
        lookup_id = fixture["engine_lookup_id"]
    else:
        reject()
    if fmt == "{{.Id}}":
        print(lookup_id)
    elif fmt == "{{json .}}":
        print(json.dumps(metadata))
    else:
        reject()
else:
    reject()
'''


class PrepareOpenSearchTestImagesTests(unittest.TestCase):
    bash_path: str
    source_root: pathlib.Path
    source_script: pathlib.Path
    source_reference: pathlib.Path
    HELPER_ID = "sha256:" + "1" * 64
    ENGINE_ID = "sha256:" + "2" * 64
    OTHER_ID = "sha256:" + "3" * 64
    REPOSITORY = "opensearchproject/opensearch"
    COMMIT_SHA = "0123456789abcdef0123456789abcdef01234567"

    @classmethod
    def setUpClass(cls):
        bash_path = shutil.which("bash")
        if bash_path is None:
            raise RuntimeError("bash is required for the preparation-script regression")
        cls.bash_path = bash_path
        cls.source_root = pathlib.Path(__file__).resolve().parents[2]
        cls.source_script = cls.source_root / ".github/scripts/prepare-opensearch-test-images.sh"
        cls.source_reference = cls.source_root / "deploy/compose/opensearch/image.ref"

    def run_script(self, case):
        with tempfile.TemporaryDirectory(prefix="aura-prepare-opensearch-") as temporary:
            root = pathlib.Path(temporary)
            stub_bin = root / "bin"
            state = root / "state"
            stub_bin.mkdir(mode=0o700)
            state.mkdir(mode=0o700)
            copied_script = root / ".github/scripts/prepare-opensearch-test-images.sh"
            copied_reference = root / "deploy/compose/opensearch/image.ref"
            copied_script.parent.mkdir(mode=0o700, parents=True)
            copied_reference.parent.mkdir(mode=0o700, parents=True)
            shutil.copy2(self.source_script, copied_script)
            shutil.copy2(self.source_reference, copied_reference)
            (root / "deploy/images").mkdir(mode=0o700, parents=True)
            (root / "deploy/images/Dockerfile").write_text("FROM scratch\n")

            valid_reference = copied_reference.read_text(encoding="ascii").strip()
            digest = valid_reference.rsplit("@", 1)[1]
            selected_reference = case.get("reference", valid_reference)
            if selected_reference != valid_reference:
                copied_reference.write_text(selected_reference + "\n", encoding="ascii")

            helper = {
                "Id": case.get("helper_metadata_id", self.HELPER_ID),
                "Os": "linux",
                "Architecture": "amd64",
                "Config": {
                    "User": case.get("helper_user", "10001:10001"),
                    "Entrypoint": case.get("helper_entrypoint", ["/usr/bin/python3"]),
                    "Env": ["HOME=/home/aura"],
                },
            }
            engine = {
                "Id": case.get("engine_metadata_id", self.ENGINE_ID),
                "Os": case.get("engine_os", "linux"),
                "Architecture": case.get("engine_architecture", "amd64"),
                "Config": {},
            }
            if not case.get("omit_repo_digests"):
                engine["RepoDigests"] = case.get(
                    "repo_digests", [self.REPOSITORY + "@" + digest]
                )
            fixture = {
                "reference": selected_reference,
                "helper_lookup_id": case.get("helper_lookup_id", self.HELPER_ID),
                "engine_lookup_id": case.get("engine_lookup_id", self.ENGINE_ID),
                "helper": helper,
                "engine": engine,
                "fail_build": case.get("fail_build", False),
                "fail_pull": case.get("fail_pull", False),
            }
            (state / "metadata.json").write_text(json.dumps(fixture), encoding="utf-8")
            stub = stub_bin / "docker"
            stub.write_text("#!" + sys.executable + "\n" + DOCKER_STUB, encoding="utf-8")
            stub.chmod(0o700)
            github_env = root / "github-env"
            github_env.write_text("SENTINEL=keep\n", encoding="utf-8")
            environment = {
                "PATH": str(stub_bin) + os.pathsep + os.defpath,
                "COMMIT_SHA": self.COMMIT_SHA,
                "RUNNER_TEMP": str(state),
                "GITHUB_ENV": str(github_env),
                "FAKE_DOCKER_STATE": str(state),
                "PYTHONDONTWRITEBYTECODE": "1",
            }
            result = subprocess.run(
                [self.bash_path, str(copied_script)],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=15,
                check=False,
            )
            commands = [json.loads(line) for line in (state / "commands.jsonl").read_text().splitlines()]
            return result, commands, github_env.read_text(encoding="utf-8"), valid_reference

    def test_script_validates_canonical_engine_identity_and_failure_paths(self):
        reference = self.source_reference.read_text(encoding="ascii").strip()
        digest = reference.rsplit("@", 1)[1]
        cases = (
            {"name": "canonical tagged input and tagless digest", "success": True},
            {"name": "missing repository digest", "omit_repo_digests": True},
            {"name": "empty repository digest", "repo_digests": []},
            {"name": "wrong digest", "repo_digests": [self.REPOSITORY + "@sha256:" + "4" * 64]},
            {"name": "wrong repository", "repo_digests": ["other/opensearch@" + digest]},
            {"name": "tagged repository digest only", "repo_digests": [reference]},
            {"name": "tag-only repository metadata", "repo_digests": [reference.rsplit("@", 1)[0]]},
            {"name": "arm64 engine", "engine_architecture": "arm64"},
            {"name": "non-linux engine", "engine_os": "windows"},
            {"name": "metadata identity disagreement", "engine_metadata_id": self.OTHER_ID},
            {"name": "invalid local identity", "engine_lookup_id": "sha256:" + "5" * 63},
            {"name": "invalid helper entrypoint", "helper_entrypoint": ["/bin/sh"]},
            {"name": "invalid checked-in reference", "reference": self.REPOSITORY + ":latest", "no_pull": True},
            {"name": "build failure", "fail_build": True, "no_pull": True},
            {"name": "pull failure", "fail_pull": True},
        )
        for case in cases:
            with self.subTest(case=case["name"]):
                result, commands, github_env, reference = self.run_script(case)
                if case.get("success"):
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(
                        github_env,
                        "SENTINEL=keep\n"
                        + "C2_HELPER_IMAGE=" + self.HELPER_ID + "\n"
                        + "C2_OPENSEARCH_IMAGE=" + self.ENGINE_ID + "\n",
                    )
                    self.assertEqual(len(commands), 7)
                    self.assertEqual(commands[0][0], "build")
                    self.assertEqual(commands[0][commands[0].index("--platform") + 1], "linux/amd64")
                    self.assertEqual(commands[0][commands[0].index("--target") + 1], "opensearch-test-helper")
                    self.assertEqual(
                        commands[0][commands[0].index("--build-arg") + 1],
                        "COMMIT_SHA=" + self.COMMIT_SHA,
                    )
                    self.assertEqual(commands[0][-1], ".")
                    self.assertEqual(
                        commands[3], ["pull", "--platform", "linux/amd64", reference]
                    )
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(github_env, "SENTINEL=keep\n")
                    if case.get("no_pull"):
                        self.assertFalse(any(command and command[0] == "pull" for command in commands))


if __name__ == "__main__":
    unittest.main()
