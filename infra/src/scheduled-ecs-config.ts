import type { StageName } from "./config";

export function scheduledEcsClusterName(stage: StageName): string {
  return `aura-historia-scheduled-${stage}`;
}
