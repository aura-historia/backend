import * as fs from "node:fs";
import * as path from "node:path";

const RUST_ROUTE_FILES = [
  "../../src/aura-historia-api/src/lib.rs",
  "../../src/aura-historia-api/src/search_filters/mod.rs",
  "../../src/aura-historia-api/src/notifications/mod.rs",
  "../../src/aura-historia-api/src/partnership_applications/mod.rs",
  "../../src/aura-historia-api/src/partnerships/mod.rs",
] as const;

export function routeKey(method: string, routePath: string): string {
  const normalizedPath = routePath.replace(/\{([^}]+)}/g, (_match, parameter: string) => {
    if (parameter.startsWith("*")) return "{*}";
    if (parameter.endsWith("+")) return "{+}";
    return "{}";
  });
  return `${method.toUpperCase()} ${normalizedPath}`;
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
    const methodMatch = line.match(/^    (get|post|put|patch|delete|head|options|trace):\s*$/);
    if (methodMatch && currentPath) {
      routes.push(routeKey(methodMatch[1], currentPath));
    }
  }

  return routes.sort();
}

export function axumRouteKeys(): string[] {
  return RUST_ROUTE_FILES.flatMap((fileName) =>
    parseAxumRouteKeys(fs.readFileSync(path.join(__dirname, fileName), "utf8")),
  ).sort();
}

export function parseAxumRouteKeys(source: string): string[] {
  const routes: string[] = [];
  const routeStart = /\.route\s*\(\s*"([^"]+)"\s*,/g;
  let match: RegExpExecArray | null;
  while ((match = routeStart.exec(source))) {
    const routePath = match[1];
    const handlers = routeHandlers(source, routeStart.lastIndex);
    const methods = [...handlers.matchAll(/\b(get|post|put|patch|delete|head|options|trace|connect|any)\s*\(/g)];
    if (methods.length === 0) {
      throw new Error(`Cannot extract explicit HTTP methods for ${routePath}`);
    }
    for (const method of methods) {
      routes.push(routeKey(method[1], routePath));
    }
  }

  return routes.sort();
}

function routeHandlers(source: string, start: number): string {
  // Balance the route call itself, not the surrounding router chain. Parentheses in
  // ordinary Rust strings and comments do not delimit the method-router expression.
  const tokens = /"(?:\\.|[^"\\])*"|\/\/[^\n]*|\/\*[\s\S]*?\*\/|[()]/g;
  tokens.lastIndex = start;
  let depth = 1;
  let token: RegExpExecArray | null;
  while ((token = tokens.exec(source))) {
    if (token[0] === "(") depth += 1;
    if (token[0] === ")") depth -= 1;
    if (depth === 0) return source.slice(start, token.index);
  }
  throw new Error("Unterminated Axum route call");
}
