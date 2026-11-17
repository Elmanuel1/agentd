# Vault credential storage: IAM

These policies are for the AWS secret stores, `AGENTD_SECRETS=aws_sm` (Secrets
Manager) and `AGENTD_SECRETS=aws_ssm` (Parameter Store). The default store,
`postgres`, keeps credentials sealed in agentd's own database and needs no AWS
permission.

With `aws_sm`, agentd keeps each vault credential in Secrets Manager as
`PREFIX/<workspace>/<vault>/<credential>`; with `aws_ssm`, as the SecureString
parameter `/PREFIX/<workspace>/<vault>/<credential>`. Either is encrypted under
one customer-managed KMS key and tagged `workspace`, `vault` and
`managed-by=agentd`. An `environment_variable` credential's secret is also
tagged `credential_type=environment_variable`. The store holds nothing else:
agentd's own keys come from its environment. Only agentd's role, and an
operator running `agentd vault-secrets` as it, can read them. Sandboxes and
runners cannot read them or use the key.

## Placeholders

| Placeholder | Value |
|---|---|
| `PREFIX` | `AGENTD_SECRETS_PREFIX` (default `agentd`) |
| `KMS_KEY_ARN` | the ARN of the key in `AGENTD_SECRETS_KMS_KEY_ID` |
| `AGENTD_ROLE_ARN` | the role agentd runs as (or assumes with `AGENTD_SECRETS_ROLE_ARN`) |
| `ACCOUNT_ID` | the account that owns the key and the secrets |

The ARNs match every region and account (`*:*`). Pinning them to your
region and account is tighter and loses nothing.

## The files

- `agentd-secrets-policy.json`, on agentd's role:
  - `CreateSecret` under `PREFIX/` only with the `managed-by=agentd` tag, only
    agentd's tag keys (`workspace`, `vault`, `managed-by`, `credential_type`),
    and only under `KMS_KEY_ARN`
    (`secretsmanager:KmsKeyArn`, as in AWS's
    [identity-based policy examples](https://docs.aws.amazon.com/secretsmanager/latest/userguide/auth-and-access_iam-policies.html);
    agentd always sends the key ARN).
  - `TagResource` on `PREFIX/` secrets, only with the `managed-by=agentd` tag
    and the same tag keys, for the tags a secret gets at create and for
    `agentd vault-secrets tag`.
  - `GetSecretValue`, `DescribeSecret` and `DeleteSecret` on `PREFIX/`
    secrets. These are scoped by name only: a tag condition fails for a secret
    that does not exist, so AWS would answer `AccessDenied` where agentd needs
    `ResourceNotFound` (a gone secret is already deleted).
  - `PutSecretValue` and `RestoreSecret`, which only touch existing secrets,
    on `PREFIX/` secrets tagged `managed-by=agentd`.
  - No delete without a recovery window.
  - `kms:DescribeKey` on the key, for the check at boot.
  - `kms:Encrypt`, `kms:Decrypt` and `kms:GenerateDataKey` on the key only
    through Secrets Manager (`kms:ViaService`) and only for `PREFIX/` secrets
    (`kms:EncryptionContext:SecretARN`).
- `agentd-ssm-policy.json`, on agentd's role in place of the Secrets Manager
  policy when the store is `aws_ssm`:
  - `PutParameter`, `GetParameter` and `DeleteParameter` on `/PREFIX/`
    parameters, scoped by name only for the reason above. An overwrite
    carries no tags, so `PutParameter` takes no tag condition.
  - `AddTagsToResource` on `/PREFIX/` parameters, only with the
    `managed-by=agentd` tag and agentd's tag keys: the tags a parameter gets
    at create.
  - `kms:DescribeKey` on the key, for the check at boot.
  - `kms:Encrypt`, `kms:Decrypt` and `kms:GenerateDataKey` on the key only
    through Parameter Store (`kms:ViaService`) and only for `/PREFIX/`
    parameters (`kms:EncryptionContext:PARAMETER_ARN`).
- `kms-key-policy.json`, the key policy. The account administers the key but
  holds no use of it and cannot create grants on it, so no IAM policy or grant
  can give a principal its use; only a change to this key policy can. Only
  `AGENTD_ROLE_ARN` may use it, through Secrets Manager for `PREFIX/` secrets
  or through Parameter Store for `/PREFIX/` parameters; drop the statement of
  the store you do not run. Rotate its material with KMS automatic key rotation: the key stays
  the same, so existing secrets keep decrypting. A different key id leaves
  every existing secret under the old key, which these policies no longer
  allow.
- `sandbox-deny-policy.json`, on every sandbox and runner role, or as their
  permissions boundary: an explicit deny on `PREFIX/` secrets, on reading
  `/PREFIX/` parameters (`ssm:GetParameter*`), on listing secrets, and on the
  key. Grant those roles nothing else for secrets.

## What the host must also do

- Sandboxes must not share agentd's credentials. On EC2, require IMDSv2 with
  a hop limit of 1, so a container cannot fetch the instance role. On ECS or
  EKS, give agentd its own task role or service account, and run sandboxes
  outside agentd's task.
- Give agentd no AWS environment keys in a sandbox's environment.

## Boot checks

agentd calls `kms:DescribeKey` at boot and does not start unless the key is
reachable, customer managed, enabled and a symmetric encryption key. An AWS
managed key (`aws/secretsmanager`, `aws/ssm`) is refused.
