import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import type * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import type { StageName } from "./config";
import { CONTAINER_IMAGE_CATALOG } from "./container-image-catalog";
import lambdaBinaries from "../../ci/lambda-binaries.json";
import { boundaryArn } from "./routine-role-boundary";
import { routineStackNames } from "./release-artifacts";

export interface EnvironmentRoles {
  readonly execution: iam.Role;
  readonly migrationExecution: iam.Role;
  readonly action: iam.Role;
  readonly planner: iam.Role;
}

export function createEnvironmentRoles(scope: Construct, stage: StageName, bucket: s3.IBucket,
  artifacts: s3.IBucket, pipelineRole: iam.IRole): EnvironmentRoles {
  const stack = cdk.Stack.of(scope);
  const arn = (service: string, resource: string, resourceName: string, region = stack.region) =>
    stack.formatArn({ service, resource, resourceName, region,
      arnFormat: service === "secretsmanager" ? cdk.ArnFormat.COLON_RESOURCE_NAME : cdk.ArnFormat.SLASH_RESOURCE_NAME });
  const functionArn = (name: string) => stack.formatArn({ service: "lambda", resource: "function",
    resourceName: name, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME });
  const routineBinaries = [...lambdaBinaries.filter((binary) => !["database-migration-lambda", "fxrate-lambda"].includes(binary)), "api-cloudfront-web-acl-provider", "mcp-cloudfront-web-acl-provider"];
  const functionArns = routineBinaries.flatMap((binary) => [functionArn(`${binary}-${stage}`), functionArn(`${binary}-${stage}:*`)]);
  functionArns.push(functionArn(`CodeDeployHook_*_${stage}`));
  const hookTargets = ["aura-historia-api", "aura-historia-mcp"].flatMap((binary) =>
    [functionArn(`${binary}-${stage}`), functionArn(`${binary}-${stage}:*`)]);
  for (const kind of ["runtime", "migration"] as const) {
    const boundary = new iam.ManagedPolicy(scope, `${stage}${kind}Boundary`, {
      managedPolicyName: `${kind}-${stage}`, path: "/aura-historia/delivery/",
      statements: [
        new iam.PolicyStatement({ actions: [
          "logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents", "logs:DescribeLogGroups", "logs:PutRetentionPolicy",
          "ec2:CreateNetworkInterface", "ec2:DescribeNetworkInterfaces", "ec2:DescribeSubnets", "ec2:DeleteNetworkInterface",
          "ec2:AssignPrivateIpAddresses", "ec2:UnassignPrivateIpAddresses", "cloudwatch:PutMetricData",
          ...(kind === "runtime" ? ["geo-places:Geocode"] : []),
        ], resources: ["*"] }),
        new iam.PolicyStatement({ actions: ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"], resources: [
          arn("secretsmanager", "secret", `/aura-historia/${stage}/postgres/${kind === "runtime" ? "runtime" : "migrator"}-??????`),
        ] }),
        new iam.PolicyStatement({ actions: ["s3:GetObject", "s3:GetObjectVersion"], resources: [bucket.arnForObjects("objects/sha256/*"), bucket.arnForObjects("mail/*")] }),
        ...(kind === "runtime" ? [
          new iam.PolicyStatement({ actions: ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:ChangeMessageVisibility",
            "sqs:GetQueueAttributes", "sqs:GetQueueUrl", "sqs:SendMessage"], resources: [arn("sqs", `*-${stage}*`, "").replace(/\/$/, "")] }),
          new iam.PolicyStatement({ actions: ["kinesis:GetRecords", "kinesis:GetShardIterator", "kinesis:DescribeStream",
            "kinesis:DescribeStreamSummary", "kinesis:ListShards"], resources: [arn("kinesis", "stream", `*-${stage}*`)] }),
          new iam.PolicyStatement({ actions: ["s3:PutObject"], resources: [
            `arn:${stack.partition}:s3:::aura-historia-cdc-router-failures-${stage}/*`,
          ] }),
          new iam.PolicyStatement({ actions: ["es:ESHttpPost"], resources: [arn("es", "domain", `aura-historia-${stage}/product-listings/_doc/*`)] }),
          new iam.PolicyStatement({ actions: ["ses:SendEmail"], resources: [arn("ses", "identity", "notify.aura-historia.com"),
            arn("ses", "configuration-set", `aura-historia-${stage}-email`)] }),
          new iam.PolicyStatement({ actions: ["cognito-idp:ListUsers", "cognito-idp:AdminGetUser", "cognito-idp:AdminDeleteUser",
            "cognito-idp:AdminUserGlobalSignOut", "cognito-idp:AdminLinkProviderForUser"], resources: [arn("cognito-idp", "userpool", "*")] }),
          new iam.PolicyStatement({ actions: ["lambda:InvokeFunction", "lambda:GetAlias", "lambda:UpdateAlias", "lambda:GetFunctionConfiguration"], resources: hookTargets }),
          new iam.PolicyStatement({ actions: ["codedeploy:GetDeployment", "codedeploy:GetApplicationRevision", "codedeploy:PutLifecycleEventHookExecutionStatus"], resources: ["*"] }),
          new iam.PolicyStatement({ actions: ["wafv2:CreateWebACL", "wafv2:UpdateWebACL", "wafv2:DeleteWebACL", "wafv2:GetWebACL", "wafv2:ListWebACLs", "wafv2:TagResource"], resources: ["*"] }),
          new iam.PolicyStatement({ actions: ["lambda:InvokeFunction"], resources: [functionArn(`backend-cleanup-lambda-${stage}:*`), functionArn(`fxrate-lambda-${stage}`), functionArn(`CodeDeployHook_*_${stage}`)] }),
          new iam.PolicyStatement({ actions: ["cloudwatch:DescribeAlarms", "ecr:GetAuthorizationToken"], resources: ["*"] }),
          new iam.PolicyStatement({ actions: ["ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer", "ecr:BatchCheckLayerAvailability"], resources: CONTAINER_IMAGE_CATALOG.map((image) => arn("ecr", "repository", image.repository)) }),
          new iam.PolicyStatement({ actions: ["ssm:GetParameters"], resources: [arn("ssm", "parameter", `opensearch/${stage}/*`), arn("ssm", "parameter", `secrets/${stage}/*`)] }),
          new iam.PolicyStatement({ actions: ["ecs:RunTask"], resources: [arn("ecs", "task-definition", `*-${stage}:*`)] }),
          new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [arn("iam", "role", `aura-historia/runtime/${stage}/*`, "")], conditions: { StringEquals: { "iam:PassedToService": "ecs-tasks.amazonaws.com" } } }),
          new iam.PolicyStatement({ actions: ["s3:ListBucket"], resources: [`arn:${stack.partition}:s3:::aura-historia-cdc-router-failures-${stage}`] }),
        ] : []),
      ],
    });
    boundary.applyRemovalPolicy(cdk.RemovalPolicy.RETAIN);
  }
  const stacks = routineStackNames(stage).map((name) => arn("cloudformation", "stack", `${name}/*`));
  const execution = new iam.Role(scope, `${stage}CloudFormationExecution`, {
    roleName: `aura-delivery-${stage}-cloudformation`, path: "/aura-historia/delivery/",
    assumedBy: new iam.ServicePrincipal("cloudformation.amazonaws.com"),
  });
  execution.addToPolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
    actions: ["dms:*", "rds:*", "cognito-idp:Create*", "cognito-idp:Update*", "cognito-idp:Delete*", "kinesis:Create*",
      "kinesis:Delete*", "kinesis:Update*", "sqs:CreateQueue", "sqs:DeleteQueue", "sqs:SetQueueAttributes",
      "ec2:CreateVpc", "ec2:DeleteVpc", "secretsmanager:GetSecretValue", "sts:AssumeRole"], resources: ["*"] }));
  // Runtime resources are name/ownership checked by admission; service APIs without resource-level support use '*'.
  execution.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:*"], resources: functionArns }));
  execution.addToPolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY, actions: ["lambda:InvokeFunction"], resources: [
    stack.formatArn({ service: "lambda", resource: "function", resourceName: `database-bootstrap-lambda-${stage}*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME }),
    stack.formatArn({ service: "lambda", resource: "function", resourceName: `fxrate-lambda-${stage}*`, arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME }),
  ] }));
  execution.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:PublishLayerVersion", "lambda:GetLayerVersion", "lambda:DeleteLayerVersion"],
    resources: [stack.formatArn({ service: "lambda", resource: "layer", resourceName: "*", arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
  execution.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:CreateEventSourceMapping", "lambda:UpdateEventSourceMapping",
    "lambda:DeleteEventSourceMapping", "lambda:GetEventSourceMapping", "lambda:ListEventSourceMappings"], resources: ["*"] }));
  execution.addToPolicy(new iam.PolicyStatement({ actions: ["apigateway:*", "cloudfront:*", "wafv2:*", "codedeploy:*",
    "cloudwatch:*", "logs:*", "events:PutRule", "events:DeleteRule", "events:DescribeRule", "events:PutTargets", "events:RemoveTargets",
    "events:ListTargetsByRule", "ecs:RegisterTaskDefinition", "ecs:DeregisterTaskDefinition", "ecs:DescribeTaskDefinition",
    "ecs:CreateCluster", "ecs:DescribeClusters", "ecs:DeleteCluster", "ecs:TagResource", "ecs:UntagResource",
    "scheduler:CreateSchedule", "scheduler:UpdateSchedule", "scheduler:DeleteSchedule", "scheduler:GetSchedule",
    "ssm:GetParameters", "ssm:GetParameter", "ec2:Describe*", "s3:GetBucketLocation"], resources: ["*"] }));
  for (const kind of ["runtime"] as const) {
    const roles = arn("iam", "role", `aura-historia/${kind}/${stage}/*`, "");
    execution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:CreateRole", "iam:PutRolePermissionsBoundary"], resources: [roles],
      conditions: { StringEquals: { "iam:PermissionsBoundary": boundaryArn(stack, stage, kind) } } }));
    execution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:GetRole", "iam:DeleteRole", "iam:PutRolePolicy", "iam:DeleteRolePolicy",
      "iam:GetRolePolicy", "iam:AttachRolePolicy", "iam:DetachRolePolicy", "iam:UpdateAssumeRolePolicy", "iam:TagRole", "iam:UntagRole"], resources: [roles] }));
    execution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [roles], conditions: {
      StringEquals: { "iam:PassedToService": ["lambda.amazonaws.com", "ecs-tasks.amazonaws.com", "scheduler.amazonaws.com", "codedeploy.amazonaws.com"] },
    } }));
  }
  bucket.grantRead(execution, "objects/sha256/*");
  const migrationExecution = new iam.Role(scope, `${stage}MigrationCloudFormationExecution`, {
    roleName: `aura-delivery-${stage}-migration-cloudformation`, path: "/aura-historia/delivery/",
    assumedBy: new iam.ServicePrincipal("cloudformation.amazonaws.com"),
  });
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:CreateFunction", "lambda:DeleteFunction", "lambda:GetFunction",
    "lambda:GetFunctionConfiguration", "lambda:UpdateFunctionCode", "lambda:UpdateFunctionConfiguration", "lambda:PublishVersion",
    "lambda:ListVersionsByFunction", "lambda:TagResource", "lambda:UntagResource", "lambda:GetRuntimeManagementConfig", "lambda:PutRuntimeManagementConfig"],
    resources: [functionArn(`database-migration-lambda-${stage}`), functionArn(`database-migration-lambda-${stage}:*`)] }));
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["lambda:PublishLayerVersion", "lambda:GetLayerVersion", "lambda:DeleteLayerVersion"],
    resources: [stack.formatArn({ service: "lambda", resource: "layer", resourceName: "*", arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME })] }));
  const migrationRoles = arn("iam", "role", `aura-historia/migration/${stage}/*`, "");
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:CreateRole", "iam:PutRolePermissionsBoundary"], resources: [migrationRoles],
    conditions: { StringEquals: { "iam:PermissionsBoundary": boundaryArn(stack, stage, "migration") } } }));
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:GetRole", "iam:DeleteRole", "iam:PutRolePolicy", "iam:DeleteRolePolicy",
    "iam:GetRolePolicy", "iam:AttachRolePolicy", "iam:DetachRolePolicy", "iam:UpdateAssumeRolePolicy", "iam:TagRole", "iam:UntagRole"], resources: [migrationRoles] }));
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [migrationRoles],
    conditions: { StringEquals: { "iam:PassedToService": "lambda.amazonaws.com" } } }));
  migrationExecution.addToPolicy(new iam.PolicyStatement({ actions: ["ec2:DescribeSecurityGroups", "ec2:DescribeSubnets", "ec2:DescribeVpcs"], resources: ["*"] }));
  bucket.grantRead(migrationExecution, "objects/sha256/*");
  const action = new iam.Role(scope, `${stage}CloudFormationAction`, {
    roleName: `aura-delivery-${stage}-action`, path: "/aura-historia/delivery/",
    assumedBy: pipelineRole.grantPrincipal,
  });
  action.addToPolicy(new iam.PolicyStatement({ actions: ["cloudformation:ExecuteChangeSet", "cloudformation:DescribeChangeSet",
    "cloudformation:DescribeStacks", "cloudformation:DescribeStackEvents", "cloudformation:GetTemplate", "cloudformation:UpdateStack"], resources: stacks,
    conditions: { StringEqualsIfExists: { "cloudformation:RoleArn": [execution.roleArn, migrationExecution.roleArn] } } }));
  action.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [execution.roleArn, migrationExecution.roleArn],
    conditions: { StringEquals: { "iam:PassedToService": "cloudformation.amazonaws.com" } } }));
  action.addToPolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY,
    actions: ["cloudformation:CreateStack", "cloudformation:DeleteStack", "cloudformation:SetStackPolicy"], resources: ["*"] }));
  action.addToPolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY, actions: ["iam:PassRole"],
    notResources: [execution.roleArn, migrationExecution.roleArn] }));
  action.addToPolicy(new iam.PolicyStatement({ effect: iam.Effect.DENY, actions: ["iam:PassRole"], resources: ["*"],
    conditions: { StringNotEquals: { "iam:PassedToService": "cloudformation.amazonaws.com" } } }));
  artifacts.grantRead(action);
  const planner = new iam.Role(scope, `${stage}Planner`, {
    roleName: `aura-delivery-${stage}-planner`, path: "/aura-historia/delivery/",
    assumedBy: new iam.ServicePrincipal("codebuild.amazonaws.com", { conditions: {
      StringEquals: { "aws:SourceAccount": stack.account }, ArnEquals: { "aws:SourceArn": arn("codebuild", "project", `aura-delivery-${stage}-admission`) },
    } }),
  });
  planner.addToPolicy(new iam.PolicyStatement({ actions: ["cloudformation:CreateChangeSet", "cloudformation:DeleteChangeSet",
    "cloudformation:DescribeChangeSet", "cloudformation:DescribeStacks", "cloudformation:DescribeStackResources",
    "cloudformation:ListStackResources", "cloudformation:GetTemplate"], resources: stacks,
    conditions: { StringEqualsIfExists: { "cloudformation:RoleArn": [execution.roleArn, migrationExecution.roleArn] } } }));
  planner.addToPolicy(new iam.PolicyStatement({ actions: ["iam:PassRole"], resources: [execution.roleArn, migrationExecution.roleArn],
    conditions: { StringEquals: { "iam:PassedToService": "cloudformation.amazonaws.com" } } }));
  return { execution, migrationExecution, action, planner };
}
