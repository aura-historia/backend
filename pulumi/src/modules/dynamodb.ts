/**
 * Module for creating DynamoDB tables and related alarms
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface DynamoDBResources {
  table: aws.dynamodb.Table;
  throttledRequestsAlarm: aws.cloudwatch.MetricAlarm;
}

export function createTable(stageName: string, snsTopicArn: pulumi.Output<string>): DynamoDBResources {
  const table = new aws.dynamodb.Table('TableOne', {
    name: `table_1-${stageName}`,
    billingMode: 'PAY_PER_REQUEST',
    hashKey: 'pk',
    rangeKey: 'sk',
    attributes: [
      { name: 'pk', type: 'S' },
      { name: 'sk', type: 'S' },
      { name: 'lsi1_sk', type: 'S' },
      { name: 'gsi1_pk', type: 'S' },
      { name: 'gsi1_sk', type: 'S' },
    ],
    localSecondaryIndexes: [
      {
        name: 'lsi1',
        rangeKey: 'lsi1_sk',
        projectionType: 'ALL',
      },
    ],
    globalSecondaryIndexes: [
      {
        name: 'gsi1',
        hashKey: 'gsi1_pk',
        rangeKey: 'gsi1_sk',
        projectionType: 'ALL',
      },
    ],
    tableClass: 'STANDARD',
    streamEnabled: true,
    streamViewType: 'NEW_IMAGE',
  });

  const throttledRequestsAlarm = new aws.cloudwatch.MetricAlarm(`${stageName}-dynamodb-throttled-requests`, {
    name: `${stageName}-dynamodb-throttled-requests`,
    alarmDescription: 'Alarm when DynamoDB table has throttled requests',
    metricName: 'UserErrors',
    namespace: 'AWS/DynamoDB',
    statistic: 'Sum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 5,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      TableName: table.name,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  return { table, throttledRequestsAlarm };
}
