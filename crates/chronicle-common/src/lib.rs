//! Shared transport-neutral domain types.

use serde::{Deserialize, Serialize};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

pub type Timestamp = OffsetDateTime;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolId(String);

impl ProtocolId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn unknown() -> Self {
        Self::new("unknown")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProtocolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TransportProtocol {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectionKey {
    pub network_namespace: Option<u64>,
    pub client: Endpoint,
    pub server: Endpoint,
    pub transport: TransportProtocol,
}

impl ConnectionKey {
    pub fn new(client: Endpoint, server: Endpoint, transport: TransportProtocol) -> Self {
        Self {
            network_namespace: None,
            client,
            server,
            transport,
        }
    }
}

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(RecordingId);
uuid_id!(SessionId);
uuid_id!(ConnectionId);
uuid_id!(OperationId);
uuid_id!(ReplayRunId);

/// Escape control characters for safe human rendering of untrusted references
/// (recording names, IDs from input). Non-control characters pass through
/// unchanged; control characters render as `\u{...}` escapes so untrusted
/// input cannot inject terminal sequences or newlines.
pub fn escape_control(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for character in input.chars() {
        if character.is_control() {
            use std::fmt::Write as _;
            let _ = write!(out, "\\u{{{:x}}}", character as u32);
        } else {
            out.push(character);
        }
    }
    out
}

impl RecordingId {
    /// CLI display form: `rec_<full-uuid>`.
    ///
    /// Presentation-only: the internal `Display`/serde representation stays the
    /// bare UUID so storage formats are unchanged.
    pub fn to_cli_string(&self) -> String {
        format!("rec_{}", self.0)
    }

    /// Parse a public CLI recording reference: `rec_<full-uuid>` or, for
    /// compatibility with the legacy surface, a bare full UUID.
    ///
    /// Rejects partial/prefix forms (`rec_abc12`), case variants of the `rec_`
    /// prefix, and any other input.
    pub fn parse_cli(input: &str) -> Result<RecordingId, RecordingIdParseError> {
        let rest = input.strip_prefix("rec_").unwrap_or(input);
        let uuid = Uuid::parse_str(rest).map_err(|_| RecordingIdParseError::new(input))?;
        Ok(RecordingId(uuid))
    }
}

/// Error returned when a CLI recording reference cannot be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingIdParseError {
    input: String,
}

impl RecordingIdParseError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for RecordingIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid recording reference '{}': expected 'rec_<full-uuid>' or a full UUID",
            self.input
        )
    }
}

impl std::error::Error for RecordingIdParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RecordingId {
        RecordingId(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap())
    }

    #[test]
    fn cli_string_is_prefixed_full_uuid() {
        assert_eq!(
            sample().to_cli_string(),
            "rec_00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn display_remains_bare_uuid_for_storage_compat() {
        assert_eq!(sample().to_string(), "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn parses_prefixed_form() {
        let parsed = RecordingId::parse_cli("rec_00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(parsed, sample());
    }

    #[test]
    fn parses_bare_uuid_form() {
        let parsed = RecordingId::parse_cli("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(parsed, sample());
    }

    #[test]
    fn roundtrip_prefixed_form() {
        let id = sample();
        assert_eq!(RecordingId::parse_cli(&id.to_cli_string()).unwrap(), id);
    }

    #[test]
    fn rejects_partial_prefix() {
        assert!(RecordingId::parse_cli("rec_abc12").is_err());
        assert!(RecordingId::parse_cli("rec_").is_err());
        assert!(RecordingId::parse_cli("rec").is_err());
    }

    #[test]
    fn rejects_case_variant_prefix() {
        assert!(RecordingId::parse_cli("REC_00000000-0000-0000-0000-000000000001").is_err());
        assert!(RecordingId::parse_cli("Rec_00000000-0000-0000-0000-000000000001").is_err());
    }

    #[test]
    fn rejects_truncated_uuid() {
        assert!(RecordingId::parse_cli("00000000-0000-0000-0000-0000000000").is_err());
        assert!(RecordingId::parse_cli("rec_00000000-0000-0000-0000-0000000000").is_err());
    }

    #[test]
    fn rejects_unrelated_input() {
        assert!(RecordingId::parse_cli("latest").is_err());
        assert!(RecordingId::parse_cli("").is_err());
        assert!(RecordingId::parse_cli("not-a-uuid").is_err());
    }

    #[test]
    fn error_is_actionable_and_echoes_input() {
        let err = RecordingId::parse_cli("rec_abc12").unwrap_err();
        assert_eq!(err.input(), "rec_abc12");
        assert!(err.to_string().contains("rec_<full-uuid>"));
    }

    #[test]
    fn escape_control_renders_untrusted_references_safely() {
        assert_eq!(escape_control("checkout"), "checkout");
        assert_eq!(escape_control("a\nb"), "a\\u{a}b");
        assert_eq!(escape_control("x\x1b[31m"), "x\\u{1b}[31m");
        assert_eq!(escape_control("\u{0}"), "\\u{0}");
        assert_eq!(escape_control(""), "");
    }
}
