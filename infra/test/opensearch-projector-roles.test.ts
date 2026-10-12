import { readFileSync } from "node:fs";
import { join } from "node:path";

describe("stage product-projector permissions", () => {
  test("allows indexing, bulk and required visibility confirmation on its own index", () => {
    const manifest = JSON.parse(readFileSync(
      join(__dirname, "../../opensearch/stage/roles/product-projector.json"),
      "utf8",
    ));

    expect(manifest).toEqual({
      cluster_permissions: [],
      index_permissions: [{
        index_patterns: ["product-listings"],
        allowed_actions: ["indices:data/write/index", "indices:data/write/bulk*", "indices:admin/refresh", "indices:admin/refresh[s]"],
      }],
      tenant_permissions: [],
    });
  });
});
