import type { StageName } from "./config";

export function scheduledEcsClusterName(stage: StageName): string {
  if (stage === "ephemeral") throw new Error("Scheduled ECS jobs require a real AWS stage.");
  return `aura-historia-scheduled-${stage}`;
}
