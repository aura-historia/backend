"""Offline integrity, metrics and review rendering for the pinned corpus."""

import hashlib
import html
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read_json(path):
    return json.loads(Path(path).read_text())


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def local_path(relative):
    path = (ROOT / relative).resolve()
    if not path.is_relative_to(ROOT) or "://" in relative:
        raise ValueError(f"Non-local corpus resource: {relative}")
    return path


def validate():
    manifest = read_json(ROOT / "manifest.json")
    assert manifest["label_provenance"] == "agent-authored"
    groups = {g["id"]: g for g in manifest["groups"]}
    cases = manifest["cases"]
    assert len(groups) == len(manifest["groups"])
    assert len({c["id"] for c in cases}) == len(cases) == manifest["case_count"]
    assets = {a["path"]: a for a in manifest["assets"]}
    for name, asset in assets.items():
        blob = local_path(name).read_bytes()
        assert blob.startswith(b"\xff\xd8\xff"), name
        assert len(blob) == asset["bytes"] <= 4 * 1024 * 1024, name
        assert hashlib.sha256(blob).hexdigest() == asset["sha256"], name
        source = read_json(local_path(asset["source_record"]))
        assert source["share_license_status"] == "CC0", name
    for case in cases:
        group = groups[case["group"]]
        assert isinstance(case["expected_match"], bool)
        assert case["label_reason"] and case["object_ids"]
        expected_images = {"TEXT_ONLY": 0, "ONE_IMAGE": 1, "FOUR_IMAGES": 4}
        assert len(case["image_paths"]) == expected_images[group["evidence"]]
        assert all(path in assets for path in case["image_paths"])
        assert len(case["title"]) <= 255 and len(case["description"]) <= 32768
    assert len({i for c in cases for i in c["object_ids"]}) == manifest["unique_object_count"]
    return manifest


def estimate(manifest):
    # Conservative bound: UTF-8 bytes, rather than a language-specific tokenizer.
    tokens = sum(
        1800
        + len((c["title"] + c["description"]).encode())
        + 1027 * len(c["image_paths"])
        for c in manifest["cases"]
    )
    neurons = tokens * manifest["neurons_per_million_input_tokens"] / 1_000_000
    return {"max_provider_attempts_per_case": 1, "input_token_bound": tokens,
            "estimated_neuron_bound": round(neurons, 2)}


def metrics(rows):
    resolved = [r for r in rows if r["quality_resolved"]]
    counts = {"true_positive": 0, "false_positive": 0,
              "true_negative": 0, "false_negative": 0}
    for row in resolved:
        key = ("true" if row["expected_match"] == row["matched"] else "false")
        key += "_positive" if row["matched"] else "_negative"
        counts[key] += 1
    tp, fp, fn = counts["true_positive"], counts["false_positive"], counts["false_negative"]
    return {"cases": len(rows), "quality_resolved": len(resolved),
            "not_scored_or_degraded": len(rows) - len(resolved), **counts,
            "precision": tp / (tp + fp) if tp + fp else None,
            "recall": tp / (tp + fn) if tp + fn else None}


def measurement_summary(manifest, rows):
    observed = metrics([{**r, "quality_resolved": True} for r in rows])
    # Retrieval exclusions are observable outcomes, not additional model decisions.
    observed.pop("quality_resolved")
    observed.pop("not_scored_or_degraded")
    return {
        "manifest_sha256": hashlib.sha256((ROOT / "manifest.json").read_bytes()).hexdigest(),
        "classifier_quality": metrics(rows),
        "observed_persisted_matches": observed,
        "by_group": {g["id"]: metrics([r for r in rows if r["group"] == g["id"]])
                     for g in manifest["groups"]},
        "not_evaluated_case_ids": [r["id"] for r in rows if r["decision"] is None],
    }


def render_review(manifest, report=None):
    rows = {r["id"]: r for r in (report or {}).get("cases", [])}
    cards = []
    for case in manifest["cases"]:
        row = rows.get(case["id"])
        status = "unscored" if row is None else (
            "unresolved" if not row["quality_resolved"] else (
                "pass" if row["matched"] == case["expected_match"] else "miss"))
        images = case["image_paths"] or [f"assets/cma-{case['object_ids'][0]}.jpg"]
        photos = "".join(f'<a href="{html.escape(p)}"><img loading="lazy" src="{html.escape(p)}"></a>'
                         for p in images)
        observed = "Not scored" if row is None else (
            f"Matched: {row['matched']}; score: {row['score']}; "
            f"images sent: {row['sent_images']}/{row['requested_images']}")
        cards.append(f'<article data-group="{case["group"]}" data-status="{status}">'
                     f'<h2>{html.escape(case["id"])} · {status}</h2><div class="photos">{photos}</div>'
                     f'<h3>{html.escape(case["title"])}</h3><p>{html.escape(case["description"])}</p>'
                     f'<p><b>Expected match: {case["expected_match"]}</b> · {html.escape(observed)}</p>'
                     f'<p>{html.escape(case["label_reason"])}</p></article>')
    options = "".join(f'<option>{html.escape(g["id"])}</option>' for g in manifest["groups"])
    page = '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Matcher corpus review</title>
<style>body{font:16px system-ui;max-width:1200px;margin:2rem auto;padding:0 1rem;background:#f4f3ef;color:#222}
header{position:sticky;top:0;background:#f4f3ef;padding:1rem 0}article{background:white;padding:1rem;margin:1rem 0;border:1px solid #ccc}
article[data-status=miss]{border:3px solid #ae332b}.photos{display:flex;gap:.5rem;flex-wrap:wrap}.photos img{height:180px;max-width:280px;object-fit:contain}select{padding:.5rem}</style>
<header><h1>Real-world matcher review</h1><p>Agent-authored labels; human review pending. A purposive challenge set, not a population error-rate estimate.
All photographs and source snapshots are local. Photographs on text-only cases are for review and were not sent to the model.</p>
<select id="group"><option value="">All groups</option>''' + options + '''</select>
<select id="status"><option value="">All results</option><option>miss</option><option>pass</option><option>unresolved</option><option>unscored</option></select></header>
''' + "".join(cards) + '''<script>
function filter(){document.querySelectorAll('article').forEach(a=>a.hidden=(group.value&&a.dataset.group!==group.value)||(status.value&&a.dataset.status!==status.value))}
const group=document.getElementById('group'),status=document.getElementById('status');group.onchange=status.onchange=filter;
</script></html>'''
    (ROOT / "review.html").write_text(page)
