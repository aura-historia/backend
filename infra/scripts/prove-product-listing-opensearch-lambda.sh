#!/usr/bin/env bash
set -euo pipefail

require() {
  local name="$1"
  if [ -z "${!name:-}" ]; then
    printf 'Missing required %s\n' "$name" >&2
    exit 2
  fi
}

if [ "${AURA_HISTORIA_ISOLATED_AWS:-}" != "true" ]; then
  printf 'Set AURA_HISTORIA_ISOLATED_AWS=true only for an isolated account.\n' >&2
  exit 2
fi

for name in \
  AURA_HISTORIA_ISOLATED_ACCOUNT_ID \
  AURA_HISTORIA_PROOF_STAGE \
  AURA_HISTORIA_PROJECTION_QUEUE_URL \
  AURA_HISTORIA_PROJECTION_DLQ_URL \
  AURA_HISTORIA_VALID_JOB_FILE \
  AURA_HISTORIA_POISON_JOB_FILE \
  AURA_HISTORIA_EXPECTED_DOCUMENT_URL \
  AURA_HISTORIA_OPENSEARCH_CURL_CONFIG; do
  require "$name"
done

if [ "${AURA_HISTORIA_PROOF_STAGE}" = "prod" ]; then
  printf 'This proof helper never runs against prod.\n' >&2
  exit 2
fi

actual_account="$(aws sts get-caller-identity --query Account --output text)"
if [ "$actual_account" != "$AURA_HISTORIA_ISOLATED_ACCOUNT_ID" ]; then
  printf 'AWS caller account does not match the isolated account gate.\n' >&2
  exit 2
fi

expected_queue_suffix="aura-worker-product-listing-opensearch-${AURA_HISTORIA_PROOF_STAGE}"
expected_dlq_suffix="aura-worker-product-listing-opensearch-dlq-${AURA_HISTORIA_PROOF_STAGE}"
case "$AURA_HISTORIA_PROJECTION_QUEUE_URL" in
  *"/${expected_queue_suffix}") ;;
  *)
    printf 'Source queue is not the ProductListing OpenSearch queue for the selected stage.\n' >&2
    exit 2
    ;;
esac
case "$AURA_HISTORIA_PROJECTION_DLQ_URL" in
  *"/${expected_dlq_suffix}") ;;
  *)
    printf 'DLQ is not the paired ProductListing OpenSearch DLQ for the selected stage.\n' >&2
    exit 2
    ;;
esac

source_arn="$(aws sqs get-queue-attributes --queue-url "$AURA_HISTORIA_PROJECTION_QUEUE_URL" --attribute-names QueueArn RedrivePolicy --query 'Attributes.QueueArn' --output text)"
dlq_arn="$(aws sqs get-queue-attributes --queue-url "$AURA_HISTORIA_PROJECTION_DLQ_URL" --attribute-names QueueArn --query 'Attributes.QueueArn' --output text)"
source_redrive_policy="$(aws sqs get-queue-attributes --queue-url "$AURA_HISTORIA_PROJECTION_QUEUE_URL" --attribute-names RedrivePolicy --query 'Attributes.RedrivePolicy' --output text)"
case "$source_redrive_policy" in
  *"${dlq_arn}"*) ;;
  *)
    printf 'Source queue does not redrive to the supplied paired DLQ.\n' >&2
    exit 2
    ;;
esac

for queue_url in "$AURA_HISTORIA_PROJECTION_QUEUE_URL" "$AURA_HISTORIA_PROJECTION_DLQ_URL"; do
  queue_messages="$(aws sqs get-queue-attributes --queue-url "$queue_url" --attribute-names ApproximateNumberOfMessages ApproximateNumberOfMessagesNotVisible --query 'Attributes.[ApproximateNumberOfMessages,ApproximateNumberOfMessagesNotVisible]' --output text)"
  if [ "$queue_messages" != $'0\t0' ]; then
    printf 'Proof queues must be empty before running; do not purge shared or retained queues.\n' >&2
    exit 2
  fi
done

