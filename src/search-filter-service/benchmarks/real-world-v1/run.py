#!/usr/bin/env python3
"""Validate/review the offline corpus or explicitly run its deployed production matcher."""

import argparse
import json
from pathlib import Path

from corpus import ROOT, estimate, read_json, render_review, validate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("validate")
    review = commands.add_parser("review")
    review.add_argument("--report", type=Path, default=ROOT / "results-prod-2026-10-31.json")
    live = commands.add_parser("live")
    live.add_argument("--token-file", required=True, type=Path)
    live.add_argument("--state-dir", required=True, type=Path)
    live.add_argument("--output", required=True, type=Path)
    live.add_argument("--allow-production", action="store_true", required=True)
    cleanup = commands.add_parser("cleanup")
    cleanup.add_argument("--state-dir", required=True, type=Path)
    cleanup.add_argument("--token-file", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "cleanup":
        # Recovery must remain usable even when the corpus itself is damaged.
        from prod import Api, cleanup
        state = read_json(args.state_dir / "state.json")
        print(json.dumps(cleanup(Api(args.token_file), args.state_dir, state), indent=2))
        return
    manifest = validate()
    if args.command == "validate":
        print(json.dumps({"cases": len(manifest["cases"]), "objects": manifest["unique_object_count"],
                          "assets": len(manifest["assets"]), **estimate(manifest)}, indent=2))
    elif args.command == "review":
        render_review(manifest, read_json(args.report))
        print(ROOT / "review.html")
    elif args.command == "live":
        from prod import live
        report = live(manifest, args.token_file, args.state_dir, args.output)
        print(json.dumps({"quality": report["classifier_quality"], "usage": report["usage"],
                          "cleanup": report["cleanup"]}, indent=2))


if __name__ == "__main__":
    main()
