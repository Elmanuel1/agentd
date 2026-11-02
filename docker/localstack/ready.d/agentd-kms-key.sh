#!/bin/sh
# The customer-managed KMS key agentd encrypts vault credentials with, made
# once when the emulator is ready and found by its alias after that.
set -eu
if ! awslocal kms describe-key --key-id alias/agentd-secrets >/dev/null 2>&1; then
  key=$(awslocal kms create-key --description "agentd secrets" --query KeyMetadata.KeyId --output text)
  awslocal kms create-alias --alias-name alias/agentd-secrets --target-key-id "$key"
fi
