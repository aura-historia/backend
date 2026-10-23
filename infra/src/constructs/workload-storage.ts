import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { periodicMatcherNames } from "../periodic-matcher-config";

/** Failure custody survives ordinary workload and scheduler releases. */
export class WorkloadStorage extends Construct {
  readonly maintenanceDlq: sqs.Queue;
  readonly periodicMatcherDlq: sqs.Queue;
  readonly routerFailureArchive: s3.Bucket;
  constructor(scope: Construct, id: string, config: StageConfig) {
    super(scope, id);
    const queue = (id: string, queueName: string) => new sqs.Queue(this, id, {
      queueName, encryption: sqs.QueueEncryption.SQS_MANAGED, enforceSSL: true,
      retentionPeriod: cdk.Duration.days(14), removalPolicy: config.removalPolicy,
    });
    this.maintenanceDlq = queue("MaintenanceSchedulerDeadLetterQueue", `aura-historia-maintenance-scheduler-dlq-${config.stage}`);
    this.periodicMatcherDlq = queue("PeriodicMatcherDeliveryDlq", periodicMatcherNames(config.stage).dlq);
    this.routerFailureArchive = new s3.Bucket(this, "CdcRouterFailureArchive", {
      bucketName: `aura-historia-cdc-router-failures-${config.stage}`,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL, encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true, lifecycleRules: [{ enabled: true, expiration: cdk.Duration.days(90) }],
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });
  }
}
