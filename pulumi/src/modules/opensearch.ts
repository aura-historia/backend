/**
 * Module for creating OpenSearch domains and related alarms
 */

import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface OpenSearchResources {
  domain: aws.opensearch.Domain;
  clusterStatusRedAlarm: aws.cloudwatch.MetricAlarm;
  clusterStatusYellowAlarm: aws.cloudwatch.MetricAlarm;
  freeStorageSpaceAlarm: aws.cloudwatch.MetricAlarm;
  cpuUtilizationAlarm: aws.cloudwatch.MetricAlarm;
  jvmMemoryPressureAlarm: aws.cloudwatch.MetricAlarm;
}

export function createOpenSearchDomain(
  stageName: string,
  snsTopicArn: pulumi.Output<string>,
  accountId: string,
  region: string
): OpenSearchResources {
  const domain = new aws.opensearch.Domain('OpenSearchDomain', {
    domainName: `application-${stageName}`,
    engineVersion: 'OpenSearch_3.1',
    clusterConfig: {
      instanceType: 't3.small.search',
      instanceCount: 1,
      zoneAwarenessEnabled: false,
    },
    ebsOptions: {
      ebsEnabled: true,
      volumeSize: 10,
      volumeType: 'gp3',
    },
    accessPolicies: pulumi.interpolate`{
      "Version": "2012-10-17",
      "Statement": [{
        "Effect": "Allow",
        "Principal": {
          "AWS": "arn:aws:iam::${accountId}:root"
        },
        "Action": "es:*",
        "Resource": "arn:aws:es:${region}:${accountId}:domain/application-${stageName}/*"
      }]
    }`,
  });

  const clusterStatusRedAlarm = new aws.cloudwatch.MetricAlarm('OpenSearchClusterStatusRedAlarm', {
    alarmName: `${stageName}-opensearch-cluster-red`,
    alarmDescription: 'Alarm when OpenSearch cluster status is RED',
    metricName: 'ClusterStatus.red',
    namespace: 'AWS/ES',
    statistic: 'Maximum',
    period: 60,
    evaluationPeriods: 1,
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      DomainName: domain.domainName,
      ClientId: accountId,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  const clusterStatusYellowAlarm = new aws.cloudwatch.MetricAlarm('OpenSearchClusterStatusYellowAlarm', {
    alarmName: `${stageName}-opensearch-cluster-yellow`,
    alarmDescription: 'Alarm when OpenSearch cluster status is YELLOW',
    metricName: 'ClusterStatus.yellow',
    namespace: 'AWS/ES',
    statistic: 'Maximum',
    period: 300,
    evaluationPeriods: 2,
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      DomainName: domain.domainName,
      ClientId: accountId,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  const freeStorageSpaceAlarm = new aws.cloudwatch.MetricAlarm('OpenSearchFreeStorageSpaceAlarm', {
    alarmName: `${stageName}-opensearch-low-storage`,
    alarmDescription: 'Alarm when OpenSearch has low free storage space',
    metricName: 'FreeStorageSpace',
    namespace: 'AWS/ES',
    statistic: 'Minimum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 2000,
    comparisonOperator: 'LessThanOrEqualToThreshold',
    dimensions: {
      DomainName: domain.domainName,
      ClientId: accountId,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  const cpuUtilizationAlarm = new aws.cloudwatch.MetricAlarm('OpenSearchCPUUtilizationAlarm', {
    alarmName: `${stageName}-opensearch-high-cpu`,
    alarmDescription: 'Alarm when OpenSearch CPU utilization is high',
    metricName: 'CPUUtilization',
    namespace: 'AWS/ES',
    statistic: 'Average',
    period: 300,
    evaluationPeriods: 2,
    threshold: 80,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      DomainName: domain.domainName,
      ClientId: accountId,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  const jvmMemoryPressureAlarm = new aws.cloudwatch.MetricAlarm('OpenSearchJVMMemoryPressureAlarm', {
    alarmName: `${stageName}-opensearch-high-jvm-memory`,
    alarmDescription: 'Alarm when OpenSearch JVM memory pressure is high',
    metricName: 'JVMMemoryPressure',
    namespace: 'AWS/ES',
    statistic: 'Maximum',
    period: 300,
    evaluationPeriods: 1,
    threshold: 85,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    dimensions: {
      DomainName: domain.domainName,
      ClientId: accountId,
    },
    treatMissingData: 'notBreaching',
    alarmActions: [snsTopicArn],
  });

  return {
    domain,
    clusterStatusRedAlarm,
    clusterStatusYellowAlarm,
    freeStorageSpaceAlarm,
    cpuUtilizationAlarm,
    jvmMemoryPressureAlarm,
  };
}
