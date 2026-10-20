import * as fs from "node:fs";
import * as path from "node:path";

const RUST_ROUTE_FILES = [
  "../../src/aura-historia-api/src/lib.rs",
  "../../src/aura-historia-api/src/search_filters/mod.rs",
  "../../src/aura-historia-api/src/notifications/mod.rs",
  "../../src/aura-historia-api/src/partnership_applications/mod.rs",
  "../../src/aura-historia-api/src/partnerships/mod.rs",
] as const;

function normalizePath(value: string): string {
  return value.replace(/\{[^}]+}/g, "{}");
}

export function routeKey(method: string, routePath: string): string {
  return `${method.toUpperCase()} ${normalizePath(routePath)}`;
}

export function swaggerRouteKeys(): string[] {
  const lines = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8").split("\n");
  const routes: string[] = [];
  let currentPath: string | undefined;

  for (const line of lines) {
    const routeMatch = line.match(/^  (\/[^:]+):\s*$/);
    if (routeMatch) {
      currentPath = routeMatch[1];
      continue;
    }
    const methodMatch = line.match(/^    (get|post|put|patch|delete):\s*$/);
    if (methodMatch && currentPath) {
      routes.push(routeKey(methodMatch[1], currentPath));
    }
  }

  return routes.sort();
}

export function axumRouteKeys(): string[] {
  const routes = new Set<string>();

  for (const fileName of RUST_ROUTE_FILES) {
    const source = fs.readFileSync(path.join(__dirname, fileName), "utf8");
    const routeStart = /\.route\(\s*"([^"]+)"\s*,/g;
    let match: RegExpExecArray | null;
    while ((match = routeStart.exec(source))) {
      const routePath = match[1];
      if (!routePath.startsWith("/api/v1/")) {
        continue;
      }
      const nextRoute = source.indexOf(".route(", routeStart.lastIndex);
      const stateBoundary = source.indexOf(".with_state", routeStart.lastIndex);
      const handlerEnd = [nextRoute, stateBoundary].filter((index) => index !== -1).reduce(
        (end, index) => Math.min(end, index),
        source.length,
      );
      const handlers = source.slice(routeStart.lastIndex, handlerEnd);
      for (const method of handlers.matchAll(/\b(get|post|put|patch|delete)\s*\(/g)) {
        routes.add(routeKey(method[1], routePath));
      }
    }
  }

  return [...routes].sort();
}
