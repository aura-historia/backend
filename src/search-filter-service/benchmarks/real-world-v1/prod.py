"""Opt-in production fixture lifecycle; no application code or configuration changes."""

import datetime
import json
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

from corpus import ROOT, estimate, local_path, measurement_summary, render_review, write_json

REGION = "eu-central-1"
BUCKET = "aura-historia-binary-artifacts-eu-central-1"
API_URL = "https://api.aura-historia.com"


class Api:
    def __init__(self, token_file):
        self.token_file = Path(token_file).resolve()
        if self.token_file.is_relative_to(ROOT.parents[3]):
            raise ValueError("Bearer files must be outside the repository")

    def call(self, method, path, body=None, expected=(200, 201, 204)):
        headers = {"Authorization": "Bearer " + self.token_file.read_text().strip(),
                   "Content-Type": "application/json", "X-Correlation-Id": "matcher-corpus-v1"}
        request = urllib.request.Request(API_URL + path, method=method, headers=headers,
                                         data=json.dumps(body).encode() if body is not None else None)
        try:
            with urllib.request.urlopen(request, timeout=35) as response:
                raw = response.read()
                result = json.loads(raw) if raw else None
                status = response.status
        except urllib.error.HTTPError as error:
            status, result = error.code, None
        except (OSError, TimeoutError):
            raise RuntimeError(f"API transport failed: {method} {path}; reconcile before retry") from None
        if status not in expected:
            raise RuntimeError(f"API rejected {method} {path}: {status}")
        return result


def aws(*args):
    result = subprocess.run(["aws", "--region", REGION, *args], capture_output=True, text=True)
    if result.returncode:
        # Never echo command arguments, stdout or provider errors containing signed URLs.
        raise RuntimeError(f"AWS {args[0]} {args[1]} failed (exit {result.returncode})")
    return json.loads(result.stdout) if result.stdout.strip() else None


def save_state(path, state):
    write_json(path / "state.json", state)


def private_workspace(path):
    path = Path(path).resolve()
    if path.is_relative_to(ROOT.parents[3]):
        raise ValueError("Live state/credentials must be outside the repository")
    path.mkdir(parents=True, exist_ok=False, mode=0o700)
    return path


def deployment():
    schedule = aws("scheduler", "get-schedule", "--name", "search-filter-periodic-match-prod",
                   "--group-name", "aura-historia-periodic-matcher-prod")
    arn = schedule["Target"]["EcsParameters"]["TaskDefinitionArn"]
    definition = aws("ecs", "describe-task-definition", "--task-definition", arn)["taskDefinition"]
    container = definition["containerDefinitions"][0]
    env = {x["name"]: x["value"] for x in container["environment"]}
    assert env["CLASSIFIER_MODEL"] == "clef-flash"
    assert env["PERIODIC_MATCH_BATCH_CANDIDATES"] == "1"
    assert env["SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS"] == "5000"
    return schedule, {"task_definition": arn, "image": container["image"],
                      "revision": env["AURA_HISTORIA_SOURCE_REVISION"],
                      "model": env["CLASSIFIER_MODEL"], "threshold": 0.5}


def create_source(api, workspace, state):
    source = api.call("POST", "/v1/admin/listing-sources", {
        "name": state["name"], "listingSourceType": "DEALER",
        "operator": {"type": "NEW", "name": state["name"]},
        "ingestionConfiguration": [{"type": "PARTNER_API"}]})
    state["source_id"] = source["listingSourceId"]
    save_state(workspace, state)
    application = api.call("POST", "/v1/me/partnership-applications", {
        "proposal": {"type": "EXISTING_LISTING_SOURCE", "listingSourceId": state["source_id"]}})
    state["application_id"] = application["id"]
    save_state(workspace, state)
    path = "/v1/admin/partnership-applications/" + application["id"]
    api.call("PATCH", path)
    approved = api.call("POST", path + "/decision", {"decision": "APPROVE"})
    state["partnership_id"] = approved["approvedPartnershipId"]
    save_state(workspace, state)


