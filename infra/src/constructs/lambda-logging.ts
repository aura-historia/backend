import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import type { Construct } from "constructs";
import type { StageConfig } from "../config";

/** Retention of every Lambda log group that does not declare its own. */
const DEFAULT_LAMBDA_LOG_RETENTION = logs.RetentionDays.ONE_WEEK;

/**
 * Lambda's own logging controls. The system log level WARN drops the START, END and REPORT
 * lines, which Lambda writes per invocation. The application level is deliberately not WARN: for
 * a custom runtime Lambda filters application logs by the `level` of a JSON line and assigns
 * INFO to every line without an RFC 3339 `timestamp`, which the shared `tracing` formatter omits
 * and which plain-text panic output never has, so a WARN filter would drop the errors it should
 * keep. The application filters itself through LOG_LEVEL; TRACE keeps Lambda from filtering a
 * second time. Application log levels need the JSON log format.
 */
export const LAMBDA_LOGGING = {
  loggingFormat: lambda.LoggingFormat.JSON,
  systemLogLevelV2: lambda.SystemLogLevel.WARN,
  applicationLogLevelV2: lambda.ApplicationLogLevel.TRACE,
} as const;

/** The level the application logs at: production only reports what needs attention. */
export function lambdaLogLevel(config: StageConfig): "warn" | "info" {
  return config.isProd ? "warn" : "info";
}

/** The log group of a function that has no reason to keep its logs longer than a week. */
export function lambdaLogGroup(scope: Construct, id: string, functionName: string, config: StageConfig): logs.LogGroup {
  return new logs.LogGroup(scope, id, {
    logGroupName: `/aws/lambda/${functionName}`,
    retention: DEFAULT_LAMBDA_LOG_RETENTION,
    removalPolicy: config.removalPolicy,
  });
}
