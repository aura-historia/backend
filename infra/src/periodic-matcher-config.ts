import type { StageName } from "./config";

export const PERIODIC_MATCHER_REPOSITORY = "aura-historia-periodic-matcher";
export const PERIODIC_MATCHER_CONTAINER = "periodic-matcher";
export const PERIODIC_MATCHER_DIGEST_PATTERN = "^sha256:[0-9a-f]{64}$";

export function periodicMatcherNames(stage: StageName) {
  if (stage === "ephemeral") throw new Error("Periodic matcher requires a real AWS stage.");
  return {
    cluster: `aura-historia-scheduled-${stage}`,
    family: `aura-historia-periodic-matcher-${stage}`,
    group: `aura-historia-periodic-matcher-${stage}`,
    schedule: `search-filter-periodic-match-${stage}`,
    dlq: `aura-historia-periodic-matcher-delivery-${stage}`,
    applicationLog: `/aura-historia/${stage}/periodic-matcher`,
    lifecycleLog: `/aura-historia/${stage}/periodic-matcher-lifecycle`,
    lifecycleRule: `aura-historia-periodic-matcher-stopped-${stage}`,
    exitFailureRule: `aura-historia-periodic-matcher-exit-failure-${stage}`,
    interruptionRule: `aura-historia-periodic-matcher-interruption-${stage}`,
  };
}

export function matcherTaskEventPattern(clusterArn: string, familyArnPrefix: string, failure: "exit" | "interruption") {
  return {
    source: ["aws.ecs"],
    "detail-type": ["ECS Task State Change"],
    detail: {
      clusterArn: [clusterArn],
      taskDefinitionArn: [{ prefix: familyArnPrefix }],
      lastStatus: ["STOPPED"],
      ...(failure === "exit"
        ? { containers: { name: [PERIODIC_MATCHER_CONTAINER], exitCode: [{ "anything-but": 0 }] } }
        : { stopCode: ["TaskFailedToStart", "UserInitiated", "ServiceSchedulerInitiated", "SpotInterruption", "TerminationNotice"] }),
    },
  };
}