def host_assets(manifest, workspace, state):
    urls = {}
    for asset in manifest["assets"]:
        key = state["prefix"] + "/" + Path(asset["path"]).name
        response = aws("s3api", "put-object", "--bucket", BUCKET, "--key", key,
                       "--body", str(local_path(asset["path"])), "--content-type", "image/jpeg",
                       "--cache-control", "no-store")
        state["objects"].append({"key": key, "version": response.get("VersionId")})
        save_state(workspace, state)
        result = subprocess.run(["aws", "--region", REGION, "s3", "presign",
                                 f"s3://{BUCKET}/{key}", "--expires-in", "3600"],
                                capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError("Could not sign a temporary image read")
        urls[asset["path"]] = result.stdout.strip()
    return urls


def listing_directory(api, state):
    return api.call("GET", "/v1/admin/product-listings?listingSourceId="
                    + state["source_id"] + "&size=100")["items"]


def create_listings(api, manifest, urls, workspace, state):
    payload = [{"sourceListingId": c["id"], "title": {"text": c["title"], "language": "en"},
                "availability": "IN_STOCK", "images": [urls[p] for p in c["image_paths"]],
                "url": "https://matcher-corpus.example.invalid/" + c["id"]}
               for c in manifest["cases"]]
    failures = api.call("POST", "/v1/listing-sources/" + state["source_id"] + "/product-listings", payload)
    if failures:
        raise RuntimeError("Fixture batch contained rejected listings; cleanup will reconcile the source")
    state["listing_ids"] = {x["sourceListingId"]: x["productListingId"]
                            for x in listing_directory(api, state)}
    assert len(state["listing_ids"]) == len(manifest["cases"])
    save_state(workspace, state)


def create_filters(api, manifest, workspace, state):
    for group in manifest["groups"]:
        excluded = [state["listing_ids"][c["id"]] for c in manifest["cases"]
                    if c["group"] != group["id"]]
        search = dict(group["search"], listingSourceId=[state["source_id"]],
                      excludeProductListingId=excluded)
        result = api.call("POST", "/v1/me/search-filters", {
            "name": state["name"] + " " + group["id"], "notifications": False, "search": search})
        state["filters"][group["id"]] = result["userSearchFilterId"]
        save_state(workspace, state)
        assert set(result["search"]["excludeProductListingId"]) == set(excluded)
    # Description writes occur after filter creation, satisfying the matching freshness interval.
    patches = [{"sourceListingId": c["id"], "description": {"text": c["description"], "language": "en"}}
               for c in manifest["cases"]]
    failures = api.call("PATCH", "/v1/listing-sources/" + state["source_id"] + "/product-listings", patches)
    if failures:
        raise RuntimeError("Fixture description update had failures")


def await_projection(api, workspace, state):
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        canonical = {x["productListingId"]: x["updated"] for x in listing_directory(api, state)}
        projected = api.call("GET", "/v1/product-listings?listingSourceId="
                             + state["source_id"] + "&size=100")["items"]
        actual = {x["item"]["productListingId"]: x["item"]["updated"] for x in projected}
        if canonical == actual:
            save_state(workspace, state)
            return
        time.sleep(5)
    raise RuntimeError("Canonical/search freshness did not converge within four minutes")


def start_task(schedule, workspace, state):
    ecs = schedule["Target"]["EcsParameters"]
    network = ecs["NetworkConfiguration"]["awsvpcConfiguration"]
    overrides = {"PERIODIC_MATCH_PROJECTION_LAG_SECONDS": "0",
                 "PERIODIC_MATCH_MAX_RUN_SECONDS": "600",
                 "PERIODIC_MATCH_CLASSIFICATION_CONCURRENCY": "2",
                 "PERIODIC_MATCH_MAX_ATTEMPTS": "1"}
    request = {"cluster": schedule["Target"]["Arn"], "taskDefinition": ecs["TaskDefinitionArn"],
               "launchType": "FARGATE", "platformVersion": ecs["PlatformVersion"], "count": 1,
               "startedBy": "real-world-matcher-corpus",
               "networkConfiguration": {"awsvpcConfiguration": {
                   k[0].lower() + k[1:]: v for k, v in network.items()}},
               "overrides": {"containerOverrides": [{"name": "periodic-matcher",
                   "environment": [{"name": k, "value": v} for k, v in overrides.items()]}]}}
    write_json(workspace / "task-request.json", request)
    result = aws("ecs", "run-task", "--cli-input-json", "file://" + str(workspace / "task-request.json"))
    assert not result["failures"], "ECS rejected the bounded matcher invocation"
    state["task_arn"] = result["tasks"][0]["taskArn"]
    state["cluster"] = request["cluster"]
    state["overrides"] = overrides
    save_state(workspace, state)


def await_task(workspace, state):
    deadline = time.monotonic() + 780
    while time.monotonic() < deadline:
        task = aws("ecs", "describe-tasks", "--cluster", state["cluster"],
                   "--tasks", state["task_arn"])["tasks"][0]
        if task["lastStatus"] == "STOPPED":
            state["task_stopped"] = True
            save_state(workspace, state)
            return {"status": task["lastStatus"], "exit_code": task["containers"][0].get("exitCode"),
                    "image_digest": task["containers"][0].get("imageDigest")}
        time.sleep(10)
    raise RuntimeError("Matcher did not stop within the bounded observation period")


def read_events(state):
    stream = "periodic-matcher/periodic-matcher/" + state["task_arn"].rsplit("/", 1)[1]
    events, token = [], None
    for _ in range(30):
        args = ["logs", "get-log-events", "--log-group-name", "/aura-historia/prod/periodic-matcher",
                "--log-stream-name", stream, "--start-from-head"]
        if token:
            args.extend(["--next-token", token])
        result = aws(*args)
        for event in result["events"]:
            try:
                events.append(json.loads(event["message"]))
            except json.JSONDecodeError:
                pass
        if result["nextForwardToken"] == token:
            break
        token = result["nextForwardToken"]
    return events


def find_listing_id(value):
    if isinstance(value, dict):
        if "productListingId" in value:
            return value["productListingId"]
        for child in value.values():
            found = find_listing_id(child)
            if found:
                return found
    return None


def collect_report(api, manifest, state, deployed, task):
    events = read_events(state)
    fields = [e.get("fields", {}) for e in events]
    decisions = {f["correlationId"].split(":")[3]: f for f in fields
                 if f.get("eventType") == "SAVED_SEARCH_MATCH_DECISION"}
    costs = [f for f in fields if f.get("eventType") == "CLASSIFIER_INVOCATION"]
    matches = set()
    for fid in state["filters"].values():
        body = api.call("GET", "/v1/me/search-filters/" + fid + "/matches?size=100")
        matches.update(find_listing_id(item) for item in body["items"])
    rows = []
    for case in manifest["cases"]:
        pid = state["listing_ids"][case["id"]]
        decision = decisions.get(pid, {})
        requested = len(case["image_paths"])
        sent = decision.get("sentImageCount")
        score = decision.get("shouldShowScore")
        resolved = score is not None and sent == requested
        matched = pid in matches
        if decision.get("finalDecision") in ("matched", "rejected"):
            assert matched == (decision["finalDecision"] == "matched"), case["id"]
        rows.append({"id": case["id"], "group": case["group"],
                     "listing_id": pid, "filter_id": state["filters"][case["group"]],
                     "expected_match": case["expected_match"], "matched": matched,
                     "score": score, "requested_images": requested, "sent_images": sent,
                     "quality_resolved": resolved, "decision": decision.get("finalDecision"),
                     "omission_reason": decision.get("imageOmissionReason")})
    summaries = [f for f in fields if f.get("message") == "search_filter.periodic_match.completed"]
    tokens = sum(c.get("inputTokens", 0) for c in costs)
    return {"dataset_revision": manifest["dataset_revision"], "label_provenance": "agent-authored",
            "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "fixture_source_id": state["source_id"],
            "human_reviewed": False, "scope": manifest["scope"], "deployment": deployed,
            "task": {**task, "arn": state["task_arn"], "overrides": state["overrides"]},
            "summary": summaries[-1] if summaries else None, "cases": rows,
            **measurement_summary(manifest, rows),
            "usage": {"provider_calls": len(costs), "input_tokens": tokens,
                      "estimated_neurons": round(tokens * manifest["neurons_per_million_input_tokens"] / 1e6, 3)},
            "evidence_delivery_failures": sum(r["sent_images"] != r["requested_images"]
                                               for r in rows if r["score"] is not None)}


def cleanup(api, workspace, state):
    errors = []

    def attempt(label, action):
        try:
            return action()
        except Exception:
            errors.append(label)
            return None

    if state.get("task_arn") and not state.get("task_stopped"):
        attempt("stop matcher", lambda: aws("ecs", "stop-task", "--cluster", state["cluster"],
                                           "--task", state["task_arn"], "--reason", "Corpus cleanup"))
        attempt("await stopped matcher", lambda: await_task(workspace, state))
    for fid in state["filters"].values():
        attempt("delete filter " + fid, lambda fid=fid: api.call(
            "DELETE", "/v1/me/search-filters/" + fid, expected=(204, 404)))
    if state.get("source_id"):
        listings = attempt("read fixture directory", lambda: listing_directory(api, state))
        for item in listings or []:
            if item["lifecycle"] == "ACTIVE":
                attempt("withdraw " + item["productListingId"], lambda item=item: api.call(
                    "DELETE", "/v1/admin/product-listings/" + item["productListingId"]
                    + "?expectedVersion=" + str(item["expectedVersion"])))
        attempt("disable ingestion", lambda: api.call("PATCH", "/v1/admin/listing-sources/"
                + state["source_id"], {"ingestionConfiguration": []}))
    if state.get("partnership_id"):
        attempt("dissolve partnership", lambda: api.call("DELETE", "/v1/admin/partnerships/"
                                                        + state["partnership_id"]))
    attempt("restore account tier", lambda: api.call("PATCH", "/v1/admin/users/" + state["user_id"],
                                                     {"tier": state["original_tier"]}))
    identifiers = [*state["filters"].values(), *state.get("listing_ids", {}).values()]
    if state.get("application_id"):
        identifiers.append(state["application_id"])
    notifications = attempt("read notifications", lambda: api.call("GET", "/v1/me/notifications?size=100"))
    for notification in (notifications or {}).get("items", []):
        if any(i in json.dumps(notification) for i in identifiers):
            attempt("delete notification", lambda n=notification: api.call(
                "DELETE", "/v1/me/notifications/" + n["notificationId"]))
    for obj in state["objects"]:
        args = ["s3api", "delete-object", "--bucket", BUCKET, "--key", obj["key"]]
        if obj.get("version"):
            args.extend(["--version-id", obj["version"]])
        attempt("delete image " + obj["key"], lambda args=args: aws(*args))
    state["cleanup_errors"] = errors
    save_state(workspace, state)
    return {"completed": not errors, "errors": errors, "fixtures_withdrawn": len(state.get("listing_ids", {})),
            "filter_count": len(state["filters"]), "image_versions_deleted": len(state["objects"]),
            "restored_tier": state["original_tier"], "historical_rows_retained": True}


def live(manifest, caller_token, state_dir, output):
    budget = estimate(manifest)
    assert budget["estimated_neuron_bound"] <= manifest["max_estimated_neurons"]
    schedule, deployed = deployment()
    workspace = private_workspace(state_dir)
    api = Api(caller_token)
    account = api.call("GET", "/v1/me/account")
    assert account["role"] == "ADMIN"
    catalogue = api.call("GET", "/v1/product-listings?size=1")
    if catalogue["items"]:
        raise RuntimeError("This fixture driver requires an empty active catalogue to bound production exposure")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S")
    state = {"name": "EPHEMERAL real-world matcher " + stamp, "user_id": account["userId"],
             "original_tier": account["tier"], "prefix": "verification/matcher-corpus/" + stamp,
             "objects": [], "filters": {}}
    save_state(workspace, state)
    report = None
    try:
        api.call("PATCH", "/v1/admin/users/" + state["user_id"], {"tier": "ULTIMATE"})
        print("Preparing isolated source and local image uploads", flush=True)
        create_source(api, workspace, state)
        urls = host_assets(manifest, workspace, state)
        create_listings(api, manifest, urls, workspace, state)
        create_filters(api, manifest, workspace, state)
        print("Waiting for canonical/search freshness convergence", flush=True)
        await_projection(api, workspace, state)
        start_task(schedule, workspace, state)
        print("Running deployed matcher: " + state["task_arn"], flush=True)
        task = await_task(workspace, state)
        report = collect_report(api, manifest, state, deployed, task)
        write_json(output, report)
        if task["exit_code"] != 0:
            raise RuntimeError("Matcher exited incompletely; unresolved cases are retained in the report")
    finally:
        print("Cleaning up temporary production resources", flush=True)
        cleaned = cleanup(api, workspace, state)
        if report is not None:
            report["cleanup"] = cleaned
            write_json(output, report)
            render_review(manifest, report)
        if not cleaned["completed"]:
            raise RuntimeError("Cleanup incomplete; resume from external state: " + str(workspace))
    return report
