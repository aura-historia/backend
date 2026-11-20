import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as ses from "aws-cdk-lib/aws-ses";
import { Construct } from "constructs";
import { CLOUDFORMATION_STAGING_BUCKET_NAME, EDGE_METRICS_REGION, stageConfig, type StageConfig, type StageName } from "./config";
import { constrainRoutineRoles } from "./routine-role-boundary";
import { ApiCloudFront } from "./constructs/api";
import { Identity } from "./constructs/cognito";
import { CognitoTriggers } from "./constructs/cognito-triggers";
import { HostAccess } from "./constructs/host-access";
import { EdgeMonitoring, HostMonitoring } from "./constructs/host-monitoring";
import { McpCloudFront } from "./constructs/mcp";
import { TriggerArtifactPublisher } from "./constructs/trigger-artifact-publisher";

export interface ApplicationStackProps extends cdk.StackProps {
  readonly stage: StageName;
}

export interface ApplicationStageProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly stackNamePrefix?: string;
}

/** The resources that stay on AWS while the application runs on the self-hosted host. */
export interface ApplicationStageStacks {
  readonly host: ApplicationHostStack;
  readonly identity: ApplicationIdentityStack;
  readonly api: ApplicationApiStack;
  readonly monitoring: ApplicationMonitoringStack;
  readonly edgeMonitoring: ApplicationEdgeMonitoringStack;
}

export function createApplicationStacks(scope: Construct, props: ApplicationStageProps): ApplicationStageStacks {
  const stackNamePrefix = props.stackNamePrefix ?? `application-${props.stage}`;
  const baseProps = stackBaseProps(props);
  const stack = (suffix: string) => ({ ...baseProps, stage: props.stage, stackName: `${stackNamePrefix}-${suffix}` });

  const host = new ApplicationHostStack(scope, `${stackNamePrefix}-host`, stack("host"));
  // Both name host resources rather than importing them: the host user and the origin secret.
  const identity = new ApplicationIdentityStack(scope, `${stackNamePrefix}-identity`, stack("identity"));
  identity.addStackDependency(host);
  const api = new ApplicationApiStack(scope, `${stackNamePrefix}-api`, stack("api"));
  api.addStackDependency(host);
  // Both name what they watch rather than importing it, so either deploys on its own.
  const monitoring = new ApplicationMonitoringStack(scope, `${stackNamePrefix}-monitoring`, stack("monitoring"));
  const edgeMonitoring = new ApplicationEdgeMonitoringStack(scope, `${stackNamePrefix}-edge-monitoring`, {
    ...stack("edge-monitoring"),
    env: { account: baseProps.env?.account, region: EDGE_METRICS_REGION },
  });

  return { host, identity, api, monitoring, edgeMonitoring };
}

/**
 * Backups, the host's IAM user, the origin secret and the role that publishes the Cognito trigger
 * code; deployable before the host serves traffic.
 */
export class ApplicationHostStack extends cdk.Stack {
  readonly access: HostAccess;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));
    this.templateOptions.description = "Aura Historia host access stack";
    this.access = new HostAccess(this, "HostAccess", { config: stageConfig(props.stage) });
    const publisher = new TriggerArtifactPublisher(this, "TriggerArtifactPublisher", { config: stageConfig(props.stage) });

    new cdk.CfnOutput(this, "HostUserName", { value: this.access.user.userName });
    new cdk.CfnOutput(this, "BackupBucketName", { value: this.access.backupBucket.bucketName });
    new cdk.CfnOutput(this, "OriginVerifySecretArn", { value: this.access.originVerifySecret.secretArn });
    new cdk.CfnOutput(this, "TriggerArtifactPublisherRoleArn", { value: publisher.role.roleArn });
  }
}

export class ApplicationIdentityStack extends cdk.Stack {
  readonly identity: Identity;
  readonly triggers: CognitoTriggers;
  readonly emailConfigurationSet: ses.ConfigurationSet;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));
    const config = stageConfig(props.stage);
    const stageName = config.stage;
    this.emailConfigurationSet = new ses.ConfigurationSet(this, "EmailConfigurationSet", {
      configurationSetName: config.emailSending.configurationSet,
      reputationMetrics: true,
    });
    // Outstanding sender versions and the host may still reference this set.
    this.emailConfigurationSet.applyRemovalPolicy(cdk.RemovalPolicy.RETAIN);
    const emailMetrics = this.emailConfigurationSet.addEventDestination("CloudWatchMetrics", {
      destination: ses.EventDestination.cloudWatchDimensions([{
        name: "stage",
        source: ses.CloudWatchDimensionSource.MESSAGE_TAG,
        // Senders omit this tag; the stage default keeps metrics low-cardinality and non-sensitive.
        defaultValue: stageName,
      }]),
      events: [
        ses.EmailSendingEvent.SEND,
        ses.EmailSendingEvent.DELIVERY,
        ses.EmailSendingEvent.BOUNCE,
        ses.EmailSendingEvent.COMPLAINT,
        ses.EmailSendingEvent.DELIVERY_DELAY,
        ses.EmailSendingEvent.RENDERING_FAILURE,
        ses.EmailSendingEvent.OPEN,
        ses.EmailSendingEvent.CLICK,
        ses.EmailSendingEvent.SUBSCRIPTION,
      ],
    });

    this.triggers = new CognitoTriggers(this, "CognitoTriggers", { config });
    this.identity = new Identity(this, "Identity", {
      config, stageName, emailConfigurationSet: this.emailConfigurationSet,
      postConfirmationTrigger: this.triggers.postConfirmation,
      preSignUpTrigger: this.triggers.preSignUp,
    });
    this.identity.userPool.node.addDependency(emailMetrics);
    grantHostPoolAdministration(this, config, this.identity);
    retainRetiredStackExports(this, this.identity, this.emailConfigurationSet);

    new cdk.CfnOutput(this, "CognitoUserPoolId", { value: this.identity.userPool.userPoolId });
    new cdk.CfnOutput(this, "CognitoUserPoolClientPublicId", { value: this.identity.publicClient.userPoolClientId });
    new cdk.CfnOutput(this, "CognitoHostedUIDomain", {
      value: (this.identity.customDomain ?? this.identity.domain).baseUrl(),
    });
    if (this.identity.customDomain) {
      new cdk.CfnOutput(this, "CognitoCustomDomainCloudFrontDomainName", {
        value: this.identity.customDomain.cloudFrontEndpoint,
      });
    }
  }
}

