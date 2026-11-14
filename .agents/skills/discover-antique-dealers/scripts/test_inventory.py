"""Single-location dealer inventory policy decisions and readback; no network."""

import unittest

from unittest.mock import patch

from inventory import POLICY_EVIDENCE, POLICY_REFERENCE, apply, decide, read

EXPLICIT = {"reference": "https://dealer.example/showroom", "scope": "ADVERTISED_INVENTORY"}


def default(location="ploc_a", inventory=None, eligibility="ACTIVE"):
    assignment = {"partyLocationId": location} | ({"advertisedInventory": inventory} if inventory else {})
    return {"revision": 2, "eligibility": eligibility, "assignment": assignment}


class DecideTest(unittest.TestCase):
    def test_should_set_policy_for_dealer_with_single_selected_location(self):
        self.assertEqual(("SET", "SINGLE_LOCATION_DEALER"), decide("DEALER", ["ploc_a"], default()))

    def test_should_skip_dealer_with_several_active_locations(self):
        self.assertEqual((None, "MULTIPLE_LOCATIONS"), decide("DEALER", ["ploc_a", "ploc_b"], default()))

    def test_should_skip_non_dealer_sources(self):
        self.assertEqual((None, "NOT_DEALER"), decide("AUCTION_HOUSE", ["ploc_a"], default()))

    def test_should_skip_sources_without_default(self):
        self.assertEqual((None, "NO_DEFAULT"), decide("DEALER", ["ploc_a"], {"revision": 1}))

    def test_should_skip_retired_default(self):
        self.assertEqual((None, "RETIRED_DEFAULT"),
                         decide("DEALER", ["ploc_b"], default(eligibility="RETIRED")))

    def test_should_skip_default_selecting_another_location(self):
        self.assertEqual((None, "DEFAULT_SELECTS_OTHER_LOCATION"), decide("DEALER", ["ploc_b"], default()))

    def test_should_keep_current_policy_assertion(self):
        self.assertEqual((None, "POLICY_CURRENT"),
                         decide("DEALER", ["ploc_a"], default(inventory=POLICY_EVIDENCE)))

    def test_should_clear_policy_assertion_once_operator_has_another_location(self):
        self.assertEqual(("CLEAR", "POLICY_NO_LONGER_APPLIES"),
                         decide("DEALER", ["ploc_a", "ploc_b"], default(inventory=POLICY_EVIDENCE)))

    def test_should_clear_policy_assertion_for_non_dealer(self):
        self.assertEqual(("CLEAR", "POLICY_NO_LONGER_APPLIES"),
                         decide("MARKETPLACE", ["ploc_a"], default(inventory=POLICY_EVIDENCE)))

    def test_should_leave_policy_assertion_on_retired_reference(self):
        self.assertEqual((None, "POLICY_ON_RETIRED_REFERENCE"),
                         decide("DEALER", [], default(inventory=POLICY_EVIDENCE, eligibility="RETIRED")))

    def test_should_never_change_explicit_evidence(self):
        for locations in (["ploc_a"], ["ploc_a", "ploc_b"], []):
            self.assertEqual((None, "EXPLICIT_EVIDENCE"),
                             decide("DEALER", locations, default(inventory=EXPLICIT)))


class FakeApi:
    def __init__(self, stored):
        self.stored, self.requests = stored, []

    def request(self, method, path, body=None):
        self.requests.append((method, path, body))
        if method == "PATCH":
            return None
        return {"revision": 3, "eligibility": "ACTIVE",
                "assignment": {"partyLocationId": "ploc_a"} | (
                    {"advertisedInventory": self.stored} if self.stored else {})}


class ApplyTest(unittest.TestCase):
    def test_should_patch_only_inventory_with_expected_revision(self):
        api = FakeApi(POLICY_EVIDENCE)
        apply(api, {"listingSourceId": "ls_a", "action": "SET", "revision": 2})
        self.assertEqual(("PATCH", "/v1/admin/listing-sources/ls_a/location-default",
                          {"expectedRevision": 2, "advertisedInventory": POLICY_EVIDENCE}), api.requests[0])
        self.assertEqual(POLICY_REFERENCE, api.requests[0][2]["advertisedInventory"]["reference"])

    def test_should_remove_policy_with_null(self):
        api = FakeApi(None)
        apply(api, {"listingSourceId": "ls_a", "action": "CLEAR", "revision": 4})
        self.assertIsNone(api.requests[0][2]["advertisedInventory"])

    def test_should_fail_when_readback_differs(self):
        with self.assertRaisesRegex(ValueError, "readback"):
            apply(FakeApi(None), {"listingSourceId": "ls_a", "action": "SET", "revision": 2})


class FlakyApi:
    def __init__(self, failures, code=503):
        self.failures, self.code, self.calls = failures, code, 0

    def request(self, method, path, body=None):
        self.calls += 1
        if self.calls <= self.failures:
            raise RuntimeError(f"Aura API returned HTTP {self.code}; reconcile before retrying writes")
        return {"ok": True}


@patch("inventory.time.sleep")
class ReadTest(unittest.TestCase):
    def test_should_retry_temporary_unavailability(self, _sleep):
        api = FlakyApi(2)
        self.assertEqual({"ok": True}, read(api, "/v1/x"))
        self.assertEqual(3, api.calls)

    def test_should_give_up_after_bounded_attempts(self, _sleep):
        with self.assertRaisesRegex(RuntimeError, "503"):
            read(FlakyApi(9), "/v1/x")

    def test_should_not_retry_other_errors(self, _sleep):
        api = FlakyApi(1, code=403)
        with self.assertRaisesRegex(RuntimeError, "403"):
            read(api, "/v1/x")
        self.assertEqual(1, api.calls)


if __name__ == "__main__":
    unittest.main()
