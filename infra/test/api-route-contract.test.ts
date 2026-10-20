import { parseAxumRouteKeys, routeKey } from "./api-route-contract";

describe("Axum route contract extraction", () => {
  test("retains legacy and unversioned aliases so parity cannot silently exclude them", () => {
    const routes = parseAxumRouteKeys(`
      Router::new()
        .route("/v1/health", get(health))
        .route("/api/v1/health", get(health))
        .route("/health", get(health))
        .with_state(state)
    `);

    expect(routes).toEqual(["GET /api/v1/health", "GET /health", "GET /v1/health"]);
  });

  test("retains duplicate operations after normalizing parameter names", () => {
    const routes = parseAxumRouteKeys(`
      Router::new()
        .route("/v1/items/{item_id}", get(read_item).patch(update_item))
        .route("/v1/items/{itemId}", get(read_item))
        .with_state(state)
    `);

    expect(routes).toEqual(["GET /v1/items/{}", "GET /v1/items/{}", "PATCH /v1/items/{}"]);
  });

  test("retains unexpected explicit methods and catch-alls for closed-catalog checks", () => {
    const routes = parseAxumRouteKeys(`
      Router::new()
        .route("/v1/health", head(health).options(preflight))
        .route("/v1/{*path}", any(proxy))
        .with_state(state)
    `);

    expect(routes).toEqual(["ANY /v1/{*}", "HEAD /v1/health", "OPTIONS /v1/health"]);
  });

  test("does not normalize Axum or Gateway catch-alls into ordinary parameters", () => {
    expect(routeKey("get", "/v1/{*path}")).toBe("GET /v1/{*}");
    expect(routeKey("get", "/v1/{proxy+}")).toBe("GET /v1/{+}");
    expect(routeKey("get", "/v1/{item_id}")).toBe("GET /v1/{}");
  });

  test("bounds methods to each route call, excluding layers and later helper calls", () => {
    const routes = parseAxumRouteKeys(`
      Router::new()
        .route("/v1/items", get(read_items).post(create_item))
        .layer(make_layer("ignored )", cache.get(key)))
        .route ("/v1/health", get(health))
        .with_state(state);
      let unrelated = cache.get(key);
    `);

    expect(routes).toEqual(["GET /v1/health", "GET /v1/items", "POST /v1/items"]);
  });

  test("fails closed when a route uses an unsupported method-router expression", () => {
    expect(() => parseAxumRouteKeys(`
      Router::new().route("/api/v1/hidden", legacy_handler).with_state(state)
    `)).toThrow("Cannot extract explicit HTTP methods for /api/v1/hidden");
  });
});
