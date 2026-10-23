import * as cdk from "aws-cdk-lib";
import * as budgets from "aws-cdk-lib/aws-budgets";
import * as codebuild from "aws-cdk-lib/aws-codebuild";
import * as codepipeline from "aws-cdk-lib/aws-codepipeline";
import * as actions from "aws-cdk-lib/aws-codepipeline-actions";
import * as codestarnotifications from "aws-cdk-lib/aws-codestarnotifications";
import * as ecr from "aws-cdk-lib/aws-ecr";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as sns from "aws-cdk-lib/aws-sns";
import * as path from "node:path";
import { Construct } from "constructs";
import { ARTIFACT_BUCKET_NAME, type StageName } from "./config";
import { CONTAINER_IMAGE_CATALOG } from "./container-image-catalog";
import { createEnvironmentRoles, type EnvironmentRoles } from "./delivery-iam";

export interface DeliveryStackProps extends cdk.StackProps {
  readonly enabledTargets?: readonly StageName[];
  readonly repository?: string;
  readonly connectionArn?: string;
  readonly controlKey: string;
  readonly controlVersion: string;
  readonly buildConfig: string;
  readonly buildConfigDigest?: string;
  readonly workflowRevision?: string;
  readonly githubProviderArn?: string;
  readonly monthlyBudgetUsd?: number;
}

/** Separately authorized control plane; application bundles cannot update this stack. */
export class DeliveryStack extends cdk.Stack {
  readonly releases: s3.Bucket;
  readonly buildPipeline: codepipeline.Pipeline;
  readonly deployments = new Map<StageName, codepipeline.Pipeline>();

