"""Conservative source evidence, never a substitute for compiled test discovery."""

import re


PATTERNS = {
    "aura_integration_test": r"#\[\s*(?:[\w]+::)*aura_integration_test\b",
    "serial_test": r"#\[\s*(?:(?:serial_test|test_api)::)?serial(?:\s*\(|\s*\])",
    "testcontainers": r"\btestcontainers\b",
    "postgres": r"\b(?:get_postgres_client|Postgres|POSTGRES(?:_\w+)?)\b",
    "floci": r"\b(?:get_aws_emulator|spin_up_aws_emulator|Sqs|WorkerSqs|S3|Ses|ApiGateway|EventBridge)\b",
    "opensearch": r"\b(?:OpenSearch|get_opensearch_client|refresh_index)\b",
    "persistent_global": r"\b(?:static\s+(?:mut\s+)?\w+|OnceLock|LazyLock|lazy_static!)\b",
    "external_effect_review": r"\b(?:std::env::(?:set_var|remove_var)|Command::new|reqwest::|TcpListener|TcpStream|std::fs::(?:write|remove_file|remove_dir_all))\b",
    "cluster_global_ddl_replication": r"(?i)\b(?:CREATE\s+(?:ROLE|DATABASE|PUBLICATION|SUBSCRIPTION)|ALTER\s+(?:SYSTEM|ROLE)|pg_(?:create|drop)_logical_replication_slot|pg_replication_slots|wal_level)\b",
    "ignored": r"#\[\s*ignore\b",
    "platform_conditional": r"#\[\s*cfg(?:_attr)?\([^\n]*(?:target_|windows|unix)",
    "feature_conditional": r"#\[\s*cfg(?:_attr)?\([^\n]*feature\s*=",
    "parameterized": r"#\[\s*(?:rstest(?:::rstest)?|case(?:[(:\]])|test_case|test_matrix)",
    "source_test": r"#\[\s*(?:(?:tokio|actix_rt)::)?test\s*(?:\]|\()",
}


def source_paths(target, package_root, member_roots):
    """Scan a target's source subtree; overlap is explicit and conservative.

    Integration files can import siblings through mod/path/include, so their whole
    tests subtree is reviewed. Library/bin source shares a src subtree. This can
    overclassify fixtures; compiled case counts remain authoritative.
    """
    source = target["src_path"]
    scope = source.parent
    nested_members = {other for other in member_roots if package_root in other.parents}
    return sorted(path for path in scope.rglob("*.rs") if not any(
        other in path.parents for other in nested_members
    ))


def scan(paths, root):
    evidence = []
    for path in paths:
        # These are lexical review signals, including comments/doc examples and
        # inactive cfg branches. Do not infer executable case counts from them.
        for number, line in enumerate(path.read_text().splitlines(), 1):
            for kind, pattern in PATTERNS.items():
                if re.search(pattern, line):
                    evidence.append({"kind": kind, "path": str(path.relative_to(root)), "line": number})
    kinds = {item["kind"] for item in evidence}
    fixtures = sorted(kinds & {"postgres", "floci", "opensearch", "testcontainers"})
    return {"scope": "SOURCE_SUBTREE_CONSERVATIVE", "evidence": evidence, "fixtures": fixtures,
            "review_required": sorted(kinds & {"persistent_global", "external_effect_review", "cluster_global_ddl_replication"}),
            "lexical_test_attributes": sum(item["kind"] in {"source_test", "aura_integration_test"} for item in evidence)}
