import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as events from "aws-cdk-lib/aws-events";
import * as targets from "aws-cdk-lib/aws-events-targets";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as actions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as sns from "aws-cdk-lib/aws-sns";
import { Construct } from "constructs";
import { join } from "node:path";

/** Operational ownership is independent of the application stacks it protects. */
export class ReleaseOperationsStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: cdk.StackProps = {}) {
    super(scope, id, props);
    const bucket = new s3.Bucket(this, "ReleaseState", {
      versioned: true,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });
    bucket.addToResourcePolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.DENY,
        principals: [new iam.AnyPrincipal()],
        actions: ["s3:PutObject"],
        resources: [bucket.arnForObjects("stages/*")],
        conditions: { Null: { "s3:if-match": "true", "s3:if-none-match": "true" } },
      }),
    );
    bucket.addToResourcePolicy(
      new iam.PolicyStatement({
        effect: iam.Effect.DENY,
        principals: [new iam.AnyPrincipal()],
        actions: ["s3:DeleteObject", "s3:DeleteObjectVersion"],
        resources: [bucket.arnForObjects("stages/*")],
      }),
    );
    const topic = new sns.Topic(this, "ReleaseAlarmTopic", { displayName: "Aura Historia release recovery" });
    const monitor = new lambda.Function(this, "PauseMonitor", {
      runtime: lambda.Runtime.NODEJS_24_X,
      handler: "index.handler",
      code: lambda.Code.fromAsset(join(__dirname, "resources/release-monitor")),
      timeout: cdk.Duration.seconds(60),
      memorySize: 128,
      logGroup: new logs.LogGroup(this, "MonitorLogs", { retention: logs.RetentionDays.ONE_MONTH }),
      environment: { STATE_BUCKET: bucket.bucketName, ACCOUNT: this.account },
    });
    // GetObject needs ListBucket to distinguish an unadopted stage from denied access.
    monitor.addToRolePolicy(new iam.PolicyStatement({ actions: ["s3:ListBucket"], resources: [bucket.bucketArn] }));
    for (const stage of ["dev", "prod"]) {
      monitor.addToRolePolicy(
        new iam.PolicyStatement({
          actions: ["s3:GetObject"],
          resources: [bucket.arnForObjects(`stages/${this.account}/${this.region}/${stage}.json`)],
        }),
      );
      for (const [suffix, metricName] of [
        ["overdue", "PauseOverdue"],
        ["monitor", "MonitorError"],
      ]) {
        new cloudwatch.Alarm(this, `${stage}${suffix}`, {
          alarmName: `aura-historia-release-${stage}-${suffix}`,
          alarmDescription:
            "Release ownership needs operator attention. Never automatically clear ownership or resume DMS.",
          metric: new cloudwatch.Metric({
            namespace: "AuraHistoria/Release",
            metricName,
            dimensionsMap: { Stage: stage },
            period: cdk.Duration.minutes(1),
            statistic: "Maximum",
          }),
          threshold: 1,
          evaluationPeriods: 2,
          datapointsToAlarm: 1,
          treatMissingData: cloudwatch.TreatMissingData.BREACHING,
        }).addAlarmAction(new actions.SnsAction(topic));
      }
    }
    monitor.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ["cloudwatch:PutMetricData"],
        resources: ["*"],
        conditions: { StringEquals: { "cloudwatch:namespace": "AuraHistoria/Release" } },
      }),
    );
    new events.Rule(this, "PauseMonitorSchedule", {
      schedule: events.Schedule.rate(cdk.Duration.minutes(1)),
      targets: [new targets.LambdaFunction(monitor)],
    });
    new cdk.CfnOutput(this, "ReleaseStateBucket", { value: bucket.bucketName });
    new cdk.CfnOutput(this, "ReleaseAlarmTopicArn", { value: topic.topicArn });
  }
}
