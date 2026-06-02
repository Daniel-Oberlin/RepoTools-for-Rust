//! Serde helpers that make the Rust JSON manifest interoperable with the .NET
//! (Newtonsoft.Json) format: .NET-style UTC dates and Base64-encoded hash bytes.

use chrono::{DateTime, TimeZone, Utc};

/// The .NET `DateTime.MinValue` rendered by the manifest writer
/// (`0001-01-01T00:00:00.000Z`). Used as the "unset" sentinel for dates such as
/// `LastValidateDateUtc` before a repository has ever been validated.
pub fn dotnet_min_date() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).unwrap()
}

/// (De)serializes `DateTime<Utc>` the way the .NET tool does:
/// `yyyy-MM-ddTHH:mm:ss.fffZ` (three fractional digits, `Z` suffix, UTC).
///
/// On read we accept any RFC 3339 / ISO-8601 timestamp, since only semantic
/// interoperability is required.
pub mod dotnet_date {
    use chrono::{DateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        // Millisecond precision with a trailing 'Z', matching IsoDateTimeConverter.
        s.serialize_str(&dt.to_rfc3339_opts(SecondsFormat::Millis, true))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let s = String::deserialize(d)?;
        DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// (De)serializes a byte vector as a Base64 string, matching how
/// Newtonsoft.Json serializes a `byte[]` (used for `FileHash.HashData`).
pub mod base64_bytes {
    use base64::prelude::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&BASE64_STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        BASE64_STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}
