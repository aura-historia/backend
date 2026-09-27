import { resolve } from "node:path";
import catalogData from "../../ci/container-images.json";

export interface ContainerImage {
  readonly id: string;
  readonly crate: string;
  readonly binary: string;
  readonly dockerfile: string;
  readonly repository: string;
  readonly platform: "linux/amd64" | "linux/arm64";
  readonly digestParameter: string;
  readonly taskDefinitionOutput: string;
}

const { validateCatalog } = require("../../ci/container-images.cjs") as {
  validateCatalog(catalog: unknown, rootDir: string): readonly ContainerImage[];
};

export const CONTAINER_IMAGE_CATALOG = validateCatalog(catalogData, resolve(__dirname, "../.."));

export function containerImage(id: string): ContainerImage {
  const matches = CONTAINER_IMAGE_CATALOG.filter((image) => image.id === id);
  if (matches.length !== 1) throw new Error(`Expected exactly one container image catalog entry for '${id}'.`);
  return matches[0];
}
