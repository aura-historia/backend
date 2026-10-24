import { readFileSync } from "node:fs";
import { join } from "node:path";

describe("product-projector permissions", () => {
  test("allows only indexing and bulk writes on its own index", () => {
    const manifest = JSON.parse(readFileSync(
      join(__dirname, "../../opensearch/roles/product-projector.json"),
      "utf8",
    ));

    expect(manifest).toEqual({
      cluster_permissions: ["indices:data/write/bulk"],
      index_permissions: [{
        index_patterns: ["product-listings"],
        allowed_actions: ["indices:data/write/index", "indices:data/write/bulk*"],
      }],
      tenant_permissions: [],
    });
  });

  test.each(["stage", "prod"])("exposes only search and document routes through the %s gateway", (stage) => {
    const gateway = readFileSync(join(__dirname, `../../opensearch/${stage}/Caddyfile`), "utf8");
    const runtime = gateway.split("\n").find(line => line.trim().startsWith("@runtime path "));
    expect(runtime?.trim().split(/\s+/).slice(2)).toEqual([
      "/product-listings/_search", "/product-listings/_doc/*",
    ]);
    expect(gateway).not.toContain("/_refresh");
  });
});
