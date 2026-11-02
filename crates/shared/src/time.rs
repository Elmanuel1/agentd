//! Wire timestamps.
//!
//! The live API emits RFC 3339 UTC with **6-digit microseconds** and a `Z`
//! suffix, e.g. `2026-07-03T22:32:35.588293Z`. We match that byte-for-byte on
//! serialization and accept any valid RFC 3339 on deserialization.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// UTC timestamp serialized in the exact live wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Current instant.
    pub fn now() -> Self {
        Self(Utc::now())
    }

    /// Wrap an existing chrono instant.
    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }

    /// The underlying chrono instant.
    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }

    /// Wire rendering: `%Y-%m-%dT%H:%M:%S%.6fZ`.
    pub fn to_wire(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Micros, true)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        let dt = DateTime::parse_from_rfc3339(&raw).map_err(serde::de::Error::custom)?;
        Ok(Self(dt.with_timezone(&Utc)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn wire_format_matches_live_fixture() {
        // From the live probe: "created_at": "2026-07-03T22:32:35.588293Z"
        let dt =
            Utc.with_ymd_and_hms(2026, 7, 3, 22, 32, 35).unwrap() + chrono::Duration::microseconds(588_293);
        let ts = Timestamp::from_datetime(dt);
        assert_eq!(ts.to_wire(), "2026-07-03T22:32:35.588293Z");
    }

    #[test]
    fn always_six_fraction_digits_even_at_zero() {
        let dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(Timestamp::from_datetime(dt).to_wire(), "2026-01-01T00:00:00.000000Z");
    }

    #[test]
    fn serde_roundtrip_preserves_instant() {
        let ts = Timestamp::now();
        let json = serde_json::to_string(&ts).unwrap();
        let back: Timestamp = serde_json::from_str(&json).unwrap();
        // Round-trips at microsecond precision.
        assert_eq!(ts.as_datetime().timestamp_micros(), back.as_datetime().timestamp_micros());
    }

    #[test]
    fn deserialize_accepts_offset_and_normalizes_to_utc() {
        let ts: Timestamp = serde_json::from_str("\"2026-07-03T18:32:35.588293-04:00\"").unwrap();
        assert_eq!(ts.to_wire(), "2026-07-03T22:32:35.588293Z");
    }
}
