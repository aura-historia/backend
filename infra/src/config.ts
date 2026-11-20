import * as cdk from "aws-cdk-lib";

/** Production is the only stage: the host runs prod alone, and there is no AWS staging copy. */
export const STAGES = ["prod"] as const;
export type StageName = (typeof STAGES)[number];

/**
 * The Cognito trigger zips the Deploy workflow publishes, one immutable object per binary and
 * commit. The bucket is versioned and owned outside the app.
 */
export const LAMBDA_ARTIFACT_BUCKET_NAME = "aura-historia-binary-artifacts-eu-central-1";
export const CLOUDFORMATION_STAGING_BUCKET_NAME = "aura-historia-cfn-artifcats-eu-central-1";
export const WORKLOAD_REGION = "eu-central-1";
/** CloudFront publishes its metrics only here, so alarms on them live here too. */
export const EDGE_METRICS_REGION = "us-east-1";

const PROD_FRONTEND_URL = "https://aura-historia.com/";

export interface CognitoEmailConfig {
  readonly configurationSet: string;
  readonly from: string;
  readonly identityDomain: string;
  readonly replyTo: string;
}

export interface CognitoCustomDomainConfig {
  readonly domainName: string;
  readonly certificateArn: string;
}

export interface CognitoGoogleIdentityProviderConfig {
  readonly kind: "google";
  readonly providerName: "Google";
  readonly clientIdParameterName: string;
  /** Field of the stage's Cognito identity-provider credential secret. */
  readonly clientSecretField: "googleClientSecret";
  readonly scopes: readonly string[];
  readonly existingEmailAction: "LINK_VERIFIED";
  readonly linkSourceAttributeName: "Cognito_Subject";
}

export interface CognitoFacebookIdentityProviderConfig {
  readonly kind: "facebook";
  readonly providerName: "Facebook";
  readonly apiVersion: "v21.0";
  readonly clientIdParameterName: string;
  /** Field of the stage's Cognito identity-provider credential secret. */
  readonly clientSecretField: "facebookClientSecret";
  readonly scopes: readonly string[];
  readonly existingEmailAction: "REJECT";
}

// Each provider explicitly defines credentials, scopes, attribute mappings, client support, and collision policy.
export type CognitoIdentityProviderConfig =
  | CognitoGoogleIdentityProviderConfig
  | CognitoFacebookIdentityProviderConfig;

/** The SES identity and managed configuration set that every sender selects explicitly. */
export interface EmailSendingConfig {
  readonly configurationSet: string;
  readonly identityDomain: string;
}

export interface McpConfig {
  readonly domainName: string;
  readonly cloudFrontCertificateArn: string;
}

/**
 * The self-hosted origin behind both CloudFront distributions. The DNS name is unproxied and
 * deliberately unpublished, so it is an operator-owned SSM parameter rather than source.
 */
export interface HostOriginConfig {
  readonly domainName: string;
  /**
   * Header CloudFront adds to every origin request, overwriting any viewer value; the host's
   * reverse proxy rejects requests without it. Its value is the `originVerify` credential.
   */
  readonly verifyHeaderName: string;
}

/** What the host's IAM user may reach: backups, email, places, Cognito and its logs and metrics. */
export interface HostAccessConfig {
  readonly userName: string;
  readonly backupBucketName: string;
  /** Days a replaced or deleted backup object stays recoverable after the host overwrites it. */
  readonly backupNoncurrentVersionRetentionDays: number;
  /** Prefix of the container log groups the host ships to. */
  readonly containerLogGroupPrefix: string;
  /** The only CloudWatch namespace the host may publish metrics to. */
  readonly metricNamespace: string;
}

/** The Compose services whose container logs the host ships, one log group each. */
export const CONTAINER_LOG_SERVICES = ["postgres", "api", "mcp", "worker", "caddy", "ops"] as const;

/**
 * Host health alarms and their email topic. The worker publishes the metrics; the contract is in
 * src/aura-historia-worker/README.md#health-metrics.
 */
export interface MonitoringConfig {
  /**
   * Operator-owned SSM `String` parameter, in each monitoring stack's region, holding the address
   * alarms email. Deliberately unpublished, like the origin name.
   */
  readonly alarmEmailParameterName: string;
  readonly containerLogRetentionDays: number;
  /** The oldest waiting job may be this old before its class alarms. */
  readonly backlogAgeThresholdSeconds: { readonly normal: number; readonly bulk: number };
  readonly minimumFreeDiskPercent: number;
  readonly maximumBackupAgeSeconds: number;
  /**
   * Each scheduled ops job's longest time since its last success before it alarms, by the job
   * its timer runs (host/prod/systemd).
   */
  readonly maximumJobAgeSeconds: Readonly<Record<string, number>>;
  /**
   * The `api` stack's distributions. CloudFormation cannot reference them from the edge region,
   * so their IDs are pinned here; replacing a distribution must update them.
   */
  readonly cloudFrontDistributionIds: { readonly api: string; readonly mcp: string };
  /** The 5xx share of a 5-minute period that alarms, once the period has enough requests. */
  readonly cloudFront5xxRatePercent: number;
  readonly cloudFrontMinimumRequests: number;
}

