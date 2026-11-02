//! AWS failures as [`SecretError`]s.
//!
//! An error names the IAM action, its target and the AWS error code, never
//! the AWS message: a validation message can quote the request, and the
//! request can hold the secret.

use aws_sdk_secretsmanager::error::{ProvideErrorMetadata, SdkError};
use shared::ports::secrets::SecretError;

/// `action` (`secretsmanager:GetSecretValue`) on `target` failed with `e`.
pub(crate) fn failure<E: ProvideErrorMetadata, R>(
    action: &str,
    target: &str,
    e: &SdkError<E, R>,
) -> SecretError {
    SecretError::Backend(format!("{action} on {target} failed: {}", code(e)))
}

fn code<E: ProvideErrorMetadata, R>(e: &SdkError<E, R>) -> &str {
    match e {
        SdkError::ServiceError(s) => s.err().code().unwrap_or("an error with no code"),
        SdkError::TimeoutError(_) => "timeout",
        SdkError::DispatchFailure(_) => "no response (dispatch failure)",
        SdkError::ResponseError(_) => "unreadable response",
        SdkError::ConstructionFailure(_) => "request not built",
        _ => "SDK error",
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError;
    use aws_sdk_secretsmanager::types::error::DecryptionFailure;
    use aws_smithy_runtime_api::http::{Response, StatusCode};
    use aws_smithy_types::body::SdkBody;
    use aws_smithy_types::error::ErrorMetadata;

    use super::*;

    const SECRET: &str = "xoxb-known-secret-value";

    fn service(code: &str) -> SdkError<GetSecretValueError, Response> {
        let meta = ErrorMetadata::builder().code(code).message(format!("echoes {SECRET}")).build();
        let err = GetSecretValueError::generic(meta);
        let raw = Response::new(StatusCode::try_from(400).unwrap(), SdkBody::from(SECRET));
        SdkError::service_error(err, raw)
    }

    #[test]
    fn a_service_error_gives_its_code_and_target_but_not_its_message() {
        let err = failure("secretsmanager:GetSecretValue", "agentd/w/v/c", &service("AccessDeniedException"));
        assert_eq!(
            err.to_string(),
            "secret backend: secretsmanager:GetSecretValue on agentd/w/v/c failed: AccessDeniedException"
        );
        assert!(!format!("{err:?}").contains(SECRET));
    }

    #[test]
    fn a_modeled_error_gives_its_code() {
        let modeled = GetSecretValueError::DecryptionFailure(
            DecryptionFailure::builder()
                .message(SECRET)
                .meta(ErrorMetadata::builder().code("DecryptionFailure").message(SECRET).build())
                .build(),
        );
        let raw = Response::new(StatusCode::try_from(400).unwrap(), SdkBody::from(SECRET));
        let err = failure("secretsmanager:GetSecretValue", "n", &SdkError::service_error(modeled, raw));
        assert!(err.to_string().ends_with("failed: DecryptionFailure"), "{err}");
        assert!(!format!("{err} {err:?}").contains(SECRET));
    }

    #[test]
    fn transport_failures_have_fixed_labels() {
        let timeout: SdkError<GetSecretValueError, Response> = SdkError::timeout_error(SECRET);
        let built: SdkError<GetSecretValueError, Response> = SdkError::construction_failure(SECRET);
        for (e, label) in [(timeout, "timeout"), (built, "request not built")] {
            let err = failure("secretsmanager:PutSecretValue", "n", &e).to_string();
            assert!(err.ends_with(label), "{err}");
            assert!(!err.contains(SECRET));
        }
    }
}