/**
 * Managed rather than inline, like the host stack's grants, so the user's policies stay clear
 * of IAM's aggregate inline-policy size limit. The Cognito admin calls the host makes: the REST API's user lookup and global sign-out, and
 * the identity deletion worker's lookup and deletion. The pool's own stack grants them, so the
 * statement names the pool exactly.
 */
function grantHostPoolAdministration(stack: cdk.Stack, config: StageConfig, identity: Identity): void {
  new iam.ManagedPolicy(stack, "HostCognitoAdministration", {
    users: [iam.User.fromUserName(stack, "HostUser", config.host.userName)],
    statements: [new iam.PolicyStatement({
      actions: [
        "cognito-idp:ListUsers",
        "cognito-idp:AdminGetUser",
        "cognito-idp:AdminDeleteUser",
        "cognito-idp:AdminUserGlobalSignOut",
      ],
      resources: [identity.userPool.userPoolArn],
    })],
  });
}

/**
 * The retired compute stack imports these values until it is deleted, and CloudFormation refuses
 * to remove an export that is still imported. Remove this once that stack is gone.
 */
function retainRetiredStackExports(stack: cdk.Stack, identity: Identity, emailConfigurationSet: ses.ConfigurationSet): void {
  stack.exportValue(emailConfigurationSet.configurationSetName);
  stack.exportValue(identity.userPool.userPoolArn);
  stack.exportValue(identity.publicClient.userPoolClientId);
  stack.exportValue(identity.userPool.userPoolId);
  if (identity.customDomain) stack.exportValue(identity.customDomain.domainName);
}

/** The public CloudFront distributions and Web ACLs for `api.` and `mcp.`, both on the host origin. */
export class ApplicationApiStack extends cdk.Stack {
  readonly api: ApiCloudFront;
  readonly mcp: McpCloudFront;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage);
    this.templateOptions.description = "Aura Historia API stack";
    constrainRoutineRoles(this, props.stage);

    // The construct IDs predate the host origin; they keep the deployed resources' logical IDs.
    this.api = new ApiCloudFront(this, "HttpApi", { config });
    this.mcp = new McpCloudFront(this, "McpHttpApi", { config });
    new cdk.CfnOutput(this, "McpEndpointUrl", { value: this.mcp.endpointUrl });
    new cdk.CfnOutput(this, "McpCloudFrontDistributionDomainName", { value: this.mcp.distribution.attrDomainName });
    new cdk.CfnOutput(this, "ApiCloudFrontDistributionDomainName", { value: this.api.distribution.attrDomainName });
  }
}

/** The host's container log groups, the host health alarms and their email topic. */
export class ApplicationMonitoringStack extends cdk.Stack {
  readonly monitoring: HostMonitoring;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));
    this.templateOptions.description = "Aura Historia host monitoring stack";
    this.monitoring = new HostMonitoring(this, "HostMonitoring", { config: stageConfig(props.stage) });
    new cdk.CfnOutput(this, "AlarmTopicArn", { value: this.monitoring.alarms.topic.topicArn });
  }
}

/**
 * The CloudFront 5xx alarms and their own email topic, in the only region CloudFront publishes
 * metrics to. The region has no CDK bootstrap: the legacy synthesizer names no bootstrap roles
 * (the bootstrapless one still does), so the CLI deploys with its own credentials, and the
 * template has no assets and is small enough to pass inline.
 */
export class ApplicationEdgeMonitoringStack extends cdk.Stack {
  readonly monitoring: EdgeMonitoring;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, { ...stackProps(props), synthesizer: new cdk.LegacyStackSynthesizer() });
    this.templateOptions.description = "Aura Historia edge monitoring stack";
    this.monitoring = new EdgeMonitoring(this, "EdgeMonitoring", { config: stageConfig(props.stage) });
    new cdk.CfnOutput(this, "AlarmTopicArn", { value: this.monitoring.alarms.topic.topicArn });
  }
}

function stackBaseProps(props: ApplicationStageProps): cdk.StackProps {
  const { stackNamePrefix: _stackNamePrefix, stage: _stage, ...stackProps } = props;
  return stackProps;
}

function stackProps(props: ApplicationStackProps): cdk.StackProps {
  const { stage, ...rest } = props;
  return {
    ...rest,
    terminationProtection: true,
    synthesizer: new cdk.CliCredentialsStackSynthesizer({
      fileAssetsBucketName: CLOUDFORMATION_STAGING_BUCKET_NAME,
      bucketPrefix: `${stage}/`,
    }),
  };
}
