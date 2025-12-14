/**
 * Module for creating SNS topics for alarms
 */

import * as aws from '@pulumi/aws';

export function createAlarmTopic(stageName: string): aws.sns.Topic {
  return new aws.sns.Topic('AlarmNotificationTopic', {
    name: `cloudwatch-alarms-${stageName}`,
    displayName: `CloudWatch Alarms for Aura-Historia Backend stage '${stageName}'`,
  });
}
