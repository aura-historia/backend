import type { StageName } from "./config";

export const WORKER_SCOPES = [
  "product-listing-opensearch",
  "search-filter-projection",
  "search-filter-percolator",
  "search-filter-match-notification",
  "watchlist-notification",
  "product-content-assessment",
  "product-embedding",
  "product-translation",
  "product-listing-normalization",
  "notification-delivery",
] as const;

export type WorkerScope = (typeof WORKER_SCOPES)[number];


interface WorkerQueueDefinition {
  readonly id: string;
  readonly visibilityTimeoutSeconds: number;
}

// Only ProductListing OpenSearch is a Lambda SQS target. Other values match the polling Rust worker's budgets.
export const WORKER_QUEUE_DEFINITIONS = {
  "product-listing-opensearch": { id: "ProductListingOpensearch", visibilityTimeoutSeconds: 300 },
  // Lambda timeout is 45s; six bounded invocation attempts require 300s visibility.
  "search-filter-projection": { id: "SearchFilterProjection", visibilityTimeoutSeconds: 300 },
  "search-filter-percolator": { id: "SearchFilterPercolator", visibilityTimeoutSeconds: 300 },
  "search-filter-match-notification": { id: "SearchFilterMatchNotification", visibilityTimeoutSeconds: 60 },
  "watchlist-notification": { id: "WatchlistNotification", visibilityTimeoutSeconds: 60 },
  "product-content-assessment": { id: "ProductContentAssessment", visibilityTimeoutSeconds: 60 },
  "product-embedding": { id: "ProductEmbedding", visibilityTimeoutSeconds: 300 },
  "product-translation": { id: "ProductTranslation", visibilityTimeoutSeconds: 300 },
  // Lambda timeout is 45s; six bounded invocation attempts require 270s visibility.
  "product-listing-normalization": { id: "ProductListingNormalization", visibilityTimeoutSeconds: 270 },
  // The 45s Lambda leaves a five-minute delivery lease plus 30s recovery margin before retry.
  "notification-delivery": { id: "NotificationDelivery", visibilityTimeoutSeconds: 330 },
} as const satisfies Record<WorkerScope, WorkerQueueDefinition>;

export interface WorkerQueueSettings {
  readonly enabledScopes: readonly WorkerScope[];
  readonly sourceRetentionDays: number;
  readonly deadLetterRetentionDays: number;
  readonly maxReceiveCount: number;
  readonly receiveWaitTimeSeconds: number;
  readonly alarms: {
    readonly periodSeconds: number;
    readonly evaluationPeriods: number;
    readonly sourceAgeThresholdSeconds: number;
    readonly deadLetterVisibleThreshold: number;
  };
}

export const WORKER_QUEUE_SETTINGS = {
  enabledScopes: WORKER_SCOPES,
  sourceRetentionDays: 7,
  deadLetterRetentionDays: 14,
  maxReceiveCount: 5,
  receiveWaitTimeSeconds: 20,
  alarms: {
    periodSeconds: 300,
    evaluationPeriods: 1,
    sourceAgeThresholdSeconds: 900,
    deadLetterVisibleThreshold: 1,
  },
} as const satisfies WorkerQueueSettings;

export function workerQueueName(scope: WorkerScope, stage: StageName, deadLetter = false): string {
  const name = `aura-worker-${scope}${deadLetter ? "-dlq" : ""}-${stage}`;
  if (name.length > 80 || !/^[a-z0-9-]+$/.test(name)) {
    throw new Error(`Invalid Standard worker queue name: '${name}'. Expected at most 80 lowercase letters, digits, or hyphens.`);
  }
  return name;
}
