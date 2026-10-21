import copy
import json
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import inventory
from ci_inventory.catalog import LANES, lane_selection, package_owners


def target(package, kind, name, lane, doctest=False):
    return {"id": f"{package}::{kind}::{name}", "lane": lane, "test": True,
            "doctest": doctest, "harness": True, "required_features": []}


class PackageLaneSelection(unittest.TestCase):
    def setUp(self):
        self.manifest = {"schema_version": 1, "purpose": "INVENTORY_ONLY", "lanes": copy.deepcopy(LANES),
            "packages": {
                "aura-historia-parent": {"path": ".", "targets": [
                    target("aura-historia-parent", "lib", "aura_historia_parent", "REMAINING", True)]},
                "test-api-macros": {"path": "src/test-api/src/test-api-macros", "targets": [
                    target("test-api-macros", "proc-macro", "test_api_macros", "INFRASTRUCTURE", True)]},
                "aura-historia-api": {"path": "src/aura-historia-api", "targets": [
                    target("aura-historia-api", "lib", "aura_historia_api", "API", True)]},
                "crawler": {"path": "src/crawler", "targets": [
                    target("crawler", "lib", "crawler", "CRAWLER", True)]},
                "cognito-post-confirmation": {"path": "src/cognito-post-confirmation", "targets": [
                    target("cognito-post-confirmation", "lib", "cognito_post_confirmation", "REMAINING", True),
                    target("cognito-post-confirmation", "bin", "cognito-post-confirmation", "REMAINING"),
                    target("cognito-post-confirmation", "test", "lib", "INFRASTRUCTURE")]},
            }}
        self.expected = copy.deepcopy(self.manifest)

    def test_all_packages_and_targets_have_exactly_one_execution_owner(self):
        selections = [lane_selection(self.manifest, self.expected, lane) for lane in LANES]
        packages = [name for selection in selections for name in selection["packages"]]
        targets = [identity for selection in selections for identity in selection["targets"]]
        self.assertCountEqual(packages, self.manifest["packages"])
        self.assertCountEqual(targets, [item["id"] for package in self.manifest["packages"].values()
                                        for item in package["targets"]])

    def test_root_and_nested_proc_macro_keep_package_selectors_and_doctests(self):
        remaining = lane_selection(self.manifest, self.expected, "REMAINING")
        infrastructure = lane_selection(self.manifest, self.expected, "INFRASTRUCTURE")
        self.assertEqual(remaining["package_args"], ["--package", "aura-historia-parent"])
        self.assertEqual(remaining["doctest_targets"], ["aura-historia-parent::lib::aura_historia_parent"])
        self.assertIn("test-api-macros", infrastructure["packages"])
        self.assertIn("test-api-macros::proc-macro::test_api_macros", infrastructure["doctest_targets"])

    def test_mixed_cognito_package_is_promoted_whole_without_changing_source_classification(self):
        original = copy.deepcopy(self.manifest)
        selection = lane_selection(self.manifest, self.expected, "INFRASTRUCTURE")
        cognito = self.manifest["packages"]["cognito-post-confirmation"]
        self.assertTrue({item["id"] for item in cognito["targets"]} <= set(selection["targets"]))
        self.assertNotIn("cognito-post-confirmation", lane_selection(self.manifest, self.expected, "REMAINING")["packages"])
        self.assertEqual(self.manifest, original)

    def test_selection_order_does_not_depend_on_package_or_target_order(self):
        reordered = copy.deepcopy(self.manifest)
        reordered["packages"] = dict(reversed(list(reordered["packages"].items())))
        for package in reordered["packages"].values():
            package["targets"].reverse()
        self.assertEqual(lane_selection(reordered, reordered, "INFRASTRUCTURE"),
                         lane_selection(self.manifest, self.expected, "INFRASTRUCTURE"))

    def test_new_missing_duplicated_or_regated_ownership_requires_reviewed_manifest(self):
        for mutation in ("new_package", "missing_package", "new_target", "missing_target", "duplicate_target",
                         "required_features", "test", "doctest", "harness", "lane"):
            with self.subTest(mutation=mutation):
                manifest, expected = copy.deepcopy(self.manifest), copy.deepcopy(self.expected)
                package = manifest["packages"]["aura-historia-parent"]
                if mutation == "new_package":
                    expected["packages"]["new-package"] = {"path": "src/new-package", "targets": [
                        target("new-package", "lib", "new_package", "REMAINING")]}
                elif mutation == "missing_package":
                    del manifest["packages"]["aura-historia-parent"]
                elif mutation == "new_target":
                    expected["packages"]["aura-historia-parent"]["targets"].append(
                        target("aura-historia-parent", "test", "new_test", "REMAINING"))
                elif mutation == "missing_target":
                    package["targets"].clear()
                elif mutation == "duplicate_target":
                    package["targets"].append(copy.deepcopy(package["targets"][0]))
                elif mutation == "required_features":
                    package["targets"][0][mutation] = ["new-feature"]
                elif mutation == "lane":
                    package["targets"][0][mutation] = "INFRASTRUCTURE"
                else:
                    package["targets"][0][mutation] = not package["targets"][0][mutation]
                with self.assertRaisesRegex(ValueError, "contract changed"):
                    lane_selection(manifest, expected, "API")

    def test_reviewed_new_package_joins_remaining_without_a_test_count_assumption(self):
        self.manifest["packages"]["new-package"] = {"path": "src/new-package", "targets": [
            target("new-package", "lib", "new_package", "REMAINING")]}
        selection = lane_selection(self.manifest, self.manifest, "REMAINING")
        self.assertEqual(selection["package_args"], ["--package", "aura-historia-parent", "--package", "new-package"])
        self.assertIn("new-package::lib::new_package", selection["targets"])

    def test_invalid_target_identity_is_rejected_even_in_matching_manifests(self):
        for identity in ("other::lib::root", "aura-historia-parent::lib::", "aura-historia-parent::root"):
            with self.subTest(identity=identity):
                manifest = copy.deepcopy(self.manifest)
                manifest["packages"]["aura-historia-parent"]["targets"][0]["id"] = identity
                with self.assertRaisesRegex(ValueError, "does not belong"):
                    package_owners(manifest, manifest)

    def test_duplicate_target_is_rejected_even_when_live_metadata_repeats_it(self):
        targets = self.manifest["packages"]["aura-historia-parent"]["targets"]
        targets.append(copy.deepcopy(targets[0]))
        with self.assertRaisesRegex(ValueError, "Duplicate target ownership"):
            package_owners(self.manifest, self.manifest)

    def test_unknown_or_misplaced_classifications_and_targetless_packages_are_rejected(self):
        for package, classification in (("aura-historia-parent", "UNKNOWN"), ("aura-historia-parent", "API"),
                                        ("aura-historia-api", "INFRASTRUCTURE"), ("crawler", "REMAINING")):
            with self.subTest(package=package, classification=classification):
                manifest = copy.deepcopy(self.manifest)
                manifest["packages"][package]["targets"][0]["lane"] = classification
                with self.assertRaisesRegex(ValueError, "classification"):
                    package_owners(manifest, manifest)
        self.manifest["packages"]["aura-historia-parent"]["targets"].clear()
        with self.assertRaisesRegex(ValueError, "no target ownership"):
            package_owners(self.manifest, self.manifest)

    def test_unknown_or_empty_lanes_cannot_emit_default_cargo_selection(self):
        with self.assertRaisesRegex(ValueError, "Unknown test lane"):
            lane_selection(self.manifest, self.expected, "UNKNOWN")
        del self.manifest["packages"]["aura-historia-api"]
        with self.assertRaisesRegex(ValueError, "no packages"):
            lane_selection(self.manifest, self.manifest, "API")

    def test_cli_writes_only_validated_selectors_without_accepting_manifest_changes(self):
        for valid in (True, False):
            with self.subTest(valid=valid), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                manifest = copy.deepcopy(self.manifest)
                if not valid:
                    del manifest["packages"]["crawler"]
                manifest_path = root / "test-lanes.json"
                original = json.dumps(manifest)
                manifest_path.write_text(original)
                output = root / "output"
                argv = ["inventory.py", "--output", str(output), "--manifest", str(manifest_path), "--lane", "INFRASTRUCTURE"]
                with patch.object(sys, "argv", argv), patch.object(inventory, "command", side_effect=[
                        str(root), "{}", "a" * 40, "", "test toolchain"]), patch.object(
                        inventory, "build_catalog", return_value=({"summary": {}}, self.expected)), patch("builtins.print"):
                    if valid:
                        inventory.main()
                        self.assertEqual(json.loads((output / "lane-selection.json").read_text()),
                                         lane_selection(self.manifest, self.expected, "INFRASTRUCTURE"))
                    else:
                        with self.assertRaisesRegex(ValueError, "contract changed"):
                            inventory.main()
                        self.assertFalse((output / "lane-selection.json").exists())
                self.assertEqual(manifest_path.read_text(), original)


if __name__ == "__main__":
    unittest.main()
