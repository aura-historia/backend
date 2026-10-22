import contextlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
from ci_coverage import apply_branch_regions, apply_function_records, combine, parse_lcov, render
from ci_benchmark.lcov import coverage_totals
from ci_inventory.discovery import nextest_data
from ci_inventory.phases import coverage_environment

SPEC = importlib.util.spec_from_file_location("export_coverage", CI / "export-coverage.py")
EXPORTER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EXPORTER)


class NormalizedCoverage(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("fn choose() {\n    if true { 1 } else { 2 };\n}\n")
        self.report = self.root / "coverage.lcov"

    def test_repeated_instantiations_and_incorrect_llvm_summaries_are_recomputed(self):
        text = "SF:src/lib.rs\nFN:1,choose_a\nFN:1,choose_b\nFNDA:1,choose_a\nFNDA:1,choose_b\nDA:1,2\nDA:2,2\nDA:3,0\nBRDA:2,0,0,1\nBRDA:2,0,1,0\nFNF:1\nFNH:1\nLF:3\nLH:1\nBRF:2\nBRH:0\nend_of_record\n"
        sources = parse_lcov(text, self.root)
        data = {"data": [{"functions": [], "files": [{"filename": str(self.root / "src/lib.rs"), "branches": [
            [2, 8, 2, 12, 1, 0, 0, 0, 4], [2, 8, 2, 12, 0, 1, 0, 0, 4]]}]}]}
        apply_branch_regions(sources, data, self.root)
        self.report.write_text(render(sources))
        totals = coverage_totals(self.report)["identity_union"]
        self.assertEqual((totals["lines_found"], totals["lines_hit"]), (3, 2))
        self.assertEqual((totals["branches_found"], totals["branches_hit"]), (2, 2))
        self.assertEqual(totals["functions_found"], 2)
        self.assertIn("BRDA:2,0,17,1", self.report.read_text())

    def test_source_union_keeps_unexecuted_branch_zero_and_one_record(self):
        first = parse_lcov("SF:src/lib.rs\nDA:2,1\nBRDA:2,8,0,1\nBRDA:2,8,1,0\nend_of_record\n", self.root)
        second = parse_lcov(f"SF:{self.root}/src/lib.rs\nDA:2,0\nBRDA:2,8,0,0\nBRDA:2,8,1,0\nend_of_record\n", self.root)
        combine(first, second)
        self.report.write_text(render(first))
        self.assertEqual(self.report.read_text().count("SF:"), 1)
        self.assertIn("BRDA:2,8,1,0", self.report.read_text())
        self.assertEqual(coverage_totals(self.report)["identity_union"]["branches_hit"], 1)

    def test_json_functions_retain_working_lcov_names_locations_and_counts(self):
        sources = parse_lcov("SF:src/lib.rs\nFN:1,choose\nFNDA:7,choose\nDA:2,7\nend_of_record\n", self.root)
        before = render(sources)
        data = {"data": [{"functions": [{"name": "choose", "count": 7, "filenames": ["src/lib.rs"],
                 "regions": [[1, 1, 3, 2, 7, 0, 0, 0]], "branches": []}]}]}
        apply_function_records(sources, data, self.root)
        self.assertEqual(render(sources), before)

    def test_branch_only_function_keeps_json_counter_and_anchors_to_existing_condition(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:2,3\nend_of_record\n", self.root)
        function = {"name": "choose", "count": 3, "filenames": ["src/lib.rs"], "regions": [],
                    "branches": [[2, 8, 2, 12, 1, 2, 0, 0, 4]]}
        apply_function_records(sources, {"data": [{"functions": [function]}]}, self.root)
        self.assertEqual(sources["src/lib.rs"]["definitions"], {"choose": 2})
        self.assertEqual(sources["src/lib.rs"]["functions"], {"choose": 3})
        for branches in ([[4, 8, 4, 12, 0, 0, 0, 0, 4]], [[2, 0, 2, 12, 0, 0, 0, 0, 4]],
                         [[2, 999, 2, 12, 0, 0, 0, 0, 4]], [[2, 8, 2, 12, 0, 0, 0, 0, 4], [4, 8, 4, 12, 0, 0, 0, 0, 4]],
                         [[2, 8, 2, 12, 0, 0, 1, 0, 4]]):
            function["branches"] = branches
            function["filenames"] = ["src/lib.rs", "/registry/other.rs"]
            with self.subTest(branches=branches), self.assertRaises(ValueError):
                apply_function_records(sources, {"data": [{"functions": [function]}]}, self.root)

    def test_unmapped_zero_count_functions_are_reported_and_executed_unmapped_functions_fail(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nend_of_record\n", self.root)
        function = {"name": "unmapped", "count": 0, "filenames": ["src/lib.rs"], "regions": [], "branches": []}
        before = render(sources)
        unmapped = apply_function_records(sources, {"data": [{"functions": [function]}]}, self.root)
        self.assertEqual(render(sources), before)
        self.assertEqual(unmapped, [{"name": "unmapped", "primary_filename": "src/lib.rs", "count": 0,
                                    "reason": "NO_CODE_OR_BRANCH_REGIONS"}])
        function["count"] = 1
        with self.assertRaisesRegex(ValueError, "Executed LLVM function has no source anchor"):
            apply_function_records(sources, {"data": [{"functions": [function]}]}, self.root)

    def test_source_resolution_is_refreshed_between_exports(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:1,1\nend_of_record\n", self.root)
        with tempfile.TemporaryDirectory() as temporary:
            external = pathlib.Path(temporary) / "lib.rs"
            external.write_text("fn external() {}\n")
            function = {"name": "choose", "count": 7, "filenames": [str(external)],
                        "regions": [[1, 1, 3, 2, 7, 0, 0, 0]], "branches": []}
            data = {"data": [{"functions": [function]}]}
            apply_function_records(sources, data, self.root)
            self.assertEqual(sources["src/lib.rs"]["functions"], {})
            external.unlink()
            external.symlink_to(self.root / "src/lib.rs")
            apply_function_records(sources, data, self.root)
            self.assertEqual(sources["src/lib.rs"]["functions"], {"choose": 7})

    def test_missing_json_sources_cannot_erase_existing_branch_coverage(self):
        for data in ({"data": []}, {"data": [{"functions": [], "files": []}]}):
            sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nBRDA:2,0,16,1\nend_of_record\n", self.root)
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, "source records differ"):
                apply_branch_regions(sources, data, self.root)
        sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nBRDA:2,0,16,1\nend_of_record\n", self.root)
        data = {"data": [{"functions": [], "files": [{"filename": "src/lib.rs", "branches": []}]}]}
        with self.assertRaisesRegex(ValueError, "omitted LCOV branch coverage"):
            apply_branch_regions(sources, data, self.root)

    def test_ambiguous_branch_spans_fail_instead_of_merging_different_conditions(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nend_of_record\n", self.root)
        data = {"data": [{"functions": [], "files": [{"filename": "src/lib.rs", "branches": [
            [2, 8, 2, 12, 1, 0, 0, 0, 4], [2, 8, 2, 15, 0, 1, 0, 0, 4]]}]}]}
        with self.assertRaisesRegex(ValueError, "Ambiguous"):
            apply_branch_regions(sources, data, self.root)

    def test_tracing_macro_evaluation_paths_union_the_same_user_condition(self):
        source = 'tracing::info!(outcome = if persisted.outcome.changed() { "CHANGED" } else { "UNCHANGED" });'
        (self.root / "src/lib.rs").write_text(source + "\n")
        column = source.index("persisted.outcome.changed()") + 1
        branches = [[1, column, 1, column + 27, 1, 0, 0, 0, 4],
                    [1, column, 1, column + 27, 0, 1, 0, 0, 4]]
        sources = parse_lcov("SF:src/lib.rs\nDA:1,2\nend_of_record\n", self.root)
        function = {"filenames": ["src/lib.rs"], "branches": branches}
        data = {"data": [{"functions": [function], "files": [{"filename": "src/lib.rs", "branches": branches}]}]}
        apply_branch_regions(sources, data, self.root)
        self.assertEqual(sources["src/lib.rs"]["branches"], {(1, 0, 2 * column): 1, (1, 0, 2 * column + 1): 1})
        branches[1][3] += 1
        with self.assertRaisesRegex(ValueError, "within a function"):
            apply_branch_regions(sources, data, self.root)
        branches[1][3] -= 1
        branches[1][6] = 1
        with self.assertRaisesRegex(ValueError, "file mapping"):
            apply_branch_regions(sources, data, self.root)

    def test_external_sources_are_omitted_but_missing_or_invalid_repository_sources_fail(self):
        text = "SF:/rustc/library/std.rs\nDA:999999,1\nend_of_record\nSF:src/lib.rs\nDA:1,1\nend_of_record\n"
        self.assertEqual(set(parse_lcov(text, self.root)), {"src/lib.rs"})
        for content in ("", "SF:src/missing.rs\nend_of_record\n", "SF:src/lib.rs\nDA:4,1\nend_of_record\n",
                        "SF:src/lib.rs\nDA:1,1\n", "SF:src/lib.rs\nDA:0,1\nend_of_record\n"):
            with self.subTest(content=content), self.assertRaises(ValueError):
                parse_lcov(content, self.root)


class LLVMDiagnostics(unittest.TestCase):
    def test_stderr_is_relayed_without_changing_stdout_or_failure_status(self):
        with tempfile.TemporaryFile(mode="w+") as output:
            for status in (0, 37):
                with self.subTest(status=status), contextlib.redirect_stderr(io.StringIO()) as errors:
                    argv = [sys.executable, "-c", "import sys; print('coverage output'); "
                            f"print('LLVM diagnostic', file=sys.stderr); sys.exit({status})"]
                    if status:
                        with self.assertRaises(subprocess.CalledProcessError) as failure:
                            EXPORTER.run_llvm(argv, output)
                        self.assertEqual(failure.exception.returncode, status)
                        self.assertEqual(failure.exception.stderr, "LLVM diagnostic\n")
                    else:
                        self.assertEqual(EXPORTER.run_llvm(argv, output), "LLVM diagnostic\n")
                    self.assertEqual(errors.getvalue(), "LLVM diagnostic\n")
            output.seek(0)
            self.assertEqual(output.read(), "coverage output\ncoverage output\n")

    def test_failed_diagnostic_report_retains_its_status_without_replacing_export_outcome(self):
        argv = ["llvm-cov", "export", "first-object", "--object=second-object", "--instr-profile=merged.profdata"]
        error = subprocess.CalledProcessError(37, argv, stderr="LLVM report failed\n")
        with patch.object(EXPORTER, "run_llvm", side_effect=error) as run:
            diagnostic = EXPORTER.report_mismatches(argv)
        run.assert_called_once_with(["llvm-cov", "report", "--dump", "--num-threads=1", *argv[2:]], subprocess.DEVNULL)
        self.assertEqual(diagnostic, {"exit_code": 37, "stderr": "LLVM report failed\n"})

    def test_diagnostic_launch_failure_is_reported_without_replacing_export_outcome(self):
        with patch.object(EXPORTER, "run_llvm", side_effect=OSError("diagnostic denied")), \
                contextlib.redirect_stderr(io.StringIO()) as errors:
            diagnostic = EXPORTER.report_mismatches(["llvm-cov", "export", "object"])
        self.assertEqual(diagnostic, {"exit_code": None, "stderr": None, "error": "diagnostic denied"})
        self.assertIn("coverage mismatch diagnostics: diagnostic denied", errors.getvalue())


class CompiledCoverageObjects(unittest.TestCase):
    def test_objects_follow_actual_metadata_and_missing_executable_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            unit, child, supervisor = [root / name for name in ("build-out-test", "child-probe", "supervisor")]
            for path in (unit, child, supervisor):
                path.write_bytes(b"\x7fELFfixture")
            doctest = root / "doctests" / "crate" / "generated-name"
            doctest.parent.mkdir(parents=True)
            doctest.write_bytes(b"\x7fELFfixture")
            doctest.chmod(0o755)
            nextest = root / "nextest.json"
            nextest.write_text(json.dumps({"rust-suites": {"unit": {"binary-path": str(unit)}}, "rust-build-meta": {
                "target-directory": str(root), "non-test-binaries": {"pkg": [{"kind": "bin-exe", "path": child.name}]}}}))
            cargo = root / "cargo.jsonl"
            cargo.write_text("\n".join(map(json.dumps, [
                {"reason": "compiler-artifact", "executable": str(child)},
                {"reason": "compiler-artifact", "executable": str(supervisor)},
                {"reason": "build-finished", "success": True}])) + "\n")
            self.assertEqual(EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root),
                             sorted([unit, child, supervisor, doctest]))
            profiles = root / "profiles"
            (profiles / "discovery").mkdir(parents=True)
            (profiles / "discovery/list.profraw").write_bytes(b"listing is not execution")
            with self.assertRaisesRegex(ValueError, "execution coverage profiles"):
                EXPORTER.export(profiles, nextest, [cargo], root / "doctests", root / "out.lcov", root)
            executed = profiles / "managed/test.profraw"
            executed.parent.mkdir()
            executed.write_bytes(b"test execution")
            raw, execution = EXPORTER.coverage_profiles(profiles)
            self.assertEqual(raw, sorted([profiles / "discovery/list.profraw", executed]))
            self.assertEqual(execution, [executed])
            (profiles / "empty.profraw").touch()
            with self.assertRaisesRegex(ValueError, "execution coverage profiles"):
                EXPORTER.export(profiles, nextest, [cargo], root / "doctests", root / "out.lcov", root)
            self.assertFalse((root / "out.lcov").exists())
            child.unlink()
            with self.assertRaisesRegex(ValueError, "object is missing"):
                EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root)

    def test_nextest_dylib_metadata_does_not_select_dependency_build_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            unit = root / "test-executable"
            unit.write_bytes(b"\x7fELFfixture")
            outputs = [root / f"debug/build/crc-fast/97dcba175dab6f85/out/libcrc_fast{suffix}"
                       for suffix in (".a", ".rlib", ".rmeta", ".so")]
            outputs[0].parent.mkdir(parents=True)
            for path in outputs:
                path.write_bytes(b"\x7fELFfixture")
            package = "registry+https://github.com/rust-lang/crates.io-index#crc-fast@1.10.0"
            nextest = root / "nextest.json"
            nextest.write_text(json.dumps({"rust-suites": {"unit": {"binary-path": str(unit)}}, "rust-build-meta": {
                "target-directory": str(root), "non-test-binaries": {package: [
                    {"name": "crc_fast", "kind": "dylib", "build-platform": "target",
                     "path": str(path.relative_to(root))} for path in outputs]}}}))
            cargo = root / "compile.jsonl"
            cargo.write_text("\n".join(map(json.dumps, [
                {"reason": "compiler-artifact", "executable": str(unit)},
                {"reason": "compiler-artifact", "package_id": package, "executable": None,
                 "target": {"kind": ["lib", "cdylib", "staticlib"],
                            "src_path": "/registry/crc-fast/src/lib.rs"}, "filenames": list(map(str, outputs))},
                {"reason": "build-finished", "success": True}])) + "\n")
            self.assertEqual(EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root), [unit])

    def test_required_cargo_executable_is_retained_regardless_of_suffix_or_contents(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            binary = root / "runtime.rmeta"
            binary.write_bytes(b"invalid object must reach LLVM")
            nextest = root / "nextest.json"
            nextest.write_text(json.dumps({"rust-suites": {}, "rust-build-meta": {"non-test-binaries": {}}}))
            cargo = root / "compile.jsonl"
            cargo.write_text(json.dumps({"reason": "compiler-artifact", "executable": str(binary)}) +
                             '\n{"reason":"build-finished","success":true}\n')
            self.assertEqual(EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root), [binary])
            binary.unlink()
            with self.assertRaisesRegex(ValueError, "object is missing"):
                EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root)

    def test_recorded_repository_proc_macro_shared_objects_are_retained_and_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / "src/macros/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("fn expand() {}\n")
            library = root / "actual-cargo-output.so"
            library.write_bytes(b"\x7fELFfixture")
            nextest = root / "nextest.json"
            data = {"rust-suites": {}, "rust-build-meta": {"non-test-binaries": {}}}
            nextest.write_text(json.dumps(data))
            artifact = {"reason": "compiler-artifact", "executable": None, "filenames": [str(library)],
                        "target": {"kind": ["proc-macro"], "src_path": str(source)}}
            external = dict(artifact, target={"kind": ["proc-macro"], "src_path": "/registry/dependency/lib.rs"},
                            filenames=[str(root / "unselected-external.so")])
            cargo = root / "compile.jsonl"
            cargo.write_text("\n".join(map(json.dumps, [artifact, external, {"reason": "build-finished", "success": True}, data])) + "\n")
            self.assertEqual(EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root), [library])
            library.unlink()
            with self.assertRaisesRegex(ValueError, "object is missing"):
                EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root)
            artifact["filenames"] = []
            cargo.write_text(json.dumps(artifact) + '\n{"reason":"build-finished","success":true}\n')
            with self.assertRaisesRegex(ValueError, "no recorded shared object"):
                EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root)

    def test_cargo_metadata_requires_one_successful_build_before_selecting_objects(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            binary = root / "test-executable"
            binary.write_bytes(b"\x7fELFfixture")
            nextest = root / "nextest.json"
            nextest.write_text(json.dumps({"rust-suites": {"unit": {"binary-path": str(binary)}},
                                          "rust-build-meta": {"non-test-binaries": {}}}))
            cargo = root / "compile.jsonl"
            artifact = {"reason": "compiler-artifact", "executable": str(binary)}
            finished = {"reason": "build-finished", "success": True}
            for messages in ([], [artifact], [artifact, dict(finished, success=False)],
                             [finished], [artifact, finished, finished]):
                with self.subTest(messages=messages):
                    cargo.write_text("\n".join(map(json.dumps, messages)))
                    with self.assertRaisesRegex(ValueError, "metadata is incomplete"):
                        EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root)
            with self.assertRaisesRegex(ValueError, "No Cargo coverage artifact metadata"):
                EXPORTER.coverage_objects(nextest, [], root / "doctests", root)
            cargo.write_text("\n".join(map(json.dumps, [artifact, finished])))
            self.assertEqual(EXPORTER.coverage_objects(nextest, [cargo], root / "doctests", root), [binary])

    def test_forwarded_cargo_messages_keep_exactly_one_nextest_report(self):
        data = {"rust-suites": {}, "rust-build-meta": {}}
        cargo = {"reason": "compiler-artifact", "executable": None}
        raw = "\n".join(map(json.dumps, [cargo, {"reason": "build-finished", "success": True}, data])) + "\n"
        self.assertEqual(nextest_data(raw), data)
        self.assertEqual(nextest_data(json.dumps(data)), data)
        for invalid in ("", json.dumps(cargo), json.dumps(data) + "\n" + json.dumps(data), "{}", "broken"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                nextest_data(invalid)

    def test_phase_profiles_preserve_credentials_and_subprocess_inheritance(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = {"AURA_CI_COVERAGE_DIR": temporary, "AWS_ACCESS_KEY_ID": "test", "RUSTFLAGS": "-C instrument-coverage"}
            managed = coverage_environment(base, "managed")
            doctests = coverage_environment(managed, "doctests")
            self.assertNotEqual(managed["LLVM_PROFILE_FILE"], doctests["LLVM_PROFILE_FILE"])
            self.assertEqual(pathlib.Path(managed["LLVM_PROFILE_FILE"]).name, "%b-%4m.profraw")
            self.assertEqual(pathlib.Path(doctests["LLVM_PROFILE_FILE"]).name, "%b-%4m.profraw")
            self.assertEqual(managed["AWS_ACCESS_KEY_ID"], "test")
            self.assertEqual(managed["RUSTFLAGS"], doctests["RUSTFLAGS"])
            self.assertTrue(pathlib.Path(doctests["LLVM_PROFILE_FILE"]).parent.is_dir())
            self.assertEqual(coverage_environment({"LLVM_PROFILE_FILE": "old-%p.profraw"}, "managed"),
                             {"LLVM_PROFILE_FILE": "old-%p.profraw"})


if __name__ == "__main__":
    unittest.main()
