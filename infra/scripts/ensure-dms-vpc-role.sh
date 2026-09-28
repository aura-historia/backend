#!/usr/bin/env bash
# DMS requires this account-wide role before CloudFormation can create a replication subnet group.
set -euo pipefail

role_name=dms-vpc-role
policy_arn=arn:aws:iam::aws:policy/service-role/AmazonDMSVPCManagementRole
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

if ! aws iam get-role --role-name "$role_name" --output json > "$work_dir/role.json" 2> "$work_dir/error"; then
  if ! grep -q '(NoSuchEntity)' "$work_dir/error"; then
    cat "$work_dir/error" >&2
    exit 1
  fi

  # This role is shared by all stages in the account, not owned by any stage stack.
  if ! aws iam create-role --role-name "$role_name" \
    --assume-role-policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dms.amazonaws.com"},"Action":"sts:AssumeRole"}]}' \
    --output json > /dev/null 2> "$work_dir/error"; then
    # Another stage/operator may have created it concurrently; validate that role too.
    if ! grep -q '(EntityAlreadyExists)' "$work_dir/error"; then
      cat "$work_dir/error" >&2
      exit 1
    fi
  fi
  aws iam wait role-exists --role-name "$role_name"
  aws iam get-role --role-name "$role_name" --output json > "$work_dir/role.json"
fi

# Never overwrite the trust policy of an existing account-wide role.
if ! jq -e '
  .Role.AssumeRolePolicyDocument.Statement |
  (if type == "array" then . else [.] end) |
  length == 1 and .[0].Effect == "Allow" and
  .[0].Principal.Service == "dms.amazonaws.com" and
  .[0].Action == "sts:AssumeRole" and
  (.[0].Condition == null)
' "$work_dir/role.json" > /dev/null; then
  echo "${role_name} has an unexpected trust policy; an operator must restore DMS-only sts:AssumeRole trust before Deploy." >&2
  exit 1
fi

aws iam list-attached-role-policies --role-name "$role_name" --output json > "$work_dir/policies.json"
if ! jq -e --arg arn "$policy_arn" '.AttachedPolicies | any(.PolicyArn == $arn)' "$work_dir/policies.json" > /dev/null; then
  aws iam attach-role-policy --role-name "$role_name" --policy-arn "$policy_arn"
  # IAM is eventually consistent with DMS; give the newly attached policy time to propagate.
  sleep 15
fi

echo "DMS VPC role is configured for replication subnet groups."
