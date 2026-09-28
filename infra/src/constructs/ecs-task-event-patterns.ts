import type { EventPattern } from "aws-cdk-lib/aws-events";

export type EcsTaskEventKind = "stopped" | "exit" | "interruption";

export interface RawEcsTaskEventPattern {
  readonly source: string[];
  readonly "detail-type": string[];
  readonly detail: {
    readonly clusterArn: string[];
    readonly taskDefinitionArn: { readonly prefix: string }[];
    readonly lastStatus: string[];
    readonly containers?: { readonly name: string[]; readonly exitCode: { readonly "anything-but": number }[] };
    readonly stopCode?: string[];
  };
}

export function ecsTaskEventPattern(clusterArn: string, familyArnPrefix: string, containerName: string, kind: EcsTaskEventKind): RawEcsTaskEventPattern {
  return {
    source: ["aws.ecs"],
    "detail-type": ["ECS Task State Change"],
    detail: {
      clusterArn: [clusterArn],
      taskDefinitionArn: [{ prefix: familyArnPrefix }],
      lastStatus: ["STOPPED"],
      ...(kind === "exit" ? { containers: { name: [containerName], exitCode: [{ "anything-but": 0 }] } } : {}),
      ...(kind === "interruption" ? { stopCode: ["TaskFailedToStart", "UserInitiated", "ServiceSchedulerInitiated", "SpotInterruption", "TerminationNotice"] } : {}),
    },
  };
}

export function ecsTaskL2EventPattern(clusterArn: string, familyArnPrefix: string, containerName: string, kind: EcsTaskEventKind): EventPattern {
  const rawPattern = ecsTaskEventPattern(clusterArn, familyArnPrefix, containerName, kind);
  return {
    source: [...rawPattern.source],
    detailType: [...rawPattern["detail-type"]],
    detail: rawPattern.detail,
  };
}
