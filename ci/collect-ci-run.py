#!/usr/bin/env python3
"""Collect completed GitHub workflow/job timings without inferring missing stages."""

import argparse
import datetime
import json
import pathlib
import subprocess


def seconds(start, end):
    if not start or not end:
        return None
    elapsed = (datetime.datetime.fromisoformat(end.replace("Z", "+00:00")) -
               datetime.datetime.fromisoformat(start.replace("Z", "+00:00"))).total_seconds()
    return elapsed if elapsed >= 0 else None


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", path], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_id", type=int)
    parser.add_argument("--repository", default="aura-historia/backend")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    base = f"repos/{args.repository}/actions/runs/{args.run_id}"
    run = api(base)
    if run["status"] != "completed":
        raise SystemExit("Workflow must be completed before collecting whole-workflow duration")
    # The run endpoint keeps the first attempt's created_at after a rerun. Its
    # attempt endpoint supplies the creation time matching the jobs below.
    run = api(f"{base}/attempts/{run['run_attempt']}")
    jobs, page = [], 1
    while True:
        batch = api(f"{base}/attempts/{run['run_attempt']}/jobs?per_page=100&page={page}")["jobs"]
        jobs.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    output = {"schema_version": 1, "kind": "WORKFLOW_TIMING", "repository": args.repository,
        "run_id": args.run_id, "attempt": run["run_attempt"], "head_sha": run["head_sha"], "url": run["html_url"],
        "created_at": run["created_at"], "run_started_at": run["run_started_at"], "updated_at": run["updated_at"],
        "conclusion": run["conclusion"], "workflow_seconds": seconds(run["created_at"], run["updated_at"]),
        "duration_method": "GitHub completed run attempt updated_at minus created_at; includes scheduling, jobs and finalization",
        "timestamp_limitations": "Missing or reversed intervals are unavailable (null). Reruns can retain successful jobs from earlier attempts; these are diagnostic snapshots, not fresh full-suite performance repetitions. Use fresh run IDs for controlled repetitions.",
        "jobs": [{"id": job["id"], "name": job["name"], "conclusion": job["conclusion"],
                  "runner_name": job["runner_name"], "labels": job["labels"],
                  "created_at": job.get("created_at"), "started_at": job["started_at"], "completed_at": job["completed_at"],
                  "scheduling_delay_seconds": seconds(job.get("created_at"), job["started_at"]),
                  "seconds": seconds(job["started_at"], job["completed_at"]), "steps": job["steps"]} for job in jobs]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