  constructor(scope: Construct, id: string, props: DeliveryStackProps) {
    super(scope, id, { ...props, terminationProtection: true });
    const targets = props.enabledTargets ?? ["prod"];
    if (new Set(targets).size !== targets.length || targets.some((target) => !["dev", "prod"].includes(target)))
      throw new Error("Enabled delivery targets must be unique dev/prod names.");
    const pipelineVariables = new Map<codepipeline.Pipeline, codepipeline.Variable[]>();
    const connectionArn = props.connectionArn ?? new cdk.CfnParameter(this, "ConnectionArn", { type: "String" }).valueAsString;
    const buildConfigDigest = props.buildConfigDigest ?? new cdk.CfnParameter(this, "BuildConfigDigest", { type: "String", allowedPattern: "^[a-f0-9]{64}$" }).valueAsString;
    const workflowRevision = props.workflowRevision ?? new cdk.CfnParameter(this, "WorkflowRevision", { type: "String", allowedPattern: "^[a-f0-9]{40}$" }).valueAsString;
    const repository = props.repository ?? "aura-historia/backend";
    const repositoryParts = repository.split("/");
    if (repositoryParts.length !== 2) throw new Error("Expected owner/repository.");
    this.releases = new s3.Bucket(this, "ReleaseBundles", {
      bucketName: ARTIFACT_BUCKET_NAME, versioned: true, blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true, encryption: s3.BucketEncryption.S3_MANAGED, removalPolicy: cdk.RemovalPolicy.RETAIN,
      objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
    });
    const artifacts = new s3.Bucket(this, "PipelineArtifacts", {
      versioned: true, blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL, enforceSSL: true,
      encryption: s3.BucketEncryption.S3_MANAGED, removalPolicy: cdk.RemovalPolicy.RETAIN,
      lifecycleRules: [{ expiration: cdk.Duration.days(14), noncurrentVersionExpiration: cdk.Duration.days(14),
        abortIncompleteMultipartUploadAfter: cdk.Duration.days(1) }],
    });
    const immutable = ["objects/sha256/*", "mail/*", "bundles/*", "requests/*", "reports/*", "tests/*", "control/*", "claims/*", "builds/*", "maintenance/*", "evidence/*"];
    this.releases.addToResourcePolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
      principals: [new iam.AnyPrincipal()], actions: ["s3:PutObject"], resources: immutable.map((prefix) => this.releases.arnForObjects(prefix)),
      conditions: { Null: { "s3:if-none-match": "true" }, Bool: { "s3:ObjectCreationOperation": "true" } } }));
    this.releases.addToResourcePolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
      principals: [new iam.AnyPrincipal()], actions: ["s3:DeleteObject", "s3:DeleteObjectVersion"],
      resources: immutable.map((prefix) => this.releases.arnForObjects(prefix)) }));
    // The environment status key uses conditional CAS and has no TTL or delete path.
    this.releases.addToResourcePolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
      principals: [new iam.AnyPrincipal()], actions: ["s3:DeleteObject", "s3:DeleteObjectVersion"],
      resources: [this.releases.arnForObjects("environments/*")] }));
    this.releases.addToResourcePolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
      principals: [new iam.AnyPrincipal()], actions: ["s3:PutObject"], resources: [this.releases.arnForObjects("environments/*")],
      conditions: { Null: { "s3:if-match": "true", "s3:if-none-match": "true" } } }));
    const topic = new sns.Topic(this, "DeliveryNotifications", { displayName: "Aura Historia delivery failures and budgets" });
    const oidc = iam.OpenIdConnectProvider.fromOpenIdConnectProviderArn(this, "GitHubOidc",
      props.githubProviderArn ?? this.formatArn({ service: "iam", region: "", resource: "oidc-provider",
        resourceName: "token.actions.githubusercontent.com", arnFormat: cdk.ArnFormat.SLASH_RESOURCE_NAME }));
    const requester = (name: string, environment: string) => new iam.Role(this, name, {
      path: "/aura-historia/delivery/", assumedBy: new iam.WebIdentityPrincipal(oidc.openIdConnectProviderArn, {
        StringEquals: { "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
          "token.actions.githubusercontent.com:sub": environment === "__reader__" ? `repo:${repository}:ref:refs/heads/develop` : `repo:${repository}:environment:${environment}` },
      }),
    });
    const readRelease = (role: iam.IGrantable) => this.releases.grantRead(role);
    const requestVariables = ["RequestKey", "RequestVersion", "RequestDigest"].map((name) =>
      new codepipeline.Variable({ variableName: name, defaultValue: "UNAUTHORIZED" }));
    const source = (pipeline: codepipeline.Pipeline, candidate: boolean) => {
      const control = new codepipeline.Artifact("Control");
      const sourceActions: codepipeline.IAction[] = [new actions.S3SourceAction({
        actionName: "ControlSource", bucket: this.releases, bucketKey: props.controlKey,
        output: control, trigger: actions.S3Trigger.NONE, variablesNamespace: "ControlSource",
      })];
      const candidateSource = candidate ? new codepipeline.Artifact("Candidate") : undefined;
      if (candidateSource) sourceActions.push(new actions.CodeStarConnectionsSourceAction({
        actionName: "CandidateSource", owner: repositoryParts[0], repo: repositoryParts[1], branch: "develop",
        connectionArn, triggerOnPush: false, output: candidateSource, variablesNamespace: "CandidateSource",
      }));
      pipeline.addStage({ stageName: "Source", actions: sourceActions });
      return { control, candidate: candidateSource };
    };
    const admission = (_pipeline: codepipeline.Pipeline, target?: StageName) => {
      const fn = new lambda.Function(this, `${target ?? "build"}ApprovalAdmission`, {
        runtime: lambda.Runtime.NODEJS_22_X, handler: "aws-admission.handler", timeout: cdk.Duration.seconds(30),
        code: lambda.Code.fromAsset(path.join(__dirname, "../../ci/delivery"), { exclude: ["*.test.cjs"] }),
        environment: { DELIVERY_BUCKET: this.releases.bucketName, DELIVERY_REPOSITORY: repository,
          DELIVERY_CONTROL_KEY: props.controlKey, DELIVERY_CONTROL_VERSION: props.controlVersion,
          DELIVERY_WORKFLOW_REVISION: workflowRevision, DELIVERY_BUILD_CONFIG_DIGEST: buildConfigDigest,
          APPROVAL_SCOPE: target ? "DEPLOY" : "BUILD", APPROVAL_ENVIRONMENT: target ? `aws-${target}` : "aws-release-build",
          ...(target ? { DELIVERY_TARGET: target } : {}) },
        logGroup: new logs.LogGroup(this, `${target ?? "build"}AdmissionLogs`, { retention: logs.RetentionDays.ONE_MONTH }),
      });
      this.releases.grantRead(fn, "requests/*");
      this.releases.grantReadWrite(fn, "claims/*");
      return new actions.LambdaInvokeAction({ actionName: "AuthorizedRequest", lambda: fn,
        userParameters: { ...Object.fromEntries(requestVariables.map((variable) => [variable.variableName, variable.reference()])),
          ControlVersion: "#{ControlSource.VersionId}", ExecutionId: "#{codepipeline.PipelineExecutionId}",
          ...(target ? {} : { SourceSha: "#{CandidateSource.CommitId}" }) },
        variablesNamespace: "Authorization", runOrder: 1 });
    };
    const project = (name: string, command: string, role?: iam.IRole) => new codebuild.PipelineProject(this, name, {
      role: role ?? new iam.Role(this, `${name}ExecutionRole`, {
        assumedBy: new iam.ServicePrincipal("codebuild.amazonaws.com", { conditions: {
          StringEquals: { "aws:SourceAccount": this.account }, ArnEquals: { "aws:SourceArn": this.formatArn({
            service: "codebuild", resource: "project", resourceName: `aura-delivery-${name}` }) },
        } }),
      }), projectName: `aura-delivery-${name}`, timeout: cdk.Duration.minutes(name === "release-build" ? 120 : 30),
      queuedTimeout: cdk.Duration.minutes(60), concurrentBuildLimit: 1,
      environment: { buildImage: codebuild.LinuxBuildImage.STANDARD_7_0, computeType: name === "release-build" ? codebuild.ComputeType.MEDIUM : codebuild.ComputeType.SMALL,
        privileged: name === "release-build" },
      logging: { cloudWatch: { logGroup: new logs.LogGroup(this, `${name}Logs`, { retention: logs.RetentionDays.ONE_MONTH }) } },
      buildSpec: codebuild.BuildSpec.fromObject({ version: "0.2",
        env: { "exported-variables": ["MigrateActionMode", "MigrateChangeSetName", "ComputeActionMode", "ComputeChangeSetName",
          "ApiActionMode", "ApiChangeSetName", "ObservabilityActionMode", "ObservabilityChangeSetName"] },
        phases: { build: { commands: [command,
          ...["Migrate", "Compute", "Api", "Observability"].flatMap((boundary) => ["ActionMode", "ChangeSetName"].map((suffix) => {
            const name = `${boundary}${suffix}`;
            return `if [ -f prepared/action-variables.json ]; then export ${name}="$(node -p 'require("./prepared/action-variables.json").${name} || ""')"; fi`;
          }))] } },
        ...(name.endsWith("-admission") ? { artifacts: { files: ["prepared/**/*"], 'discard-paths': 'no' } } : {}) }),
      environmentVariables: { DELIVERY_BUCKET: { value: this.releases.bucketName },
        DELIVERY_CONTROL_VERSION: { value: props.controlVersion }, DELIVERY_BUILD_CONFIG: { value: props.buildConfig },
        AWS_ACCOUNT_ID: { value: this.account }, DELIVERY_DEV_ENABLED: { value: String(targets.includes("dev")) } },
    });
    this.buildPipeline = new codepipeline.Pipeline(this, "BuildPipeline", {
      pipelineName: "aura-release-build", pipelineType: codepipeline.PipelineType.V2,
      executionMode: codepipeline.ExecutionMode.QUEUED, artifactBucket: artifacts, variables: requestVariables,
      restartExecutionOnUpdate: false,
    });
    const buildSource = source(this.buildPipeline, true);
    this.buildPipeline.addStage({ stageName: "Authorize", actions: [admission(this.buildPipeline)] });
    pipelineVariables.set(this.buildPipeline, requestVariables);
    const reader = requester("GitHubBundleReader", "__reader__");
    readRelease(reader);
    reader.addToPolicy(new iam.PolicyStatement({ actions: ["ecr:DescribeImages"], resources: CONTAINER_IMAGE_CATALOG.map((image) => this.formatArn({ service: "ecr", resource: "repository", resourceName: image.repository })) }));
    new cdk.CfnOutput(this, "BundleReaderRoleArn", { value: reader.roleArn });
    const builder = project("release-build", 'DELIVERY_CANDIDATE_DIR="$CODEBUILD_SRC_DIR_Candidate" node ci/delivery/build.cjs');
    readRelease(builder);
    builder.addToRolePolicy(new iam.PolicyStatement({ actions: ["s3:PutObject"], resources: ["objects/sha256/*", "mail/*", "bundles/*", "builds/*"]
      .map((prefix) => this.releases.arnForObjects(prefix)) }));
    for (const image of CONTAINER_IMAGE_CATALOG) {
      const repository = new ecr.Repository(this, `${image.id}Repository`, {
        repositoryName: image.repository, imageTagMutability: ecr.TagMutability.IMMUTABLE,
        imageScanOnPush: true, removalPolicy: cdk.RemovalPolicy.RETAIN,
      });
      repository.grantPullPush(builder);
    }
    const vars = (pipeline: codepipeline.Pipeline, extra: Record<string, string> = {}) => Object.fromEntries([
      ...(pipelineVariables.get(pipeline) ?? ["RequestKey", "RequestVersion", "RequestDigest", "BundleDigest", "ReportKey", "ReportVersion", "ReportDigest"].map((variableName) => new codepipeline.Variable({ variableName }))).map((variable) => [`DELIVERY_${variable.variableName.replace(/([a-z])([A-Z])/g, "$1_$2").toUpperCase()}`,
        { value: variable.reference() }]),
      ["DELIVERY_EXECUTION_ID", { value: "#{codepipeline.PipelineExecutionId}" }],
      ...Object.entries(extra).map(([name, value]) => [name, { value }]),
    ]);
    this.buildPipeline.addStage({ stageName: "Build", actions: [new actions.CodeBuildAction({
      actionName: "BuildOnce", project: builder, input: buildSource.control, extraInputs: [buildSource.candidate!],
      environmentVariables: vars(this.buildPipeline, { DELIVERY_SOURCE_SHA: "#{CandidateSource.CommitId}" }),
    })] });
    const buildRequester = requester("GitHubBuildRequester", "aws-release-build");
    buildRequester.addToPolicy(new iam.PolicyStatement({ actions: ["codepipeline:StartPipelineExecution", "codepipeline:GetPipelineExecution", "codepipeline:ListActionExecutions"], resources: [this.buildPipeline.pipelineArn] }));
    this.releases.grantPut(buildRequester, "requests/build/*");
    this.releases.grantPut(buildRequester, "tests/*");
    readRelease(buildRequester);
    for (const stage of targets) {
      const roles = createEnvironmentRoles(this, stage, this.releases, artifacts);
      this.createDeployment(stage, { artifacts, source, project, admission, requester, readRelease, vars, roles, topic });
    }
    new codestarnotifications.NotificationRule(this, "BuildFailures", { source: this.buildPipeline,
      events: ["codepipeline-pipeline-pipeline-execution-failed"], targets: [topic] });
    if (props.monthlyBudgetUsd !== undefined) {
      if (!Number.isFinite(props.monthlyBudgetUsd) || props.monthlyBudgetUsd <= 0) throw new Error("Budget must be positive USD.");
      new budgets.CfnBudget(this, "DeliveryBudget", {
        budget: { budgetName: "aura-delivery", budgetType: "COST", timeUnit: "MONTHLY",
          budgetLimit: { amount: props.monthlyBudgetUsd, unit: "USD" },
          costFilters: { Service: ["AWS CodeBuild", "AWS CodePipeline"] } },
        notificationsWithSubscribers: [{ notification: { comparisonOperator: "GREATER_THAN", notificationType: "ACTUAL", threshold: 80,
          thresholdType: "PERCENTAGE" }, subscribers: [{ address: topic.topicArn, subscriptionType: "SNS" }] }],
      });
      topic.addToResourcePolicy(new iam.PolicyStatement({ principals: [new iam.ServicePrincipal("budgets.amazonaws.com")],
        actions: ["sns:Publish"], resources: [topic.topicArn], conditions: { StringEquals: { "aws:SourceAccount": this.account } } }));
    }
    new cdk.CfnOutput(this, "DeliveryBucket", { value: this.releases.bucketName });
    new cdk.CfnOutput(this, "BuildPipelineName", { value: this.buildPipeline.pipelineName });
    new cdk.CfnOutput(this, "BuildRequesterRoleArn", { value: buildRequester.roleArn });
    new cdk.CfnOutput(this, "DeliveryNotificationTopicArn", { value: topic.topicArn });
  }

  private createDeployment(stage: StageName, context: {
    artifacts: s3.IBucket;
    source: (pipeline: codepipeline.Pipeline, candidate: boolean) => { control: codepipeline.Artifact };
    project: (name: string, command: string, role?: iam.IRole) => codebuild.PipelineProject;
    admission: (pipeline: codepipeline.Pipeline, target?: StageName) => actions.LambdaInvokeAction;
    requester: (name: string, environment: string) => iam.Role;
    readRelease: (role: iam.IGrantable) => unknown;
    vars: (pipeline: codepipeline.Pipeline, extra?: Record<string, string>) => Record<string, codebuild.BuildEnvironmentVariable>;
    roles: EnvironmentRoles; topic: sns.ITopic;
  }): void {
    const variables = ["RequestKey", "RequestVersion", "RequestDigest", "BundleDigest", "ReportKey", "ReportVersion", "ReportDigest"]
      .map((variableName) => new codepipeline.Variable({ variableName, defaultValue: "UNAUTHORIZED" }));
    const pipeline = new codepipeline.Pipeline(this, `${stage}DeploymentPipeline`, {
      pipelineName: `aura-deploy-${stage}`, pipelineType: codepipeline.PipelineType.V2, executionMode: codepipeline.ExecutionMode.QUEUED,
      artifactBucket: context.artifacts, variables, restartExecutionOnUpdate: false,
    });
    const { control } = context.source(pipeline, false);
    const prepared = new codepipeline.Artifact("Prepared");
    const prepare = context.project(`${stage}-admission`, "node ci/delivery/pipeline.cjs admit", context.roles.planner);
    context.readRelease(prepare);
    this.releases.grantPut(prepare, `environments/${this.account}/${this.region}/${stage}.json`);
    const migration = context.project(`${stage}-migrate`, 'DELIVERY_PREPARED_DIR="$CODEBUILD_SRC_DIR_Prepared/prepared" node ci/delivery/pipeline.cjs migrate');
    context.readRelease(migration);
    migration.addToRolePolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({
      service: "lambda", resource: "function", resourceName: `database-migration-lambda-${stage}:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    const verify = context.project(`${stage}-verify`, 'DELIVERY_PREPARED_DIR="$CODEBUILD_SRC_DIR_Prepared/prepared" node ci/delivery/pipeline.cjs verify');
    context.readRelease(verify);
    this.releases.grantPut(verify, "evidence/*");
    this.releases.grantPut(migration, "evidence/*");
    migration.addToRolePolicy(new iam.PolicyStatement({ actions: ["cloudformation:DescribeStacks"], resources: [this.formatArn({ service: "cloudformation", resource: "stack", resourceName: `application-${stage}-migrate/*` })] }));
    this.releases.grantPut(verify, `environments/${this.account}/${this.region}/${stage}.json`);

    const grantInspectionReads = (role: iam.IGrantable) => {
      iam.Grant.addToPrincipal({ grantee: role,
        actions: ["dms:DescribeReplicationTasks", "dms:DescribeEndpoints", "dms:DescribeTableStatistics", "dms:DescribeReplicationInstances",
          "rds:DescribeDBInstances", "sts:GetCallerIdentity",
          "cloudwatch:GetMetricStatistics", "cloudwatch:DescribeAlarms", "kinesis:DescribeStreamSummary",
          "lambda:GetFunction", "lambda:GetFunctionConfiguration", "lambda:GetAlias", "lambda:ListAliases", "lambda:GetEventSourceMapping", "lambda:ListEventSourceMappings",
          "cloudformation:DescribeStacks", "cloudformation:DescribeStackEvents", "cloudformation:ListStackResources", "cloudformation:GetTemplate",
          "sqs:GetQueueAttributes", "sqs:GetQueueUrl", "ecs:DescribeTaskDefinition", "scheduler:GetSchedule", "events:DescribeRule", "events:ListTargetsByRule"], resourceArns: ["*"] });
      iam.Grant.addToPrincipal({ grantee: role, actions: ["ssm:GetParameter"],
        resourceArns: ["stripe", "shopify"].map((provider) => this.formatArn({
          service: "ssm", resource: "parameter", resourceName: `eventbridge/${stage}/${provider}-event-bus-name`,
        })) });
    };
    for (const job of [prepare, migration, verify]) {
      grantInspectionReads(job);
      this.releases.grantPut(job, "reports/executions/*");
      job.addToRolePolicy(new iam.PolicyStatement({ actions: ["ecr:DescribeImages"], resources: CONTAINER_IMAGE_CATALOG.map((image) => this.formatArn({ service: "ecr", resource: "repository", resourceName: image.repository })) }));
      job.addToRolePolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({ service: "lambda", resource: "function", resourceName: `database-inspection-lambda-${stage}:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    }
    prepare.addToRolePolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({
      service: "lambda", resource: "function", resourceName: `database-inspection-lambda-${stage}:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    const deploy = pipeline.addStage({ stageName: "Deploy", actions: [context.admission(pipeline, stage)] });
    deploy.addAction(new actions.CodeBuildAction({ actionName: "RevalidateAndHold", project: prepare, input: control,
      outputs: [prepared], environmentVariables: context.vars(pipeline, { DELIVERY_TARGET: stage, DELIVERY_TARGET_CONFIG: `ci/delivery/targets/${stage}.json`, DELIVERY_EXECUTION_ROLE_ARN: context.roles.execution.roleArn, DELIVERY_MIGRATION_EXECUTION_ROLE_ARN: context.roles.migrationExecution.roleArn }), variablesNamespace: "Admission", runOrder: 2 }));
    const native = (boundary: string, order: number) => {
      const action = new actions.CloudFormationCreateUpdateStackAction({
        actionName: `Deploy${boundary}`, stackName: `application-${stage}-${boundary.toLowerCase()}`,
        templatePath: prepared.atPath(`prepared/${boundary.toLowerCase()}.template.json`),
        templateConfiguration: prepared.atPath(`prepared/${boundary.toLowerCase()}.config.json`),
        adminPermissions: false, deploymentRole: boundary === "Migrate" ? context.roles.migrationExecution : context.roles.execution, role: context.roles.action,
        cfnCapabilities: [cdk.CfnCapabilities.NAMED_IAM], runOrder: order,
      });
      deploy.addAction(action);
      // Trusted admission exports EXECUTE for changed stacks, CREATE_UPDATE only for verified unchanged existing stacks.
      const resource = pipeline.node.defaultChild as codepipeline.CfnPipeline;
      const index = boundary === "Migrate" ? 2 : boundary === "Compute" ? 4 : boundary === "Api" ? 5 : 6;
      resource.addPropertyOverride(`Stages.1.Actions.${index}.Configuration.ActionMode`, `#{Admission.${boundary}ActionMode}`);
      resource.addPropertyOverride(`Stages.1.Actions.${index}.Configuration.ChangeSetName`, `#{Admission.${boundary}ChangeSetName}`);
    };
    native("Migrate", 3);
    deploy.addAction(new actions.CodeBuildAction({ actionName: "RunCandidateMigrations", project: migration, input: control,
      extraInputs: [prepared], environmentVariables: context.vars(pipeline, { DELIVERY_TARGET: stage, DELIVERY_TARGET_CONFIG: `ci/delivery/targets/${stage}.json` }), runOrder: 4 }));
    native("Compute", 5); native("Api", 6);
    if (stage === "prod") native("Observability", 7);
    deploy.addAction(new actions.CodeBuildAction({ actionName: "VerifyAndRecord", project: verify, input: control,
      extraInputs: [prepared], environmentVariables: context.vars(pipeline, { DELIVERY_TARGET: stage, DELIVERY_TARGET_CONFIG: `ci/delivery/targets/${stage}.json` }), runOrder: 8 }));
    const planRole = context.requester(`${stage}GitHubPlanner`, "__reader__");
    context.readRelease(planRole);
    planRole.addToPolicy(new iam.PolicyStatement({ actions: ["ecr:DescribeImages"], resources: CONTAINER_IMAGE_CATALOG.map((image) => this.formatArn({ service: "ecr", resource: "repository", resourceName: image.repository })) }));
    planRole.addToPolicy(new iam.PolicyStatement({ actions: ["cloudformation:CreateChangeSet", "cloudformation:DeleteChangeSet",
      "cloudformation:DescribeChangeSet", "cloudformation:DescribeStacks", "cloudformation:DescribeStackResources",
      "cloudformation:ListStackResources", "cloudformation:GetTemplate"], resources: [this.formatArn({
      service: "cloudformation", resource: "stack", resourceName: `application-${stage}-*/*` })] }));
    planRole.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [context.roles.execution.roleArn, context.roles.migrationExecution.roleArn],
      conditions: { StringEquals: { "iam:PassedToService": "cloudformation.amazonaws.com" } } }));
    grantInspectionReads(planRole);
    planRole.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({ service: "lambda", resource: "function", resourceName: `database-inspection-lambda-${stage}:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    this.releases.grantPut(planRole, "reports/*");
    this.releases.grantPut(planRole, "evidence/*");
    const modelArn = new cdk.CfnParameter(this, `${stage}ReviewerModelArn`, { type: "String",
      description: "Explicit strong reviewer model or inference-profile ARN authorized for compatibility review." });
    planRole.addToPolicy(new iam.PolicyStatement({ actions: ["bedrock:InvokeModel"], resources: [modelArn.valueAsString] }));
    const maintenance = context.requester(`${stage}Maintenance`, `aws-${stage}-maintenance`);
    context.readRelease(maintenance);
    this.releases.grantPut(maintenance, `environments/${this.account}/${this.region}/${stage}.json`);
    const taskArn = new cdk.CfnParameter(this, `${stage}MaintenanceDmsTaskArn`, { type: "String",
      allowedPattern: "^arn:aws:dms:[a-z0-9-]+:[0-9]{12}:task:[A-Z0-9]+$" });
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["dms:StopReplicationTask", "dms:StartReplicationTask"], resources: [taskArn.valueAsString] }));
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({
      service: "lambda", resource: "function", resourceName: `database-bootstrap-lambda-${stage}`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    grantInspectionReads(maintenance);
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [this.formatArn({ service: "lambda", resource: "function", resourceName: `database-inspection-lambda-${stage}:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["codepipeline:GetPipelineExecution", "codepipeline:ListActionExecutions"], resources: [pipeline.pipelineArn] }));
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["codebuild:BatchGetBuilds"], resources: [this.formatArn({ service: "codebuild", resource: "build", resourceName: `aura-delivery-${stage}-*:*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
    maintenance.addToPolicy(new iam.PolicyStatement({ actions: ["codedeploy:GetDeployment", "codedeploy:ListDeployments", "codedeploy:GetDeploymentGroup", "codedeploy:ListApplications", "codedeploy:ListDeploymentGroups"], resources: ["*"] }));
    this.releases.grantPut(maintenance, "evidence/*");
    this.releases.grantPut(maintenance, `maintenance/${stage}/*`);
    this.releases.grantPut(maintenance, "reports/manual/*");
    new cdk.CfnOutput(this, `${stage}PlannerRoleArn`, { value: planRole.roleArn });
    new cdk.CfnOutput(this, `${stage}MaintenanceRoleArn`, { value: maintenance.roleArn });
    const requestRole = context.requester(`${stage}GitHubDeploymentRequester`, `aws-${stage}`);
    requestRole.addToPolicy(new iam.PolicyStatement({ actions: ["codepipeline:StartPipelineExecution", "codepipeline:GetPipelineExecution", "codepipeline:ListActionExecutions"], resources: [pipeline.pipelineArn] }));
    this.releases.grantPut(requestRole, `requests/${stage}/*`);
    this.releases.grantPut(requestRole, "reports/*");
    this.releases.grantPut(requestRole, "tests/*");
    context.readRelease(requestRole);
    new codestarnotifications.NotificationRule(this, `${stage}DeploymentFailures`, { source: pipeline,
      events: ["codepipeline-pipeline-pipeline-execution-failed"], targets: [context.topic] });
    new cdk.CfnOutput(this, `${stage}DeploymentPipelineName`, { value: pipeline.pipelineName });
    new cdk.CfnOutput(this, `${stage}DeploymentRequesterRoleArn`, { value: requestRole.roleArn });
    new cdk.CfnOutput(this, `${stage}MigrationCloudFormationExecutionRoleArn`, { value: context.roles.migrationExecution.roleArn });
    new cdk.CfnOutput(this, `${stage}CloudFormationExecutionRoleArn`, { value: context.roles.execution.roleArn });
    this.deployments.set(stage, pipeline);
  }
}
