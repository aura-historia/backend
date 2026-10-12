import { readFileSync } from "node:fs";
import { join } from "node:path";

describe.each([
  ["product-projector", "product-listings"],
  ["filter-projector", "user_search_filters"],
])("stage %s permissions", (role, index) => {
  test("allows indexing, bulk and required visibility confirmation on its own index", () => {
    const manifest = JSON.parse(readFileSync(
      join(__dirname, "../../opensearch/stage/roles", `${role}.json`),
      "utf8",
    ));

    expect(manifest).toEqual({
      cluster_permissions: [],
      index_permissions: [{
        index_patterns: [index],
        allowed_actions: ["indices:data/write/index", "indices:data/write/bulk*", ...(role === "product-projector" ? ["indices:admin/refresh"] : [])],
      }],
      tenant_permissions: [],
    });
  });
});
