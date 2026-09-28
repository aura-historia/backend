import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const {
  awsDescribeImages,
  extractTaskDefinitionOutputs,
  loadCatalog,
  prepareNewTemplateUpdate,
  preparePreviousTemplateUpdate,
  resolveImage,
  resolveImages,
  validateCatalog,
  validateNewTemplateParameters,
} = require("../../ci/container-images.cjs") as {
  awsDescribeImages: (repository: string, tag: string, runCommand: (...args: any[]) => any) => any;
  extractTaskDefinitionOutputs: (stack: unknown, catalog: readonly any[], stackName?: string) => readonly any[];
  loadCatalog: () => readonly any[];
  prepareNewTemplateUpdate: (stack: unknown, sha: string, digests: Record<string, string>, catalog: readonly any[]) => any;
  preparePreviousTemplateUpdate: (stack: unknown, sha: string, digests: Record<string, string>, catalog: readonly any[]) => any;
  resolveImage: (image: any, sha: string, describe: (repository: string, tag: string) => unknown, options?: { allowMissing?: boolean }) => any;
  resolveImages: (catalog: readonly any[], sha: string, describe: (repository: string, tag: string) => unknown) => Record<string, string>;
  validateCatalog: (catalog: unknown, rootDir: string) => readonly any[];
  validateNewTemplateParameters: (parameters: readonly any[], catalog: readonly any[]) => Map<string, string>;
};

const SHA = "6a9c07fa66163b0917434cbf6bcc20e40b9c845c";
const DIGEST_A = `sha256:${"a".repeat(64)}`;
const DIGEST_B = `sha256:${"b".repeat(64)}`;
const TASK_ARN = "arn:aws:ecs:eu-central-1:123456789012:task-definition/aura-historia-periodic-matcher-prod:42";

function ecrResult(tag: string, digest: string) {
  return { imageDetails: [{ imageDigest: digest, imageTags: [tag] }] };
}

