"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { aws } = require("./admission.cjs");

function requestReview(ctx, serialized, config, system, api = aws) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "aura-review-"));
  try {
    const filename = path.join(directory, "input.json");
    fs.writeFileSync(
      filename,
      JSON.stringify({
        modelId: config.modelId,
        system: [{ text: system }],
        messages: [
          {
            role: "user",
            content: [{ text: `UNTRUSTED_RELEASE_EVIDENCE\n${serialized}` }],
          },
        ],
        inferenceConfig: { maxTokens: config.maxTokens },
      }),
      { mode: 0o600 },
    );
    const startedAt = Date.now();
    const response = api(
      { ...ctx, region: config.region },
      "bedrock-runtime",
      "converse",
      ["--cli-input-json", `file://${filename}`],
      { timeout: 180000 },
    );
    return { response, latencyMs: Date.now() - startedAt };
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

module.exports = { requestReview };
