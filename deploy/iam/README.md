# Vault credential storage: IAM

agentd keeps each vault credential in AWS Secrets Manager as
`PREFIX/<workspace>/<vault>/<credential>`, encrypted under one
customer-managed KMS key and tagged `workspace`, `vault` and
`managed-by=agentd`. API keys are secrets too: each is
`PREFIX/system/api_keys/<digest>`, holding only a peppered digest, the key id,
org and workspace, and tagged with what `agentd keys list` shows. The pepper
is `PREFIX/system/api_key_pepper`. All of them are under the same KMS key, so
the same policy covers them. Only agentd's role, and an operator running
`agentd keys` as it, can read them. Sandboxes and runners cannot read them or
use the key.

## Placeholders

| Placeholder | Value |
|---|---|
| `PREFIX` | `AGENTD_SECRETS_PREFIX` (default `agentd`) |
| `KMS_KEY_ARN` | the ARN of the key in `AGENTD_SECRETS_KMS_KEY_ID` |
| `AGENTD_ROLE_ARN` | the role agentd runs as (or assumes with `AGENTD_SECRETS_ROLE_ARN`) |
| `EGRESS_ROLE_ARN` | the role the egress service, `agentd-egress`, runs as |
| `ACCOUNT_ID` | the account that owns the key and the secrets |

The ARNs match every region and account (`*:*`). Pinning them to your
region and account is tighter and loses nothing.

## The files

- `agentd-secrets-policy.json`, on agentd's role:
  - `CreateSecret` under `PREFIX/` only with the `managed-by=agentd` tag, only
    agentd's tag keys (`workspace`, `vault`, `managed-by`, and for API keys
    `org`, `api-key-id`, `key-name`, `key-hint`), and only under `KMS_KEY_ARN`
    (`secretsmanager:KmsKeyArn`, as in AWS's
    [identity-based policy examples](https://docs.aws.amazon.com/secretsmanager/latest/userguide/auth-and-access_iam-policies.html);
    agentd always sends the key ARN).
  - `GetSecretValue`, `DescribeSecret` and `DeleteSecret` on `PREFIX/`
    secrets. These are scoped by name only: a tag condition fails for a secret
    that does not exist, so AWS would answer `AccessDenied` where agentd needs
    `ResourceNotFound` (an unknown API key is a 401, a missing pepper is
    created, a gone secret is already deleted).
  - `PutSecretValue` and `RestoreSecret`, which only touch existing secrets,
    on `PREFIX/` secrets tagged `managed-by=agentd`.
  - `ListSecrets`, which takes no resource, for `agentd keys list` and
    `revoke`. It returns names and tags, never a value.
  - No delete without a recovery window.
  - `kms:DescribeKey` on the key, for the check at boot.
  - `kms:Encrypt`, `kms:Decrypt` and `kms:GenerateDataKey` on the key only
    through Secrets Manager (`kms:ViaService`) and only for `PREFIX/` secrets
    (`kms:EncryptionContext:SecretARN`).
- `egress-secrets-policy.json`, on the egress service's role: only
  `GetSecretValue`, on `PREFIX/` secrets, and an explicit deny on
  `PREFIX/system/`, where the API key records and the pepper live. The
  service reads a credential's current value when a request needs it and
  makes no other AWS call.
- `kms-key-policy.json`, the key policy. The account administers the key but
  holds no use of it and cannot create grants on it, so no IAM policy or grant
  can give a principal its use; only a change to this key policy can. Only
  `AGENTD_ROLE_ARN` may use it, through Secrets Manager, for `PREFIX/`
  secrets, and `EGRESS_ROLE_ARN` may only decrypt, the same way. Rotate its material with KMS automatic key rotation: the key stays
  the same, so existing secrets keep decrypting. A different key id leaves
  every existing secret under the old key, which these policies no longer
  allow.
- `sandbox-deny-policy.json`, on every sandbox and runner role, or as their
  permissions boundary: an explicit deny on `PREFIX/` secrets, on listing
  secrets, and on the key. Grant those roles nothing else for secrets.

## What the host must also do

- Sandboxes must not share agentd's credentials. On EC2, require IMDSv2 with
  a hop limit of 1, so a container cannot fetch the instance role. On ECS or
  EKS, give agentd its own task role or service account, and run sandboxes
  outside agentd's task.
- Give agentd no AWS environment keys in a sandbox's environment.

## Boot checks

agentd calls `kms:DescribeKey` at boot and does not start unless the key is
reachable, customer managed, enabled and a symmetric encryption key. The
`aws/secretsmanager` key is refused.
