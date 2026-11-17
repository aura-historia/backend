import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as secretsmanager from "aws-cdk-lib/aws-secretsmanager";
import { Construct } from "constructs";
import { consentEvidenceLogGroupNames, WORKLOAD_REGION, type StageConfig } from "../config";
import { credentialSecrets } from "../credentials";

/** Object prefixes the host writes: pgBackRest's repository and the encrypted secrets copy. */
export const BACKUP_PREFIXES = ["pgbackrest/", "secrets/"] as const;

/**
 * Actions the host user may never take on the backup bucket, whatever its IAM policies say:
 * removing an old object version or the settings that keep old versions.
 */
export const BACKUP_PROTECTION_ACTIONS = [
  "s3:DeleteObjectVersion",
  "s3:PutBucketVersioning",
  "s3:PutLifecycleConfiguration",
  "s3:PutBucketPolicy",
  "s3:DeleteBucketPolicy",
  "s3:DeleteBucket",
] as const;

export interface HostAccessProps {
  readonly config: StageConfig;
}

/**
 * What the self-hosted host needs from AWS before it serves traffic: an IAM user for its runtime
 * calls, the versioned bucket its backups go to, and the secret CloudFront sends to its origin.
 * Access keys are created and rotated by an operator, never by CloudFormation.
 */
export class HostAccess extends Construct {
  readonly user: iam.User;
  readonly backupBucket: s3.Bucket;
  readonly originVerifySecret: secretsmanager.Secret;

  constructor(scope: Construct, id: string, props: HostAccessProps) {
    super(scope, id);
    const { config } = props;

    this.user = new iam.User(this, "HostUser", { userName: config.host.userName });
    this.backupBucket = createBackupBucket(this, config, this.user);
    this.originVerifySecret = new secretsmanager.Secret(this, "OriginVerifySecret", {
      secretName: credentialSecrets(config.stage).originVerify.secretName,
      description: "Header value CloudFront sends to the host origin; the host proxy rejects requests without it.",
      generateSecretString: {
        secretStringTemplate: "{}",
        generateStringKey: "value",
        passwordLength: 64,
        excludePunctuation: true,
      },
      removalPolicy: cdk.RemovalPolicy.RETAIN,
    });

    grantBackups(this, this.user, this.backupBucket);
    grantEmailSending(this, config, this.user);
    grantPlaceLookups(this, this.user);
    grantLogsAndMetrics(this, config, this.user);
  }
}

function createBackupBucket(scope: Construct, config: StageConfig, user: iam.IUser): s3.Bucket {
  const bucket = new s3.Bucket(scope, "BackupBucket", {
    bucketName: config.host.backupBucketName,
    versioned: true,
    enforceSSL: true,
    minimumTLSVersion: 1.2,
    blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
    encryption: s3.BucketEncryption.S3_MANAGED,
    objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
    removalPolicy: cdk.RemovalPolicy.RETAIN,
    lifecycleRules: [{
      id: "ExpireReplacedVersions",
      // pgBackRest expires its own backups; a deleted or overwritten object stays recoverable
      // this long, so a compromised host cannot destroy the history it is denied access to.
      noncurrentVersionExpiration: cdk.Duration.days(config.host.backupNoncurrentVersionRetentionDays),
      expiredObjectDeleteMarker: true,
      abortIncompleteMultipartUploadAfter: cdk.Duration.days(7),
    }],
  });
  bucket.addToResourcePolicy(new iam.PolicyStatement({
    sid: "DenyHostVersionRemoval",
    effect: iam.Effect.DENY,
    principals: [new iam.ArnPrincipal(user.userArn)],
    actions: [...BACKUP_PROTECTION_ACTIONS],
    resources: [bucket.bucketArn, bucket.arnForObjects("*")],
  }));
  return bucket;
}

/** pgBackRest lists, reads, writes and expires current objects; old versions stay out of reach. */
function grantBackups(scope: Construct, user: iam.IUser, bucket: s3.IBucket): void {
  new iam.Policy(scope, "BackupPolicy", {
    users: [user],
    statements: [
      new iam.PolicyStatement({
        actions: ["s3:ListBucket", "s3:GetBucketLocation"],
        resources: [bucket.bucketArn],
      }),
      new iam.PolicyStatement({
        actions: ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
        resources: BACKUP_PREFIXES.map((prefix) => bucket.arnForObjects(`${prefix}*`)),
      }),
    ],
  });
}

/** Senders select the managed configuration set explicitly, never the identity's default. */
function grantEmailSending(scope: Construct, config: StageConfig, user: iam.IUser): void {
  const stack = cdk.Stack.of(scope);
  new iam.Policy(scope, "EmailSendingPolicy", {
    users: [user],
    statements: [new iam.PolicyStatement({
      actions: ["ses:SendEmail"],
      resources: [
        stack.formatArn({ service: "ses", resource: "identity", resourceName: config.emailSending.identityDomain }),
        stack.formatArn({ service: "ses", resource: "configuration-set", resourceName: config.emailSending.configurationSet }),
      ],
    })],
  });
}

/**
 * Geocode and GetPlace resolve typed and picked addresses for storage (geographic worker);
 * Autocomplete serves display-only address suggestions (REST API).
 */
function grantPlaceLookups(scope: Construct, user: iam.IUser): void {
  new iam.Policy(scope, "PlaceLookupPolicy", {
    users: [user],
    statements: [new iam.PolicyStatement({
      actions: ["geo-places:Geocode", "geo-places:GetPlace", "geo-places:Autocomplete"],
      resources: ["*"],
      conditions: { StringEquals: { "aws:RequestedRegion": WORKLOAD_REGION } },
    })],
  });
}

/** Writes to the consent-evidence and container log groups and the host's one metric namespace. */
function grantLogsAndMetrics(scope: Construct, config: StageConfig, user: iam.IUser): void {
  const stack = cdk.Stack.of(scope);
  const logGroup = (name: string) => stack.formatArn({
    service: "logs",
    resource: "log-group",
    resourceName: name,
    arnFormat: cdk.ArnFormat.COLON_RESOURCE_NAME,
  });
  new iam.Policy(scope, "TelemetryPolicy", {
    users: [user],
    statements: [
      new iam.PolicyStatement({
        actions: ["logs:CreateLogStream", "logs:PutLogEvents"],
        resources: [
          ...consentEvidenceLogGroupNames(config.stage).map((name) => logGroup(`${name}:*`)),
          logGroup(`${config.host.containerLogGroupPrefix}*`),
        ],
      }),
      new iam.PolicyStatement({
        actions: ["cloudwatch:PutMetricData"],
        resources: ["*"],
        conditions: { StringEquals: { "cloudwatch:namespace": config.host.metricNamespace } },
      }),
    ],
  });
}
