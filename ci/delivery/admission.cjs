"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { digest, validateEnvironmentState } = require("./contracts.cjs");

function aws(ctx, service, operation, args = [], options = {}) {
  try {
    return JSON.parse(
      execFileSync(
        "aws",
        [
          service,
          operation,
          ...args,
          "--region",
          ctx.region,
          "--output",
          "json",
          "--no-cli-pager",
          "--cli-connect-timeout",
          "10",
          "--cli-read-timeout",
          String(options.readTimeoutSeconds || 60),
        ],
        {
          encoding: "utf8",
          timeout: options.timeout || 90000,
          maxBuffer: 32 * 1024 * 1024,
          stdio: ["ignore", "pipe", "pipe"],
          env: { ...process.env, AWS_MAX_ATTEMPTS: "2" },
        },
      ) || "{}",
    );
  } catch (cause) {
    const error = new Error(
      `${service}/${operation} failed; outcome requires inspection`,
      { cause },
    );
    error.code =
      /\b(NoSuchKey|PreconditionFailed|ConditionalRequestConflict)\b/.exec(
        String(cause.stderr),
      )?.[1];
    throw error;
  }
}
function stateStore(ctx, api = aws) {
  if (
    !["dev", "prod"].includes(ctx.target) ||
    !/^\d{12}$/.test(ctx.account || "") ||
    !ctx.region ||
    !ctx.bucket
  )
    throw new Error("Explicit target/account/region/bucket required");
  const key = `environments/${ctx.account}/${ctx.region}/${ctx.target}.json`;
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "aura-admission-"));
  const filename = path.join(directory, "state.json");
  return {
    read() {
      try {
        const metadata = api(ctx, "s3api", "get-object", [
          "--bucket",
          ctx.bucket,
          "--key",
          key,
          filename,
        ]);
        const state = JSON.parse(fs.readFileSync(filename, "utf8"));
        validateEnvironmentState(state);
        if (state.target !== ctx.target)
          throw new Error("Wrong target environment record");
        if (!metadata.ETag)
          throw new Error("Missing state concurrency identity");
        return { state, etag: metadata.ETag };
      } catch (error) {
        if (error.code === "NoSuchKey") return null;
        throw error;
      }
    },
    write(state, etag) {
      validateEnvironmentState(state);
      if (state.target !== ctx.target)
        throw new Error("Wrong target environment record");
      fs.writeFileSync(filename, JSON.stringify(state), { mode: 0o600 });
      return api(ctx, "s3api", "put-object", [
        "--bucket",
        ctx.bucket,
        "--key",
        key,
        "--body",
        filename,
        "--content-type",
        "application/json",
        ...(etag ? ["--if-match", etag] : ["--if-none-match", "*"]),
      ]);
    },
    close() {
      fs.rmSync(directory, { recursive: true, force: true });
    },
  };
}
function update(store, expected, change) {
  const current = store.read();
  if (!current || digest(current.state) !== digest(expected))
    throw new Error(
      "Environment baseline changed; obtain fresh review and approval",
    );
  const next = {
    ...change(current.state),
    revision: current.state.revision + 1,
  };
  validateEnvironmentState(next);
  store.write(next, current.etag);
  return next;
}
function sameExecution(state, execution) {
  return state.active && digest(state.active) === digest(execution);
}
function acquire(store, expected, execution) {
  if (
    !execution?.executionId ||
    !/^[a-f0-9]{64}$/.test(execution.requestDigest || "") ||
    !/^[a-f0-9]{64}$/.test(execution.bundleDigest || "")
  )
    throw new Error(
      "Native execution and immutable request/bundle identity required",
    );
  const current = store.read();
  if (
    current?.state.state === "IN_PROGRESS" &&
    sameExecution(current.state, execution)
  )
    return current.state;
  return update(store, expected, (state) => {
    if (state.state !== "READY" || state.active || state.holdReason)
      throw new Error(
        "Environment is held; reconcile native AWS work before admission",
      );
    return {
      ...state,
      state: "IN_PROGRESS",
      active: execution,
      holdReason: "Native deployment has not completed verification",
    };
  });
}
function fail(store, execution, reason) {
  const current = store.read();
  if (!current || !sameExecution(current.state, execution))
    throw new Error("Cannot hold an execution owned by another request");
  return update(store, current.state, (state) => ({
    ...state,
    state: "NEEDS_ATTENTION",
    holdReason: reason || "Native execution outcome requires inspection",
  }));
}
function finalize(store, execution, evidence) {
  const current = store.read();
  if (
    !current ||
    current.state.state !== "IN_PROGRESS" ||
    !sameExecution(current.state, execution)
  )
    throw new Error("Only the active native execution can finalize");
  if (
    evidence.bundleDigest !== execution.bundleDigest ||
    evidence.verified !== true ||
    !evidence.sourceSha ||
    !evidence.observedSchema ||
    !evidence.nativeExecutionId ||
    evidence.nativeExecutionId !== execution.executionId
  )
    throw new Error("Complete exact native execution verification required");
  return update(store, current.state, (state) => ({
    ...state,
    state: "READY",
    active: null,
    holdReason: null,
    lastVerified: {
      bundleDigest: execution.bundleDigest,
      sourceSha: evidence.sourceSha,
      verifiedAt: evidence.verifiedAt,
    },
    observedSchema: evidence.observedSchema,
  }));
}
function enterMaintenance(store, expected, execution, reason) {
  if (
    !reason ||
    !execution?.executionId ||
    !execution?.requestDigest ||
    !execution?.bundleDigest
  )
    throw new Error("Maintenance owner, request, bundle and reason required");
  return update(store, expected, (state) => {
    if (state.state !== "READY" || state.active)
      throw new Error(
        "Settle and reconcile the active operation before entering maintenance",
      );
    return {
      ...state,
      state: "MAINTENANCE",
      active: execution,
      holdReason: reason,
    };
  });
}
function reconcile(store, expected, evidence) {
  if (
    !evidence?.operator ||
    !evidence.reason ||
    evidence.nativeWorkSettled !== true ||
    !evidence.observedSchema ||
    !evidence.evidenceDigest
  )
    throw new Error(
      "Operator identity, native settled evidence and fresh schema required; elapsed time cannot release a hold",
    );
  return update(store, expected, (state) => {
    if (
      !["MAINTENANCE", "NEEDS_ATTENTION", "IN_PROGRESS"].includes(state.state)
    )
      throw new Error("Environment has no held operation to reconcile");
    // Reconciliation records actual schema without inventing a successful application release.
    return {
      ...state,
      state: "READY",
      active: null,
      holdReason: null,
      observedSchema: evidence.observedSchema,
    };
  });
}
module.exports = {
  aws,
  stateStore,
  update,
  acquire,
  fail,
  finalize,
  enterMaintenance,
  reconcile,
};
