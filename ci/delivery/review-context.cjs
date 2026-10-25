"use strict";

function serializeContext(evidence, config) {
  const serialized = JSON.stringify(evidence);
  if (Buffer.byteLength(serialized) > (config.maxInputBytes || 1500000))
    throw new Error(
      "Complete context exceeds reviewer budget; explicit human review required",
    );
  return serialized;
}

module.exports = { serializeContext };
