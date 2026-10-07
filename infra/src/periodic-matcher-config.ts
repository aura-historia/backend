import type { EventPattern } from "aws-cdk-lib/aws-events";
import type { StageName } from "./config";
import { containerImage } from "./container-image-catalog";
import { ecsTaskEventPattern, ecsTaskL2EventPattern, type RawEcsTaskEventPattern } from "./constructs/ecs-task-event-patterns";

export const PERIODIC_MATCHER_IMAGE = containerImage("periodic-matcher");
export const PERIODIC_MATCHER_REPOSITORY = PERIODIC_MATCHER_IMAGE.repository;
export const PERIODIC_MATCHER_CONTAINER = "periodic-matcher";
export const PERIODIC_MATCHER_DIGEST_PATTERN = "^sha256:[0-9a-f]{64}$";

export function periodicMatcherNames(stage: StageName) {
  return {
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

export function matcherTaskEventPattern(clusterArn: string, familyArnPrefix: string, failure: "exit" | "interruption"): RawEcsTaskEventPattern {
  return ecsTaskEventPattern(clusterArn, familyArnPrefix, PERIODIC_MATCHER_CONTAINER, failure);
}

export function matcherTaskL2EventPattern(clusterArn: string, familyArnPrefix: string, failure: "exit" | "interruption"): EventPattern {
  return ecsTaskL2EventPattern(clusterArn, familyArnPrefix, PERIODIC_MATCHER_CONTAINER, failure);
}