export interface StageConfig {
  readonly stage: StageName;
  readonly removalPolicy: cdk.RemovalPolicy;
  readonly apiDomainName: string;
  readonly apiCloudFrontCertificateArn: string;
  readonly apiCloudFrontAliases: string[];
  readonly mcp: McpConfig;
  readonly hostOrigin: HostOriginConfig;
  readonly host: HostAccessConfig;
  readonly monitoring: MonitoringConfig;
  readonly cognitoCustomDomain: CognitoCustomDomainConfig | undefined;
  readonly cognitoCallbackUrls: string[];
  readonly cognitoLogoutUrls: string[];
  readonly cognitoIdentityProviders: readonly CognitoIdentityProviderConfig[];
  readonly cognitoEmail: CognitoEmailConfig | undefined;
  readonly emailSending: EmailSendingConfig;
}

export function isStageName(value: string): value is StageName {
  return (STAGES as readonly string[]).includes(value);
}

export function stageConfig(stage: StageName): StageConfig {
  if (!isStageName(stage)) throw new Error(`Unsupported CDK stage '${stage}'.`);
  const emailConfigurationSetName = `aura-historia-${stage}-email`;
  const apiDomainName = "api.aura-historia.com";

  return {
    stage,
    removalPolicy: cdk.RemovalPolicy.RETAIN,
    apiDomainName,
    apiCloudFrontCertificateArn: ssmValue(`/certificates/${stage}/api-cloudfront-certificate-arn`),
    apiCloudFrontAliases: [apiDomainName],
    mcp: {
      domainName: "mcp.aura-historia.com",
      cloudFrontCertificateArn: ssmValue(`/certificates/${stage}/mcp-cloudfront-certificate-arn`),
    },
    hostOrigin: {
      domainName: ssmValue(`/origin/${stage}/host-domain-name`),
      verifyHeaderName: "X-Origin-Verify",
    },
    host: {
      userName: `aura-historia-${stage}-host`,
      backupBucketName: `aura-historia-${stage}-backups-eu-central-1`,
      backupNoncurrentVersionRetentionDays: 30,
      containerLogGroupPrefix: `/aura-historia/${stage}/host/`,
      metricNamespace: "AuraHistoria/Host",
    },
    monitoring: {
      alarmEmailParameterName: `/monitoring/${stage}/alarm-email`,
      containerLogRetentionDays: 30,
      // From the retired queue alarms: 15 minutes, and the translation queue's observed jams.
      backlogAgeThresholdSeconds: { normal: 15 * 60, bulk: 12 * 60 * 60 },
      // Above OpenSearch's low disk watermark (85% used), which stops shard allocation.
      minimumFreeDiskPercent: 20,
      // A daily backup plus two hours' slack.
      maximumBackupAgeSeconds: 26 * 60 * 60,
      // One missed run of the twice-daily FX refresh or the daily matching (which stops itself
      // after two hours) alarms; the hourly cleanup alarms after two.
      maximumJobAgeSeconds: {
        "backend-cleanup": 150 * 60,
        "fx-refresh": 13 * 60 * 60,
        "periodic-match": 27 * 60 * 60,
      },
      cloudFrontDistributionIds: { api: "EP6DVM7VDGBW3", mcp: "ESDMRH0023HN6" },
      cloudFront5xxRatePercent: 5,
      cloudFrontMinimumRequests: 10,
    },
    cognitoCustomDomain: {
      domainName: "auth.aura-historia.com",
      certificateArn: ssmValue(`/certificates/${stage}/cognito-cloudfront-certificate-arn`),
    },
    cognitoCallbackUrls: [PROD_FRONTEND_URL],
    cognitoLogoutUrls: [PROD_FRONTEND_URL],
    cognitoIdentityProviders: [
      {
        kind: "google",
        providerName: "Google",
        clientIdParameterName: `/cognito/${stage}/identity-providers/google/client-id`,
        clientSecretField: "googleClientSecret",
        scopes: ["openid", "email", "profile"],
        existingEmailAction: "LINK_VERIFIED",
        linkSourceAttributeName: "Cognito_Subject",
      },
      {
        kind: "facebook",
        providerName: "Facebook",
        apiVersion: "v21.0",
        clientIdParameterName: `/cognito/${stage}/identity-providers/facebook/client-id`,
        clientSecretField: "facebookClientSecret",
        scopes: ["email", "public_profile"],
        existingEmailAction: "REJECT",
      },
    ],
    cognitoEmail: {
      configurationSet: emailConfigurationSetName,
      from: "Aura Historia <auth@notify.aura-historia.com>",
      identityDomain: "notify.aura-historia.com",
      replyTo: "contact@aura-historia.com",
    },
    emailSending: {
      configurationSet: emailConfigurationSetName,
      identityDomain: "notify.aura-historia.com",
    },
  };
}

/**
 * The consent-evidence log groups. They were created with the retired Lambdas, are retained
 * forever and outlive the stacks that created them; the host writes new evidence to them.
 */
export function consentEvidenceLogGroupNames(stage: StageName): readonly [string, string] {
  return [
    `/aws/lambda/aura-historia-api-${stage}`,
    `/aws/lambda/cognito-post-confirmation-${stage}`,
  ];
}

export function ssmValue(path: string): string {
  return `{{resolve:ssm:${path}}}`;
}
