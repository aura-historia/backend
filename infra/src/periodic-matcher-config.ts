import type { EventPattern } from "aws-cdk-lib/aws-events";
import type { StageName } from "./config";
import { containerImage } from "./container-image-catalog";

export const PERIODIC_MATCHER_IMAGE = containerImage("periodic-matcher");
export const PERIODIC_MATCHER_REPOSITORY = PERIODIC_MATCHER_IMAGE.repository;
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

export interface RawEventBridgePattern {
  readonly source: string[];
  readonly "detail-type": string[];
  readonly detail: Record<string, unknown>;
}

type MatcherTaskDetail = {
  readonly clusterArn: string[];
  readonly taskDefinitionArn: { readonly prefix: string }[];
  readonly lastStatus: string[];
} & (
  | {
      readonly containers: {
        readonly name: string[];
        readonly exitCode: { readonly "anything-but": number }[];
      };
      readonly stopCode?: never;
    }
  | {
      readonly stopCode: string[];
      readonly containers?: never;
    }
);

export type MatcherTaskEventPattern = RawEventBridgePattern & { readonly detail: MatcherTaskDetail };

export function matcherTaskEventPattern(clusterArn: string, familyArnPrefix: string, failure: "exit" | "interruption"): MatcherTaskEventPattern {
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

export function matcherTaskL2EventPattern(clusterArn: string, familyArnPrefix: string, failure: "exit" | "interruption"): EventPattern {
  const rawPattern = matcherTaskEventPattern(clusterArn, familyArnPrefix, failure);
  return {
    source: [...rawPattern.source],
    detailType: [...rawPattern["detail-type"]],
    detail: rawPattern.detail,
  };
}
