import * as cdk from "aws-cdk-lib";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as codedeploy from "aws-cdk-lib/aws-codedeploy";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as fs from "node:fs";
import * as path from "node:path";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

export interface ReleaseRolloutProps {
  readonly config: StageConfig;
  readonly api: lambda.Alias;
  readonly mcp: lambda.Alias;
}

/** Hooks and alarms precede alias updates in the same stack, avoiding observability cycles. */
export class ReleaseRollout extends Construct {
  constructor(scope: Construct, id: string, props: ReleaseRolloutProps) {
    super(scope, id);
    const profile = new cdk.CfnParameter(cdk.Stack.of(this), "RolloutProfile", {
      type: "String", default: props.config.isProd ? "Canary10Percent5Minutes" : "AllAtOnce",
      allowedValues: ["AllAtOnce", "Canary10Percent5Minutes", "Canary10Percent10Minutes"],
    });
    const deploymentConfig = codedeploy.LambdaDeploymentConfig.fromLambdaDeploymentConfigName(this,
      "RolloutConfig", cdk.Fn.join("", ["CodeDeployDefault.Lambda", profile.valueAsString]));
    for (const [kind, alias, host] of [
      ["api", props.api, props.config.apiDomainName!], ["mcp", props.mcp, props.config.mcp.domainName],
    ] as const) {
      const fn = alias.lambda;
      const alarm = new cloudwatch.Alarm(this, `${kind}CandidateErrors`, {
        metric: new cloudwatch.MathExpression({
          expression: "IF(invocations >= 3, 100 * errors / invocations, 0)",
          usingMetrics: { errors: fn.metricErrors({ statistic: "Sum", period: cdk.Duration.minutes(1) }),
            invocations: fn.metricInvocations({ statistic: "Sum", period: cdk.Duration.minutes(1) }) },
          period: cdk.Duration.minutes(1),
        }),
        threshold: 1, evaluationPeriods: 1, datapointsToAlarm: 1,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
        alarmDescription: "Controlled candidate probes supply samples; missing traffic is verified by required hooks.",
      });
      const hooks = ["pre", "post"].map((phase) => {
        const hook = new lambda.Function(this, `${kind}${phase}TrafficCheck`, {
          functionName: `CodeDeployHook_${kind}_${phase}_${props.config.stage}`,
          runtime: lambda.Runtime.NODEJS_22_X, handler: "index.handler",
          code: lambda.Code.fromInline(fs.readFileSync(path.join(__dirname, "../../../ci/delivery/codedeploy-hook.cjs"), "utf8")),
          timeout: cdk.Duration.minutes(5), memorySize: 256,
          environment: { FUNCTION_NAME: fn.functionName, PROBE_KIND: kind, PROBE_HOST: host, PROBE_PHASE: phase },
          logGroup: new logs.LogGroup(this, `${kind}${phase}TrafficLogs`, {
            retention: logs.RetentionDays.ONE_MONTH, removalPolicy: cdk.RemovalPolicy.RETAIN,
          }),
        });
        hook.addToRolePolicy(new iam.PolicyStatement({ actions: ["lambda:InvokeFunction", "lambda:GetAlias"],
          resources: [fn.functionArn, `${fn.functionArn}:*`] }));
        hook.addToRolePolicy(new iam.PolicyStatement({ actions: ["codedeploy:ListDeploymentTargets", "codedeploy:GetDeploymentTarget"],
          resources: ["*"] }));
        return hook;
      });
      const group = new codedeploy.LambdaDeploymentGroup(this, `${kind}Deployment`, {
        alias, deploymentConfig, alarms: [alarm], preHook: hooks[0], postHook: hooks[1],
        autoRollback: { failedDeployment: true, stoppedDeployment: true, deploymentInAlarm: true },
        ignorePollAlarmsFailure: false,
      });
      group.node.addDependency(alarm);
      new cdk.CfnOutput(cdk.Stack.of(this), `${kind}DeploymentGroupName`, { value: group.deploymentGroupName });
    }
  }
}
