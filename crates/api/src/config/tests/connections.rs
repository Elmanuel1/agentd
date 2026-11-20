//! How each backing service logs in.

use super::*;

#[test]
fn postgres_logs_in_with_the_url_password_unless_iam_is_set() {
    let StoreConfig::Postgres { auth, .. } = config(&[]).unwrap().store;
    assert_eq!(auth, PgAuth::Password);
    let StoreConfig::Postgres { auth, .. } = config(&[("AGENTD_PG_AUTH", "aws_iam")]).unwrap().store;
    assert_eq!(auth, PgAuth::AwsIam);
    let err = config(&[("AGENTD_PG_AUTH", "gcp_iam")]).unwrap_err();
    assert_eq!(
        (err.var, err.reason.as_str()),
        ("AGENTD_PG_AUTH", "unknown method `gcp_iam`: use password or aws_iam")
    );
}

#[test]
fn kafka_takes_librdkafka_properties_by_their_own_names() {
    let c = config(&[
        ("AGENTD_KAFKA_SECURITY_PROTOCOL", "SASL_SSL"),
        ("AGENTD_KAFKA_SASL_MECHANISM", "SCRAM-SHA-512"),
        ("AGENTD_KAFKA_SASL_USERNAME", "agentd"),
        ("AGENTD_KAFKA_SASL_PASSWORD", "s3cr3t"),
        ("AGENTD_KAFKA_SSL_CA_LOCATION", "/etc/kafka/ca.pem"),
    ])
    .unwrap();
    let want: Vec<(String, String)> = [
        ("security.protocol", "SASL_SSL"),
        ("sasl.mechanism", "SCRAM-SHA-512"),
        ("sasl.username", "agentd"),
        ("sasl.password", "s3cr3t"),
        ("ssl.ca.location", "/etc/kafka/ca.pem"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .into();
    assert_eq!(c.streams.kafka, KafkaConfig { properties: want, auth: KafkaAuth::Password });
    let shown = format!("{:?}", c.streams.kafka);
    assert!(shown.contains(r#"("sasl.password", "***")"#) && !shown.contains("s3cr3t"), "{shown}");
}

#[test]
fn kafka_is_plaintext_without_settings() {
    let c = config(&[]).unwrap();
    assert_eq!(c.streams.kafka, KafkaConfig { properties: Vec::new(), auth: KafkaAuth::Password });
}

#[test]
fn kafka_iam_is_sasl_ssl_and_takes_no_sasl_settings() {
    let c = config(&[("AGENTD_KAFKA_AUTH", "aws_iam"), ("AGENTD_KAFKA_SSL_CA_LOCATION", "/ca.pem")]).unwrap();
    assert_eq!(c.streams.kafka.auth, KafkaAuth::AwsIam);
    for clash in [("AGENTD_KAFKA_SASL_USERNAME", "u"), ("AGENTD_KAFKA_SECURITY_PROTOCOL", "PLAINTEXT")] {
        let err = config(&[("AGENTD_KAFKA_AUTH", "aws_iam"), clash]).unwrap_err();
        assert_eq!(err.var, "AGENTD_KAFKA_AUTH", "{clash:?}");
    }
    assert_eq!(config(&[("AGENTD_KAFKA_AUTH", "kerberos")]).unwrap_err().var, "AGENTD_KAFKA_AUTH");
}

#[test]
fn a_valkey_ca_needs_a_rediss_url() {
    let c = config(&[("AGENTD_VALKEY_URL", "rediss://valkey:6379"), ("AGENTD_VALKEY_CA_CERT", "/ca.pem")])
        .unwrap();
    assert_eq!(c.streams.valkey_ca.as_deref(), Some("/ca.pem"));
    let err = config(&[("AGENTD_VALKEY_CA_CERT", "/ca.pem")]).unwrap_err();
    assert_eq!(
        (err.var, err.reason.as_str()),
        ("AGENTD_VALKEY_CA_CERT", "needs a rediss:// AGENTD_VALKEY_URL")
    );
}

#[test]
fn an_otlp_certificate_needs_an_https_endpoint() {
    let c = config(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://collector:4317"),
        ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/etc/otel/ca.pem"),
    ])
    .unwrap();
    assert_eq!(c.telemetry.otlp_certificate.as_deref(), Some("/etc/otel/ca.pem"));
    let plain = [
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
        ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/ca.pem"),
    ];
    assert_eq!(config(&plain).unwrap_err().var, "OTEL_EXPORTER_OTLP_CERTIFICATE");
}

#[test]
fn a_keyword_connection_string_shows_no_password() {
    let url = "host=db user=agentd password='s3cr3t pass' dbname=agentd sslmode=require";
    let shown = format!("{:?}", config(&[("DATABASE_URL", url), ("AGENTD_PG_READ_URL", url)]).unwrap());
    assert!(
        !shown.contains("s3cr3t") && shown.contains("host=db user=agentd password=*** dbname=agentd"),
        "{shown}"
    );
}
