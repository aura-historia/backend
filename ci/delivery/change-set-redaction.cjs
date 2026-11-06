"use strict";
// CloudFormation reports dynamic-reference values resolved in change-set property values. Plain
// SSM references then appear in plaintext, so stored evidence and review input keep only values
// that the reviewed templates already show. See docs/infra.md#runtime-credentials.
const REDACTED = "<redacted:dynamic-reference>";

function unescapePointer(segment) {
  return segment.replaceAll("~1", "/").replaceAll("~0", "~");
}

// The deepest template node on the path decides: a value derived from a dynamic reference can
// only originate below a node that contains one. Change-set list order need not match the
// template, so a list decides as a whole. A resource absent from the template fails closed.
function mayHoldResolvedReference(template, logicalId, pointer) {
  let node = template?.Resources?.[logicalId];
  if (node === undefined) return true;
  for (const segment of pointer.split("/").slice(1).map(unescapePointer)) {
    if (
      node === null ||
      typeof node !== "object" ||
      Array.isArray(node) ||
      !Object.hasOwn(node, segment)
    )
      break;
    node = node[segment];
  }
  return JSON.stringify(node ?? null).includes("{{resolve:");
}

function redactLeaves(value, pointer, redact) {
  if (Array.isArray(value))
    return value.map((item, index) =>
      redactLeaves(item, `${pointer}/${index}`, redact),
    );
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        redactLeaves(
          item,
          `${pointer}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`,
          redact,
        ),
      ]),
    );
  return redact(pointer) ? REDACTED : value;
}

// Contexts are JSON documents of the whole resource; an unparseable one is replaced entirely.
function redactContext(context, template, logicalId) {
  if (typeof context !== "string") return context;
  let parsed;
  try {
    parsed = JSON.parse(context);
  } catch {
    return REDACTED;
  }
  return JSON.stringify(
    redactLeaves(parsed, "", (pointer) =>
      mayHoldResolvedReference(template, logicalId, pointer),
    ),
  );
}

// A value without a property path cannot be attributed to the template, so it is removed.
function redactTarget(target, logicalId, before, after) {
  if (!target || typeof target !== "object") return target;
  const unattributed = typeof target.Path !== "string";
  const redacted = { ...target };
  if (
    Object.hasOwn(target, "BeforeValue") &&
    (unattributed || mayHoldResolvedReference(before, logicalId, target.Path))
  )
    redacted.BeforeValue = REDACTED;
  if (
    Object.hasOwn(target, "AfterValue") &&
    (unattributed || mayHoldResolvedReference(after, logicalId, target.Path))
  )
    redacted.AfterValue = REDACTED;
  return redacted;
}

/** Replaces change-set property values that may come from a dynamic reference. */
function redactResolvedValues(changes, beforeTemplate, afterTemplate) {
  return changes.map((change) => {
    const resource = change.ResourceChange;
    if (!resource) return change;
    const id = resource.LogicalResourceId;
    return {
      ...change,
      ResourceChange: {
        ...resource,
        ...(Object.hasOwn(resource, "BeforeContext")
          ? { BeforeContext: redactContext(resource.BeforeContext, beforeTemplate, id) }
          : {}),
        ...(Object.hasOwn(resource, "AfterContext")
          ? { AfterContext: redactContext(resource.AfterContext, afterTemplate, id) }
          : {}),
        ...(Array.isArray(resource.Details)
          ? {
              Details: resource.Details.map((detail) => ({
                ...detail,
                Target: redactTarget(detail.Target, id, beforeTemplate, afterTemplate),
              })),
            }
          : {}),
      },
    };
  });
}

module.exports = { REDACTED, redactResolvedValues };
