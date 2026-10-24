"use strict";
const { aws } = require("./admission.cjs");

async function requestReview(ctx, serialized, config, prompt, options = {}) {
  const parameter = (options.api || aws)(ctx, "ssm", "get-parameter", [
    "--name", config.apiKeyParameterName, "--with-decryption",
  ]).Parameter;
  if (parameter?.Name !== config.apiKeyParameterName ||
      typeof parameter.Value !== "string" || !parameter.Value.trim())
    throw new Error("Reviewer API key is missing from its configured SSM parameter");
  const startedAt = Date.now();
  try {
    const response = await (options.fetch || fetch)("https://api.anthropic.com/v1/messages", {
      method: "POST",
      redirect: "error",
      signal: AbortSignal.timeout(300000),
      headers: {
        Authorization: `Bearer ${parameter.Value}`,
        "anthropic-version": "2026-10-21",
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: config.modelId,
        system: prompt.system,
        messages: [{ role: "user", content: `UNTRUSTED_RELEASE_EVIDENCE\n${serialized}` }],
        max_tokens: config.maxTokens,
        thinking: { type: "adaptive", display: "omitted" },
        output_config: { effort: config.effort, format: { type: "json_schema", schema: prompt.schema } },
      }),
    });
    if (!response.ok) {
      await response.body?.cancel();
      const error = new Error(`Anthropic review returned HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    const chunks = [];
    let bytes = 0;
    for await (const chunk of response.body) {
      bytes += chunk.byteLength;
      if (bytes > 1000000) throw new Error("Anthropic review response exceeds its bounded budget");
      chunks.push(chunk);
    }
    return {
      response: JSON.parse(Buffer.concat(chunks).toString("utf8")),
      latencyMs: Date.now() - startedAt,
    };
  } catch (cause) {
    // Provider bodies and transport errors can contain sensitive request content.
    const status = Number.isInteger(cause?.status) ? ` (HTTP ${cause.status})` : "";
    throw new Error(`Anthropic review request failed${status}; inspect provider status and retry the reviewed request`, { cause });
  }
}

module.exports = { requestReview };
