import copy
import pathlib
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from ci_inventory.catalog import build_catalog, check_manifest, tree_features
from ci_inventory.discovery import attach_suites, discover_doctests, listed_cases


class InventoryContract(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("#[test]\nfn example() {}\n")
        (self.root / "Cargo.toml").write_text('[package]\nname="root"\nversion="1.2.3"\n')
        self.package = {"id": "root", "name": "root", "version": "1.2.3", "features": {},
            "manifest_path": str(self.root / "Cargo.toml"), "targets": [{"name": "root", "kind": ["lib"],
            "src_path": str(self.root / "src/lib.rs"), "test": True, "doctest": True}]}
        self.metadata = {"workspace_members": ["root"], "packages": [self.package]}

    def test_root_package_and_doctest_are_inventoried_without_claiming_compiled_counts(self):
        catalog, _ = build_catalog(self.metadata, self.root)
        self.assertEqual(catalog["summary"]["packages"], 1)
        self.assertIsNone(catalog["summary"]["test_count"])
        self.assertFalse(catalog["packages"][0]["current_ci_eligible"])
        self.assertTrue(catalog["packages"][0]["targets"][0]["doctest"])

    def test_any_removed_added_unassigned_or_regated_target_requires_explicit_acceptance(self):
        _, expected = build_catalog(self.metadata, self.root)
        for mutation in ("remove_package", "remove_target", "unassign", "feature_gate", "disable_harness"):
            with self.subTest(mutation=mutation):
                manifest = copy.deepcopy(expected)
                targets = manifest["packages"]["root"]["targets"]
                if mutation == "remove_package":
                    manifest["packages"].clear()
                elif mutation == "remove_target":
                    targets.clear()
                elif mutation == "unassign":
                    targets[0]["lane"] = None
                elif mutation == "feature_gate":
                    targets[0]["required_features"] = ["new-gate"]
                else:
                    targets[0]["harness"] = False
                with self.assertRaises(ValueError):
                    check_manifest(manifest, expected)

    def test_custom_harness_is_read_from_manifest_instead_of_assumed_from_metadata(self):
        with (self.root / "Cargo.toml").open("a") as stream:
            stream.write("\n[lib]\nharness=false\n")
        catalog, _ = build_catalog(self.metadata, self.root)
        self.assertFalse(catalog["packages"][0]["targets"][0]["harness"])

    def test_source_signals_keep_fixture_global_ddl_cfg_and_parameterization_evidence(self):
        (self.root / "src/lib.rs").write_text('''#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
#[cfg(target_os = "linux")]
#[rstest]
#[case(1)]
static STATE: OnceLock<u8> = OnceLock::new();
fn sample() { get_postgres_client(); let sql = "CREATE PUBLICATION example"; }
''')
        catalog, _ = build_catalog(self.metadata, self.root)
        target = catalog["packages"][0]["targets"][0]
        kinds = {item["kind"] for item in target["source_signals"]["evidence"]}
        self.assertTrue({"aura_integration_test", "serial_test", "platform_conditional", "parameterized", "persistent_global", "cluster_global_ddl_replication"} <= kinds)
        self.assertEqual(target["lane"], "INFRASTRUCTURE")

    def test_discovery_fails_when_nextest_silently_omits_a_cargo_target(self):
        catalog, _ = build_catalog(self.metadata, self.root)
        with patch("ci_inventory.discovery.command", return_value="/rust/lib"):
            with self.assertRaisesRegex(ValueError, "disappeared"):
                attach_suites(catalog, {}, self.root, self.root)

    def test_ignored_case_parity_mismatch_fails_closed(self):
        catalog, _ = build_catalog(self.metadata, self.root)
        suite = {"package-name": "root", "binary-name": "root", "kind": "lib", "cwd": str(self.root),
                 "testcases": {"sample": {"ignored": True}}}
        with patch("ci_inventory.discovery.command", return_value="/rust/lib"), patch(
                "ci_inventory.discovery.list_harness", return_value=({"sample"}, set())):
            with self.assertRaisesRegex(ValueError, "parity mismatch"):
                attach_suites(catalog, {"root": suite}, self.root, self.root)

    def test_duplicate_crate_names_cannot_silently_remove_a_doctest_target(self):
        catalog, _ = build_catalog(self.metadata, self.root)
        duplicate = copy.deepcopy(catalog["packages"][0])
        duplicate["name"] = "another-package"
        duplicate["targets"][0]["id"] = "another-package::lib::root"
        catalog["packages"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "Ambiguous rustdoc"):
            discover_doctests(catalog, self.root, self.root)

    def test_both_complete_and_ignored_doctest_listings_require_every_target(self):
        for outputs in (("", ""), ("Doc-tests root\n", "")):
            with self.subTest(outputs=outputs):
                catalog, _ = build_catalog(self.metadata, self.root)
                processes = [types.SimpleNamespace(returncode=0, stdout=raw) for raw in outputs]
                with patch("ci_inventory.discovery.subprocess.run", side_effect=processes):
                    with self.assertRaisesRegex(ValueError, "targets disappeared"):
                        discover_doctests(catalog, self.root, self.root)

    def test_unknown_repeated_or_missing_doctest_headings_fail_closed(self):
        for raw in ("Doc-tests unknown\n", "Doc-tests root\nDoc-tests root\n", "example: test\n"):
            with self.subTest(raw=raw):
                catalog, _ = build_catalog(self.metadata, self.root)
                with patch("ci_inventory.discovery.subprocess.run", return_value=types.SimpleNamespace(returncode=0, stdout=raw)):
                    with self.assertRaises(ValueError):
                        discover_doctests(catalog, self.root, self.root)

    def test_doctest_ignored_cases_remain_part_of_complete_inventory(self):
        catalog, _ = build_catalog(self.metadata, self.root)
        processes = [types.SimpleNamespace(returncode=0, stdout="Doc-tests root\nexample: test\n") for _ in range(2)]
        with patch("ci_inventory.discovery.subprocess.run", side_effect=processes):
            discover_doctests(catalog, self.root, self.root)
        discovery = catalog["packages"][0]["targets"][0]["doctest_discovery"]
        self.assertEqual(discovery["cases"], ["example"])
        self.assertEqual(discovery["ignored_cases"], ["example"])
        self.assertEqual(discovery["ignored_count"], 1)

    def test_parameterized_names_are_not_collapsed_to_source_function_count(self):
        self.assertEqual(listed_cases("sample::case_1: test\nsample::case_2: test\n2 tests, 0 benchmarks"),
                         {"sample::case_1", "sample::case_2"})

    def test_cargo_tree_feature_duplicates_are_unioned_within_package_version(self):
        raw = "leaf v1.2.3 (/checkout)|default\nshared v2.0.0|a,b\nshared v2.0.0|a,c (*)\n"
        with patch("ci_inventory.catalog.command", return_value=raw):
            features = tree_features([], self.root, self.root / "features.txt")
        self.assertEqual(features["shared v2.0.0"], {"a", "b", "c"})


if __name__ == "__main__":
    unittest.main()