poison_timeout_seconds="${AURA_HISTORIA_PROOF_POISON_TIMEOUT_SECONDS:-1800}"
case "$poison_timeout_seconds" in
  ''|*[!0-9]*)
    printf 'AURA_HISTORIA_PROOF_POISON_TIMEOUT_SECONDS must be a positive number of seconds.\n' >&2
    exit 2
    ;;
esac
if [ "$poison_timeout_seconds" -le 0 ]; then
  printf 'AURA_HISTORIA_PROOF_POISON_TIMEOUT_SECONDS must be a positive number of seconds.\n' >&2
  exit 2
fi

if [ ! -r "$AURA_HISTORIA_VALID_JOB_FILE" ] || [ ! -r "$AURA_HISTORIA_POISON_JOB_FILE" ]; then
  printf 'Fixture jobs must be readable files.\n' >&2
  exit 2
fi
if [ ! -r "$AURA_HISTORIA_OPENSEARCH_CURL_CONFIG" ]; then
  printf 'OpenSearch curl config must be readable and must keep credentials out of this command.\n' >&2
  exit 2
fi

aws sqs send-message \
  --queue-url "$AURA_HISTORIA_PROJECTION_QUEUE_URL" \
  --message-body "file://$AURA_HISTORIA_VALID_JOB_FILE" \
  --output text \
  --query MessageId

for attempt in $(seq 1 60); do
  document="$(curl --fail --silent --show-error --config "$AURA_HISTORIA_OPENSEARCH_CURL_CONFIG" "$AURA_HISTORIA_EXPECTED_DOCUMENT_URL" || true)"
  if [ -n "$document" ] && [ -n "${AURA_HISTORIA_EXPECTED_DOCUMENT_FRAGMENT:-}" ] \
    && printf '%s' "$document" | grep --fixed-strings --quiet "$AURA_HISTORIA_EXPECTED_DOCUMENT_FRAGMENT"; then
    break
  fi
  if [ -n "$document" ] && [ -z "${AURA_HISTORIA_EXPECTED_DOCUMENT_FRAGMENT:-}" ]; then
    break
  fi
  sleep 2
done

if [ -z "${document:-}" ]; then
  printf 'Expected OpenSearch projection document did not appear.\n' >&2
  exit 1
fi
if [ -n "${AURA_HISTORIA_EXPECTED_DOCUMENT_FRAGMENT:-}" ] \
  && ! printf '%s' "$document" | grep --fixed-strings --quiet "$AURA_HISTORIA_EXPECTED_DOCUMENT_FRAGMENT"; then
  printf 'OpenSearch document did not contain the expected safe fixture fragment.\n' >&2
  exit 1
fi

aws sqs send-message \
  --queue-url "$AURA_HISTORIA_PROJECTION_QUEUE_URL" \
  --message-body "file://$AURA_HISTORIA_POISON_JOB_FILE" \
  --output text \
  --query MessageId

poison_deadline=$((SECONDS + poison_timeout_seconds))
while [ "$SECONDS" -lt "$poison_deadline" ]; do
  dlq_visible="$(aws sqs get-queue-attributes --queue-url "$AURA_HISTORIA_PROJECTION_DLQ_URL" --attribute-names ApproximateNumberOfMessages --query 'Attributes.ApproximateNumberOfMessages' --output text)"
  if [ "$dlq_visible" -gt 0 ] 2>/dev/null; then
    break
  fi
  sleep 5
done

if [ "${dlq_visible:-0}" -le 0 ] 2>/dev/null; then
  printf 'Poison job did not reach the paired DLQ within the configured proof timeout.\n' >&2
  exit 1
fi

if [ "${1:-}" = "--redrive" ]; then
  if [ "${AURA_HISTORIA_APPROVED_REDRIVE:-}" != "true" ]; then
    printf 'Set AURA_HISTORIA_APPROVED_REDRIVE=true after repairing the cause and approving redrive.\n' >&2
    exit 2
  fi
  aws sqs start-message-move-task --source-arn "$dlq_arn" --destination-arn "$source_arn"
else
  printf 'Valid job was projected and poison work reached its paired DLQ. Rerun with separately approved --redrive only after correction.\n'
fi
