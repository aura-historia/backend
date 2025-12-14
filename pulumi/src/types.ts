/**
 * Common types and interfaces used across the infrastructure
 */

export interface StackConfig {
  stage: 'dev' | 'staging' | 'prod';
  stageName: string;
  artifactBucket: string;
  resourceBucket: string;
  mailTemplateBucket: string;
  commitSHA: string;
  ec2KeyPairName: string;
}

export interface StageMapping<T> {
  dev: T;
  staging: T;
  prod: T;
}

export interface AlarmConfig {
  snsTopicArn: string;
  treatMissingData?: string;
}
