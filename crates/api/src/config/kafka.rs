//! How agentd reaches Kafka: librdkafka's own TLS and SASL properties, each
//! from `AGENTD_KAFKA_<PROPERTY>`, or IAM.

use super::{ConfigError, selector};

/// Each setting and the librdkafka property it sets
/// (<https://github.com/confluentinc/librdkafka/blob/master/CONFIGURATION.md>).
const PROPERTIES: [(&str, &str); 5] = [
    ("AGENTD_KAFKA_SECURITY_PROTOCOL", "security.protocol"),
    ("AGENTD_KAFKA_SASL_MECHANISM", "sasl.mechanism"),
    ("AGENTD_KAFKA_SASL_USERNAME", "sasl.username"),
    ("AGENTD_KAFKA_SASL_PASSWORD", "sasl.password"),
    ("AGENTD_KAFKA_SSL_CA_LOCATION", "ssl.ca.location"),
];

/// The Kafka client settings.
#[derive(Clone, PartialEq, Eq)]
pub struct KafkaConfig {
    /// The librdkafka properties that are set; unset ones keep librdkafka's
    /// defaults (`security.protocol=plaintext`).
    pub properties: Vec<(String, String)>,
    /// `AGENTD_KAFKA_AUTH`.
    pub auth: KafkaAuth,
}

/// `AGENTD_KAFKA_AUTH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KafkaAuth {
    /// `password` (the default): the SASL settings, if any.
    Password,
    /// `aws_iam`: SASL/OAUTHBEARER over TLS with Amazon MSK IAM tokens, signed
    /// with the AWS credentials of the default chain.
    AwsIam,
}

/// Shows every property but `sasl.password`'s value.
impl std::fmt::Debug for KafkaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<(&str, &str)> = self
            .properties
            .iter()
            .map(|(k, v)| (k.as_str(), if k == "sasl.password" { "***" } else { v.as_str() }))
            .collect();
        f.debug_struct("KafkaConfig").field("properties", &shown).field("auth", &self.auth).finish()
    }
}

impl KafkaConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let properties: Vec<(String, String)> = PROPERTIES
            .iter()
            .filter_map(|(var, property)| get(var).filter(|v| !v.is_empty()).map(|v| ((*property).into(), v)))
            .collect();
        let auth = match selector(get, "AGENTD_KAFKA_AUTH", "password").as_str() {
            "password" => KafkaAuth::Password,
            "aws_iam" => KafkaAuth::AwsIam,
            other => return Err(refused(format!("unknown method `{other}`: use password or aws_iam"))),
        };
        let sasl_set = properties.iter().any(|(k, _)| k.starts_with("sasl."));
        let plain =
            properties.iter().any(|(k, v)| k == "security.protocol" && !v.eq_ignore_ascii_case("SASL_SSL"));
        if auth == KafkaAuth::AwsIam && (sasl_set || plain) {
            return Err(refused(
                "aws_iam is SASL_SSL with OAUTHBEARER: unset the AGENTD_KAFKA_SASL_* settings and any other \
                 AGENTD_KAFKA_SECURITY_PROTOCOL"
                    .into(),
            ));
        }
        Ok(Self { properties, auth })
    }
}

fn refused(reason: String) -> ConfigError {
    ConfigError { var: "AGENTD_KAFKA_AUTH", reason }
}
