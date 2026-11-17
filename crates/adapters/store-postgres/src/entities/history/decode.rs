//! The feed's encodings of a row's columns (Debezium's JSON: `jsonb` as JSON
//! text, `bytea` as base64), turned back into the JSON Postgres's
//! `jsonb_populate_record` reads.

use base64::Engine as _;
use serde_json::{Map, Value};

/// How the feed encodes a column's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    /// `jsonb`, sent as JSON text.
    JsonText,
    /// `bytea`, sent as base64.
    Base64,
    /// Every other type, sent as Postgres reads it back.
    Plain,
}

impl Encoding {
    /// The encoding of a column of type `data_type` (`format_type`'s name).
    pub(super) fn of(data_type: &str) -> Self {
        match data_type {
            "jsonb" => Self::JsonText,
            "bytea" => Self::Base64,
            _ => Self::Plain,
        }
    }
}

/// `row` with each column the feed encodes turned back into what Postgres reads.
pub(super) fn decoded(
    mut row: Map<String, Value>,
    columns: &[(String, Encoding)],
) -> Result<Map<String, Value>, String> {
    for (name, encoding) in columns {
        let Some(Value::String(text)) = row.get(name) else { continue };
        let value = match encoding {
            Encoding::JsonText => serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?,
            Encoding::Base64 => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|e| format!("{name}: {e}"))?;
                Value::String(format!("\\x{}", hex::encode(bytes)))
            }
            Encoding::Plain => continue,
        };
        row.insert(name.clone(), value);
    }
    Ok(row)
}

/// The text form of a log sequence number, as `pg_lsn` reads it.
pub(super) fn lsn_text(lsn: u64) -> String {
    format!("{:X}/{:X}", lsn >> 32, lsn & 0xFFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_feed_s_json_text_and_base64_decode_by_column_type() {
        let columns = [("doc".to_owned(), Encoding::JsonText), ("key_digest".to_owned(), Encoding::Base64)];
        let row = json!({"doc": "{\"a\":1}", "key_digest": "AAE=", "name": "n"});
        let row = decoded(row.as_object().unwrap().clone(), &columns).unwrap();
        assert_eq!(Value::Object(row), json!({"doc": {"a": 1}, "key_digest": "\\x0001", "name": "n"}));
        let bad = json!({"doc": "{"}).as_object().unwrap().clone();
        assert!(decoded(bad, &columns).unwrap_err().starts_with("doc:"));
    }

    #[test]
    fn an_lsn_reads_as_pg_lsn_text() {
        assert_eq!(lsn_text((1 << 32) | 0x16B_3748), "1/16B3748");
        assert_eq!(lsn_text(28_418_376), "0/1B1A148");
    }
}
