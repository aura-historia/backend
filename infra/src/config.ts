import * as cdk from "aws-cdk-lib";

export const STAGES = ["prod", "dev", "ephemeral"] as const;
export type StageName = (typeof STAGES)[number];

export interface StageConfig {
  readonly stage: StageName;
  readonly defaultStageName: string;
  readonly isProd: boolean;
  readonly isEphemeral: boolean;
  readonly removalPolicy: cdk.RemovalPolicy;
  readonly apiEndpointUrl: string | undefined;
  readonly opensearchDomainName: string;
  readonly enableProductionObservability: boolean;
  readonly stripeCheckoutCancelUrl: string;
  readonly stripeCheckoutSuccessUrl: string;
  readonly stripePortalReturnUrl: string;
}

export function isStageName(value: string): value is StageName {
  return (STAGES as readonly string[]).includes(value);
}

export function stageConfig(stage: StageName): StageConfig {
  const isProd = stage === "prod";
  const isEphemeral = stage === "ephemeral";

  return {
    stage,
    defaultStageName: stage,
    isProd,
    isEphemeral,
    removalPolicy: isProd ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY,
    apiEndpointUrl:
      stage === "prod"
        ? "https://api.aura-historia.com"
        : stage === "dev"
          ? "https://api.dev.aura-historia.com"
          : undefined,
    opensearchDomainName: isEphemeral ? "test-domain" : `aura-historia-${stage}`,
    enableProductionObservability: isProd,
    stripeCheckoutCancelUrl: isProd ? "https://aura-historia.com" : "https://stage.aura-historia.com",
    stripeCheckoutSuccessUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripePortalReturnUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
  };
}