function createFixtureCatalog(): { root: string; catalog: any[]; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "container-catalog-"));
  const catalog = ["worker-one", "worker-two"].map((id, index) => {
    const crate = `src/${id}`;
    const dockerfile = `${crate}/Dockerfile`;
    mkdirSync(join(root, crate, "src"), { recursive: true });
    writeFileSync(join(root, crate, "Cargo.toml"), `[package]\nname = "${id}"\nversion = "0.1.0"\n`);
    writeFileSync(join(root, crate, "src/main.rs"), "fn main() {}\n");
    writeFileSync(join(root, dockerfile), "FROM scratch\n");
    mkdirSync(join(root, `ci/container-images/${id}`), { recursive: true });
    writeFileSync(join(root, `ci/container-images/${id}/smoke.sh`), "#!/usr/bin/env bash\n");
    return {
      id,
      crate,
      binary: id,
      dockerfile,
      repository: `aura-${id}`,
      platform: "linux/amd64",
      digestParameter: `${index === 0 ? "First" : "Second"}ImageDigest`,
      activationParameter: `${index === 0 ? "First" : "Second"}Enabled`,
      taskDefinitionOutput: `${index === 0 ? "First" : "Second"}TaskDefinitionArn`,
    };
  });
  return { root, catalog, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

function stack(parameters: any[], outputs: any[] = []) {
  return { Stacks: [{ Parameters: parameters, Outputs: outputs }] };
}

describe("container image catalog and release preflight", () => {
  test("validates the production catalog and makes a two-image digest map without matrix output collisions", () => {
    const production = loadCatalog();
    expect(production).toHaveLength(1);

    const fixture = createFixtureCatalog();
    try {
      const catalog = validateCatalog(fixture.catalog, fixture.root);
      let calls = 0;
      const digests = resolveImages(catalog, SHA, (repository, tag) => {
        calls += 1;
        expect(tag).toBe(`git-${SHA}`);
        return ecrResult(tag, repository === "aura-worker-one" ? DIGEST_A : DIGEST_B);
      });
      expect(calls).toBe(2);
      expect(digests).toEqual({ FirstImageDigest: DIGEST_A, SecondImageDigest: DIGEST_B });
      expect(new Set(Object.values(digests))).toEqual(new Set([DIGEST_A, DIGEST_B]));
    } finally {
      fixture.cleanup();
    }
  });

  test.each(["id", "repository", "digestParameter", "taskDefinitionOutput"])("rejects duplicate %s before any AWS lookup", (key) => {
    const fixture = createFixtureCatalog();
    try {
      const duplicate = fixture.catalog.map((entry) => ({ ...entry }));
      duplicate[1][key] = duplicate[0][key];
      expect(() => validateCatalog(duplicate, fixture.root)).toThrow(new RegExp(`duplicate ${key}`));
    } finally {
      fixture.cleanup();
    }
  });

  test("rejects unsafe paths, unsupported platforms, and missing binaries or Dockerfiles", () => {
    const fixture = createFixtureCatalog();
    try {
      expect(() => validateCatalog([{ ...fixture.catalog[0], crate: "../outside" }], fixture.root)).toThrow(/safe repository-relative path/);
      expect(() => validateCatalog([{ ...fixture.catalog[0], platform: "linux/s390x" }], fixture.root)).toThrow(/unsupported platform/);
      expect(() => validateCatalog([{ ...fixture.catalog[0], binary: "missing-binary" }], fixture.root)).toThrow(/binary .* is not declared/);
      expect(() => validateCatalog([{ ...fixture.catalog[0], dockerfile: "src/worker-one/Missing.Dockerfile" }], fixture.root)).toThrow(/Dockerfile does not exist/);
    } finally {
      fixture.cleanup();
    }
  });

  test("reuses an existing immutable SHA tag and allows building only on the typed image-not-found response", () => {
    const image = loadCatalog()[0];
    let lookups = 0;
    const existing = resolveImage(image, SHA, (repository, tag) => {
      lookups += 1;
      expect(repository).toBe(image.repository);
      return ecrResult(tag, DIGEST_A);
    }, { allowMissing: true });
    expect(existing).toEqual({ status: "existing", repository: image.repository, tag: `git-${SHA}`, digest: DIGEST_A });
    expect(lookups).toBe(1);

    const notFound = Object.assign(new Error("Image is absent"), { code: "ImageNotFoundException" });
    expect(resolveImage(image, SHA, () => { throw notFound; }, { allowMissing: true })).toMatchObject({ status: "missing" });
    expect(() => resolveImage(image, SHA, () => { throw notFound; })).toThrow(/Missing immutable image/);
  });

  test.each([
    Object.assign(new Error("access denied"), { code: "AccessDeniedException" }),
    Object.assign(new Error("throttled"), { code: "ThrottlingException" }),
    new Error("network connection failed"),
  ])("does not classify authorization, throttling, or network failures as image absence", (error) => {
    const image = loadCatalog()[0];
    expect(() => resolveImage(image, SHA, () => { throw error; }, { allowMissing: true })).toThrow(error.message);
  });

  test("classifies only ECR ImageNotFoundException as absence and rejects malformed CLI responses", () => {
    const args: any[][] = [];
    const result = awsDescribeImages("aura-test", `git-${SHA}`, (_command: string, commandArgs: string[]) => {
      args.push(commandArgs);
      return JSON.stringify(ecrResult(`git-${SHA}`, DIGEST_A));
    });
    expect(result).toEqual(ecrResult(`git-${SHA}`, DIGEST_A));
    expect(args[0]).toContain("--repository-name");
    expect(args[0]).toContain("aura-test");

    const failure = (message: string) => {
      const error = new Error("aws cli exited non-zero") as Error & { stderr: Buffer };
      error.stderr = Buffer.from(message);
      return error;
    };
    const image = loadCatalog()[0];
    const absent = resolveImage(image, SHA, (repository, tag) => awsDescribeImages(repository, tag, () => {
      throw failure("An error occurred (ImageNotFoundException) when calling the DescribeImages operation");
    }), { allowMissing: true });
    expect(absent.status).toBe("missing");
    for (const diagnostic of ["AccessDeniedException", "ThrottlingException", "network connection failed"]) {
      expect(() => awsDescribeImages("aura-test", `git-${SHA}`, () => { throw failure(diagnostic); })).toThrow(/ECR lookup failed/);
    }
    expect(() => awsDescribeImages("aura-test", `git-${SHA}`, () => "not JSON")).toThrow(/malformed JSON/);
  });

  test("rejects malformed, ambiguous, wrong-tag, and invalid-digest ECR results", () => {
    const image = loadCatalog()[0];
    const malformed = [undefined, {}, { imageDetails: [] }, { imageDetails: [{ imageDigest: DIGEST_A }] }, ecrResult(`git-${"0".repeat(40)}`, DIGEST_A), ecrResult(`git-${SHA}`, "latest")];
    for (const result of malformed) {
      expect(() => resolveImage(image, SHA, () => result, { allowMissing: true })).toThrow();
    }
  });

  test("new-template initialization allows absent new digests but rejects malformed values when present", () => {
    const catalog = loadCatalog();
    expect(validateNewTemplateParameters([{ ParameterKey: "CommitSHA", ParameterValue: SHA }], catalog).size).toBe(0);
    expect(validateNewTemplateParameters([
      { ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "false" },
      { ParameterKey: "PeriodicMatcherImageDigest", ParameterValue: DIGEST_A },
    ], catalog).get("PeriodicMatcherEnabled")).toBe("false");
    expect(validateNewTemplateParameters([
      { ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "true" },
      { ParameterKey: "PeriodicMatcherImageDigest", ParameterValue: DIGEST_A },
    ], catalog).get("PeriodicMatcherEnabled")).toBe("true");
    expect(() => validateNewTemplateParameters([{ ParameterKey: "PeriodicMatcherImageDigest", ParameterValue: "sha256:bad" }], catalog)).toThrow(/malformed/);
    expect(() => validateNewTemplateParameters([{ ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "sometimes" }], catalog)).toThrow(/true or false/);
  });

  test("new-template initialization supplies every release digest and preserves existing activation", () => {
    const catalog = loadCatalog();
    const digests = { PeriodicMatcherImageDigest: DIGEST_A };
    const expected = {
      parameters: { CommitSHA: SHA, PeriodicMatcherImageDigest: DIGEST_A },
      preserveExistingParameters: true,
    };
    expect(prepareNewTemplateUpdate(null, SHA, digests, catalog)).toEqual(expected);
    expect(prepareNewTemplateUpdate(stack([{ ParameterKey: "CommitSHA", ParameterValue: SHA }]), SHA, digests, catalog)).toEqual(expected);
    expect(prepareNewTemplateUpdate(stack([
      { ParameterKey: "CommitSHA", ParameterValue: SHA },
      { ParameterKey: "PeriodicMatcherImageDigest", ParameterValue: DIGEST_A },
      { ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "true" },
      { ParameterKey: "CdcRouterEnabled", ParameterValue: "false" },
    ]), SHA, digests, catalog)).toEqual(expected);
    expect(() => prepareNewTemplateUpdate(stack([
      { ParameterKey: "PeriodicMatcherImageDigest", ParameterValue: "sha256:bad" },
    ]), SHA, digests, catalog)).toThrow(/malformed/);
    expect(() => prepareNewTemplateUpdate(stack([
      { ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "sometimes" },
    ]), SHA, digests, catalog)).toThrow(/true or false/);
  });

  test("previous-template updates require every digest parameter, change the complete artifact set, and preserve activation", () => {
    const fixture = createFixtureCatalog();
    try {
      const catalog = validateCatalog(fixture.catalog, fixture.root);
      const parameters = [
        { ParameterKey: "CommitSHA", ParameterValue: SHA },
        { ParameterKey: "FirstImageDigest", ParameterValue: DIGEST_A },
        { ParameterKey: "SecondImageDigest", ParameterValue: DIGEST_B },
        { ParameterKey: "FirstEnabled", ParameterValue: "true" },
        { ParameterKey: "SecondEnabled", ParameterValue: "false" },
        { ParameterKey: "PeriodicMatcherEnabled", ParameterValue: "true" },
        { ParameterKey: "CdcRouterEnabled", ParameterValue: "false" },
        { ParameterKey: "OtherSetting", ParameterValue: "unchanged" },
      ];
      const outputs = catalog.map((image: any) => ({ OutputKey: image.taskDefinitionOutput, OutputValue: TASK_ARN }));
      const allUnchanged = preparePreviousTemplateUpdate(stack(parameters, outputs), SHA, { FirstImageDigest: DIGEST_A, SecondImageDigest: DIGEST_B }, catalog);
      expect(allUnchanged.unchanged).toBe(true);
      expect(allUnchanged.parameters.find((parameter: any) => parameter.ParameterKey === "PeriodicMatcherEnabled")).toEqual({ ParameterKey: "PeriodicMatcherEnabled", UsePreviousValue: true });
      const oneChanged = preparePreviousTemplateUpdate(stack(parameters, outputs), SHA, { FirstImageDigest: DIGEST_A, SecondImageDigest: `sha256:${"c".repeat(64)}` }, catalog);
      expect(oneChanged.unchanged).toBe(false);
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "FirstImageDigest").ParameterValue).toBe(DIGEST_A);
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "SecondImageDigest").ParameterValue).toBe(`sha256:${"c".repeat(64)}`);
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "FirstEnabled")).toEqual({ ParameterKey: "FirstEnabled", UsePreviousValue: true });
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "SecondEnabled")).toEqual({ ParameterKey: "SecondEnabled", UsePreviousValue: true });
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "CdcRouterEnabled")).toEqual({ ParameterKey: "CdcRouterEnabled", UsePreviousValue: true });
      expect(() => preparePreviousTemplateUpdate(stack(parameters.filter((parameter) => parameter.ParameterKey !== "SecondEnabled"), outputs), SHA, { FirstImageDigest: DIGEST_A, SecondImageDigest: DIGEST_B }, catalog)).toThrow(/missing SecondEnabled/);
      expect(oneChanged.parameters.find((parameter: any) => parameter.ParameterKey === "OtherSetting")).toEqual({ ParameterKey: "OtherSetting", UsePreviousValue: true });
      expect(() => preparePreviousTemplateUpdate(stack(parameters.filter((parameter) => parameter.ParameterKey !== "SecondImageDigest"), outputs), SHA, { FirstImageDigest: DIGEST_A, SecondImageDigest: DIGEST_B }, catalog)).toThrow(/infrastructure-bearing release first/);
      expect(() => preparePreviousTemplateUpdate(stack(parameters), SHA, { FirstImageDigest: DIGEST_A, SecondImageDigest: DIGEST_B }, catalog)).toThrow(/missing or invalid .*TaskDefinitionArn.*infrastructure-bearing release first/);
    } finally {
      fixture.cleanup();
    }
  });

  test("historical release missing a newly cataloged image cannot be rolled back by substitution", () => {
    const fixture = createFixtureCatalog();
    try {
      const catalog = validateCatalog(fixture.catalog, fixture.root);
      const missing = Object.assign(new Error("image not found"), { code: "ImageNotFoundException" });
      expect(() => resolveImages(catalog, SHA, (repository, tag) => {
        if (repository === "aura-worker-two") throw missing;
        return ecrResult(tag, DIGEST_A);
      })).toThrow(`Missing immutable image aura-worker-two:git-${SHA}`);
    } finally {
      fixture.cleanup();
    }
  });

  test("task output extraction accepts one valid ARN and rejects missing, empty, duplicate, or malformed outputs", () => {
    const catalog = loadCatalog();
    const goodOutput = { OutputKey: "PeriodicMatcherTaskDefinitionArn", OutputValue: TASK_ARN };
    expect(extractTaskDefinitionOutputs(stack([], [goodOutput]), catalog, "application-prod-compute")).toEqual([
      { id: "periodic-matcher", output: "PeriodicMatcherTaskDefinitionArn", value: TASK_ARN },
    ]);
    for (const outputs of [[], [{ ...goodOutput, OutputValue: "" }], [goodOutput, goodOutput], [{ ...goodOutput, OutputValue: "not-an-arn" }]]) {
      expect(() => extractTaskDefinitionOutputs(stack([], outputs), catalog, "application-prod-compute")).toThrow(/Post-deployment output lookup failed|not a valid ECS task-definition ARN/);
    }
  });
});
