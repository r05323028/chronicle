//! Statically linked protocol registrations.
//!
//! `fake` and bounded plaintext HTTP/1.1 are functional. Other modules declare honest target status.

use chronicle_common::ProtocolId;
use chronicle_protocol::{
    CapabilityStatus, ProtocolCapabilities, ProtocolError, ProtocolRegistration, ProtocolRegistry,
};

fn scaffold_registration(
    id: &str,
    display_name: &'static str,
    capabilities: ProtocolCapabilities,
) -> ProtocolRegistration {
    ProtocolRegistration {
        id: ProtocolId::new(id),
        display_name,
        capabilities,
        detector: None,
        decoder_factory: None,
        canonicalizer: None,
        replay_adapter: None,
        verifier: None,
    }
}

const PLANNED: ProtocolCapabilities = ProtocolCapabilities {
    detection: CapabilityStatus::Planned,
    decoding: CapabilityStatus::Planned,
    canonicalization: CapabilityStatus::Planned,
    replay: CapabilityStatus::Planned,
    verification: CapabilityStatus::Planned,
};

pub mod http {
    use super::ProtocolRegistration;
    use chronicle_canonical::{
        Attributes, CanonicalOperation, CanonicalWarning, OperationEffect, OperationKind,
        PayloadRef, ProtocolData, RelativeTimeNanos,
    };
    use chronicle_common::{Direction, Endpoint, OperationId, ProtocolId};
    use chronicle_protocol::{
        BoxFuture, CapabilityStatus, DecodedFrame, DetectionInput, DetectionResult,
        ObservedResponse, ProtocolCanonicalizer, ProtocolCapabilities, ProtocolDetector,
        ProtocolError, ProtocolStream, ReplayAdapter, ReplayConnection, ReplayContext,
        TransportErrorCategory, VerificationResult, VerificationStatus, Verifier,
    };
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, VecDeque};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    pub const MAX_HEAD_BYTES: usize = 64 * 1024;
    pub const MAX_HEADER_COUNT: usize = 128;
    pub const PROTOCOL_DATA_SCHEMA_VERSION: u16 = 1;
    pub const PROTOCOL_DATA_MEDIA_TYPE: &str =
        "application/vnd.chronicle.http-operation+json;version=1";
    pub const OBSERVED_DATA_MEDIA_TYPE: &str =
        "application/vnd.chronicle.http-observed-response+json;version=1";
    const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
    const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WarningCode {
        Malformed,
        TruncatedMessage,
        UnsupportedTransferEncoding,
        UnsupportedCloseDelimitedBody,
        UnsupportedInformationalResponse,
        UnsupportedTarget,
        UnsupportedUpgrade,
        UnsupportedVersion,
        Pipelined,
    }

    impl WarningCode {
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Malformed => "malformed_http_message",
                Self::TruncatedMessage => "truncated_message",
                Self::UnsupportedTransferEncoding => "unsupported_transfer_encoding",
                Self::UnsupportedCloseDelimitedBody => "unsupported_close_delimited_body",
                Self::UnsupportedInformationalResponse => "unsupported_informational_response",
                Self::UnsupportedTarget => "unsupported_request_target",
                Self::UnsupportedUpgrade => "unsupported_upgrade",
                Self::UnsupportedVersion => "unsupported_http_version",
                Self::Pipelined => "pipelined_requests",
            }
        }
    }

    pub struct Detector {
        id: ProtocolId,
    }

    impl Detector {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
            }
        }
    }

    impl Default for Detector {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ProtocolDetector for Detector {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn detect(&self, input: DetectionInput<'_>) -> DetectionResult {
            let mut client = Vec::new();
            let mut server = Vec::new();
            for chunk in &input.stream.chunks {
                let target = match chunk.direction {
                    Direction::ClientToServer => &mut client,
                    Direction::ServerToClient => &mut server,
                };
                if target.len() < MAX_HEAD_BYTES {
                    let remaining = MAX_HEAD_BYTES - target.len();
                    target.extend_from_slice(&chunk.payload[..chunk.payload.len().min(remaining)]);
                }
            }
            if client.is_empty() {
                detect_response_prefix(&server)
            } else {
                detect_request_prefix(&client).unwrap_or_else(|| detect_response_prefix(&server))
            }
        }
    }

    fn detect_request_prefix(bytes: &[u8]) -> Option<DetectionResult> {
        if bytes.starts_with(&[0x16, 0x03]) || bytes.starts_with(b"PRI * HTTP/2.0") {
            return Some(DetectionResult::Rejected);
        }
        let Some(line_end) = bytes.windows(2).position(|window| window == b"\r\n") else {
            if bytes.len() >= MAX_HEAD_BYTES {
                return Some(DetectionResult::Rejected);
            }
            return is_request_prefix(bytes).then_some(DetectionResult::NeedMoreData {
                minimum_additional_bytes: 1,
            });
        };
        let line = &bytes[..line_end];
        let mut parts = line.split(|byte| *byte == b' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Some(DetectionResult::Rejected);
        };
        if is_token(method) && target.starts_with(b"/") && version == b"HTTP/1.1" {
            Some(DetectionResult::Confirmed {
                confidence: 100,
                evidence: "validated HTTP/1.1 request line".into(),
            })
        } else {
            Some(DetectionResult::Rejected)
        }
    }

    fn detect_response_prefix(bytes: &[u8]) -> DetectionResult {
        if bytes.starts_with(b"HTTP/1.1 ") {
            if bytes.windows(2).any(|window| window == b"\r\n") {
                DetectionResult::Probable {
                    confidence: 90,
                    evidence: "HTTP/1.1 status line".into(),
                }
            } else {
                DetectionResult::NeedMoreData {
                    minimum_additional_bytes: 1,
                }
            }
        } else {
            DetectionResult::Unknown
        }
    }

    fn is_request_prefix(bytes: &[u8]) -> bool {
        bytes.is_empty()
            || bytes
                .iter()
                .take_while(|byte| **byte != b' ')
                .all(|byte| is_token_byte(*byte))
    }

    fn is_token(bytes: &[u8]) -> bool {
        !bytes.is_empty() && bytes.iter().all(|byte| is_token_byte(*byte))
    }

    const fn is_token_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct HeaderV1 {
        pub name: String,
        pub value: Vec<u8>,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum MessageKindV1 {
        Request,
        Response,
        Opaque,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct MessageV1 {
        pub kind: MessageKindV1,
        pub sequence: u64,
        pub method: Option<String>,
        pub target: Option<String>,
        pub status: Option<u16>,
        pub reason: Option<Vec<u8>>,
        pub headers: Vec<HeaderV1>,
        pub body: Vec<u8>,
        pub pipeline_depth: usize,
        pub orphan_response: bool,
        #[serde(default)]
        pub warnings: Vec<String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum TargetFormV1 {
        Origin,
        Opaque,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ReplayAttributesV1 {
        pub target_form: TargetFormV1,
        pub captured_sensitive_headers: bool,
        pub replayable: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct VerificationMetadataV1 {
        pub expected_status: Option<u16>,
        pub expects_response: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct HttpOperationDataV1 {
        pub method: Option<String>,
        pub target: Option<String>,
        pub request_headers: Vec<HeaderV1>,
        pub response_headers: Vec<HeaderV1>,
        pub response_status: Option<u16>,
        pub response_reason: Option<Vec<u8>>,
        pub request_sequence: u64,
        pub response_sequence: Option<u64>,
        pub pipeline_depth: usize,
        #[serde(default)]
        pub warnings: Vec<String>,
        pub replay: ReplayAttributesV1,
        pub verification: VerificationMetadataV1,
    }

    /// Rewrites captured request headers for an outbound replay request.
    ///
    /// Header values are retained only for end-to-end fields; callers must not render them.
    pub fn sanitize_request_headers(
        request_headers: &[HeaderV1],
        target: &Endpoint,
        body_len: usize,
    ) -> Vec<HeaderV1> {
        let connection_tokens: Vec<String> = request_headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("connection"))
            .flat_map(|header| header.value.split(|byte| *byte == b','))
            .filter_map(|token| std::str::from_utf8(token.trim_ascii()).ok())
            .map(str::to_ascii_lowercase)
            .collect();
        let mut headers = vec![HeaderV1 {
            name: "host".into(),
            value: target_authority(target).into_bytes(),
        }];
        headers.extend(request_headers.iter().filter_map(|header| {
            let name = header.name.to_ascii_lowercase();
            if is_stripped_replay_header(&name)
                || connection_tokens.iter().any(|token| token == &name)
            {
                None
            } else {
                Some(HeaderV1 {
                    name,
                    value: header.value.clone(),
                })
            }
        }));
        headers.push(HeaderV1 {
            name: "content-length".into(),
            value: body_len.to_string().into_bytes(),
        });
        headers
    }

    fn target_authority(target: &Endpoint) -> String {
        if target.host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("[{}]:{}", target.host, target.port)
        } else {
            format!("{}:{}", target.host, target.port)
        }
    }

    fn is_stripped_replay_header(name: &str) -> bool {
        matches!(
            name,
            "host"
                | "connection"
                | "proxy-connection"
                | "keep-alive"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "authorization"
                | "proxy-authorization"
                | "cookie"
                | "forwarded"
                | "expect"
                | "content-length"
        ) || name.starts_with("x-forwarded-")
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct HttpObservedResponseV1 {
        pub status: u16,
        pub headers: Vec<HeaderV1>,
    }

    impl HttpObservedResponseV1 {
        fn to_protocol_data(&self) -> ProtocolData {
            ProtocolData {
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: Some(OBSERVED_DATA_MEDIA_TYPE.into()),
                bytes: serde_json::to_vec(self).expect("HTTP observed response must serialize"),
            }
        }

        fn from_protocol_data(data: &ProtocolData) -> Result<Self, String> {
            if data.schema_version != PROTOCOL_DATA_SCHEMA_VERSION
                || data.media_type.as_deref() != Some(OBSERVED_DATA_MEDIA_TYPE)
            {
                return Err("unsupported HTTP observed response version".into());
            }
            serde_json::from_slice(&data.bytes).map_err(|error| error.to_string())
        }
    }

    pub struct HttpVerifier {
        id: ProtocolId,
    }

    impl HttpVerifier {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
            }
        }
    }

    impl Default for HttpVerifier {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Verifier for HttpVerifier {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn verify(
            &self,
            operation: &CanonicalOperation,
            observed: &ObservedResponse,
        ) -> VerificationResult {
            let Ok(expected) = HttpOperationDataV1::from_protocol_data(&operation.protocol_data)
            else {
                return verification(
                    VerificationStatus::Unsupported,
                    "unsupported HTTP expectation",
                );
            };
            let observed_body = &observed.payload;
            let Some(protocol_data) = observed.protocol_data.as_ref() else {
                return verification(
                    VerificationStatus::Unsupported,
                    "unsupported HTTP observed response",
                );
            };
            let Ok(observed) = HttpObservedResponseV1::from_protocol_data(protocol_data) else {
                return verification(
                    VerificationStatus::Unsupported,
                    "unsupported HTTP observed response",
                );
            };
            let Some(expected_status) = expected.response_status else {
                return verification(
                    VerificationStatus::Inconclusive,
                    "missing recorded HTTP response",
                );
            };
            if expected_status != observed.status {
                return verification_with(
                    VerificationStatus::Failed,
                    "HTTP status differed",
                    [
                        ("expected_status", expected_status.to_string()),
                        ("observed_status", observed.status.to_string()),
                    ],
                );
            }
            let expected_headers = comparable_headers(&expected.response_headers);
            let observed_headers = comparable_headers(&observed.headers);
            if expected_headers != observed_headers {
                let header = expected_headers
                    .iter()
                    .zip(&observed_headers)
                    .find(|(expected, observed)| expected != observed)
                    .map(|(header, _)| header.name.as_str())
                    .or_else(|| {
                        expected_headers
                            .get(observed_headers.len())
                            .map(|header| header.name.as_str())
                    })
                    .or_else(|| {
                        observed_headers
                            .get(expected_headers.len())
                            .map(|header| header.name.as_str())
                    })
                    .unwrap_or("header_count");
                return verification_with(
                    VerificationStatus::Failed,
                    "HTTP response headers differed",
                    [
                        ("header", header.to_owned()),
                        ("expected_header_count", expected_headers.len().to_string()),
                        ("observed_header_count", observed_headers.len().to_string()),
                    ],
                );
            }
            match (&operation.recorded_response, observed_body) {
                (
                    Some(PayloadRef::Inline {
                        bytes: expected, ..
                    }),
                    Some(PayloadRef::Inline {
                        bytes: observed, ..
                    }),
                ) if expected == observed => {
                    verification(VerificationStatus::Passed, "HTTP response matched")
                }
                (
                    Some(PayloadRef::Inline {
                        bytes: expected, ..
                    }),
                    Some(PayloadRef::Inline {
                        bytes: observed, ..
                    }),
                ) => verification_with(
                    VerificationStatus::Failed,
                    "HTTP response body differed",
                    [
                        ("expected_body_size", expected.len().to_string()),
                        ("observed_body_size", observed.len().to_string()),
                        ("expected_body_sha256", digest(expected)),
                        ("observed_body_sha256", digest(observed)),
                    ],
                ),
                _ => verification(
                    VerificationStatus::Inconclusive,
                    "HTTP response body unavailable",
                ),
            }
        }
    }

    fn comparable_headers(headers: &[HeaderV1]) -> Vec<HeaderV1> {
        headers
            .iter()
            .filter(|header| {
                !matches!(
                    header.name.to_ascii_lowercase().as_str(),
                    "date"
                        | "server"
                        | "content-length"
                        | "connection"
                        | "transfer-encoding"
                        | "keep-alive"
                        | "set-cookie"
                )
            })
            .cloned()
            .collect()
    }

    fn digest(bytes: &[u8]) -> String {
        let mut output = String::from("sha256:");
        for byte in Sha256::digest(bytes) {
            use std::fmt::Write;
            write!(output, "{byte:02x}").expect("writing into String cannot fail");
        }
        output
    }

    fn verification(status: VerificationStatus, summary: &str) -> VerificationResult {
        verification_with(status, summary, [])
    }

    fn verification_with<const N: usize>(
        status: VerificationStatus,
        summary: &str,
        details: [(&str, String); N],
    ) -> VerificationResult {
        VerificationResult {
            status,
            summary: summary.into(),
            details: details
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        }
    }

    pub struct HttpReplayAdapter {
        id: ProtocolId,
    }

    impl HttpReplayAdapter {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
            }
        }
    }

    impl Default for HttpReplayAdapter {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ReplayAdapter for HttpReplayAdapter {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn connect<'a>(
            &'a self,
            target: &'a Endpoint,
            context: &'a ReplayContext,
        ) -> BoxFuture<'a, Result<Box<dyn ReplayConnection>, ProtocolError>> {
            Box::pin(async move {
                if !context.authorizes_execution_for(target) {
                    return Err(malformed("HTTP replay execution is not authorized"));
                }
                let address = target_address(target)?;
                let stream = timeout(OPERATION_TIMEOUT, TcpStream::connect(address))
                    .await
                    .map_err(|_| transport(TransportErrorCategory::Timeout, "connect timed out"))?
                    .map_err(|error| transport(transport_category(&error), "connect failed"))?;
                Ok(Box::new(HttpReplayConnection {
                    stream,
                    target: target.clone(),
                    context: context.clone(),
                }) as Box<dyn ReplayConnection>)
            })
        }
    }

    struct HttpReplayConnection {
        stream: TcpStream,
        target: Endpoint,
        context: ReplayContext,
    }

    impl ReplayConnection for HttpReplayConnection {
        fn execute<'a>(
            &'a mut self,
            operation: &'a CanonicalOperation,
        ) -> BoxFuture<'a, Result<ObservedResponse, ProtocolError>> {
            Box::pin(async move {
                timeout(
                    OPERATION_TIMEOUT,
                    execute_http(&mut self.stream, &self.target, &self.context, operation),
                )
                .await
                .map_err(|_| {
                    transport(TransportErrorCategory::Timeout, "HTTP operation timed out")
                })?
            })
        }
    }

    async fn execute_http(
        stream: &mut TcpStream,
        target: &Endpoint,
        context: &ReplayContext,
        operation: &CanonicalOperation,
    ) -> Result<ObservedResponse, ProtocolError> {
        let data = HttpOperationDataV1::from_protocol_data(&operation.protocol_data)
            .map_err(|_| malformed("invalid HTTP replay operation"))?;
        let method = data
            .method
            .ok_or_else(|| malformed("missing HTTP method"))?;
        let request_target = data
            .target
            .ok_or_else(|| malformed("missing HTTP request target"))?;
        let body = match &operation.request {
            PayloadRef::Inline { bytes, .. } => bytes.as_slice(),
            _ => return Err(malformed("HTTP request payload was not hydrated")),
        };
        let mut headers = sanitize_request_headers(&data.request_headers, target, body.len());
        if let Some(authorization) = context.credentials.get("authorization") {
            if !authorization
                .expose()
                .iter()
                .copied()
                .all(valid_field_value_byte)
            {
                return Err(malformed("invalid runtime Authorization field value"));
            }
            headers.insert(
                1,
                HeaderV1 {
                    name: "authorization".into(),
                    value: authorization.expose().to_vec(),
                },
            );
        }
        headers.push(HeaderV1 {
            name: "connection".into(),
            value: b"close".to_vec(),
        });
        let mut wire = format!("{method} {request_target} HTTP/1.1\r\n").into_bytes();
        for header in &headers {
            wire.extend_from_slice(header.name.as_bytes());
            wire.extend_from_slice(b": ");
            wire.extend_from_slice(&header.value);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"\r\n");
        wire.extend_from_slice(body);
        stream
            .write_all(&wire)
            .await
            .map_err(|error| transport(transport_category(&error), "request write failed"))?;
        read_response(stream, &method).await
    }

    async fn read_response(
        stream: &mut TcpStream,
        method: &str,
    ) -> Result<ObservedResponse, ProtocolError> {
        let mut bytes = Vec::new();
        loop {
            let mut raw_headers = [httparse::EMPTY_HEADER; MAX_HEADER_COUNT];
            let mut response = httparse::Response::new(&mut raw_headers);
            match response
                .parse(&bytes)
                .map_err(|_| malformed("malformed HTTP response"))?
            {
                httparse::Status::Partial => {
                    if bytes.len() >= MAX_HEAD_BYTES {
                        return Err(malformed("HTTP response head exceeds limit"));
                    }
                }
                httparse::Status::Complete(head_len) => {
                    let status = response
                        .code
                        .ok_or_else(|| malformed("missing HTTP status"))?;
                    if (100..200).contains(&status) {
                        return Err(transport(
                            TransportErrorCategory::UnsupportedFraming,
                            "informational response is unsupported",
                        ));
                    }
                    let headers = normalize_headers(response.headers)?;
                    reject_unsupported_headers(&headers).map_err(|_| {
                        transport(
                            TransportErrorCategory::UnsupportedFraming,
                            "upgrade response is unsupported",
                        )
                    })?;
                    let no_body = method == "HEAD" || matches!(status, 204 | 304);
                    let body_len = if no_body {
                        0
                    } else if headers
                        .iter()
                        .any(|header| header.name == "transfer-encoding")
                    {
                        return Err(transport(
                            TransportErrorCategory::UnsupportedFraming,
                            "transfer encoding is unsupported",
                        ));
                    } else if headers.iter().any(|header| header.name == "content-length") {
                        content_length(&headers)?
                    } else {
                        return Err(transport(
                            TransportErrorCategory::UnsupportedFraming,
                            "close-delimited response is unsupported",
                        ));
                    };
                    if body_len > MAX_RESPONSE_BYTES
                        || head_len.saturating_add(body_len) > MAX_RESPONSE_BYTES
                    {
                        return Err(transport(
                            TransportErrorCategory::UnsupportedFraming,
                            "response exceeds byte limit",
                        ));
                    }
                    while bytes.len() < head_len + body_len {
                        read_more(stream, &mut bytes).await?;
                    }
                    return Ok(ObservedResponse {
                        payload: Some(PayloadRef::Inline {
                            content_type: None,
                            bytes: bytes[head_len..head_len + body_len].to_vec(),
                        }),
                        protocol_data: Some(
                            HttpObservedResponseV1 { status, headers }.to_protocol_data(),
                        ),
                        attributes: BTreeMap::new(),
                        error_category: None,
                    });
                }
            }
            read_more(stream, &mut bytes).await?;
        }
    }

    async fn read_more(stream: &mut TcpStream, bytes: &mut Vec<u8>) -> Result<(), ProtocolError> {
        let mut buffer = [0_u8; 4096];
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|error| transport(transport_category(&error), "response read failed"))?;
        if count == 0 {
            return Err(transport(
                TransportErrorCategory::Disconnect,
                "response ended before complete framing",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        Ok(())
    }

    fn valid_field_value_byte(byte: u8) -> bool {
        byte == b'\t' || (0x20..=0x7e).contains(&byte) || byte >= 0x80
    }

    fn transport(category: TransportErrorCategory, message: &str) -> ProtocolError {
        ProtocolError::Transport {
            category,
            message: message.into(),
        }
    }

    fn transport_category(error: &std::io::Error) -> TransportErrorCategory {
        if error.kind() == std::io::ErrorKind::ConnectionRefused {
            TransportErrorCategory::Refused
        } else {
            TransportErrorCategory::Io
        }
    }

    fn target_address(target: &Endpoint) -> Result<std::net::SocketAddr, ProtocolError> {
        let ip = target
            .host
            .parse::<std::net::IpAddr>()
            .map_err(|_| malformed("HTTP replay target must be an IP literal"))?;
        if !ip.is_loopback() {
            return Err(malformed("HTTP replay target must be loopback"));
        }
        Ok(std::net::SocketAddr::new(ip, target.port))
    }

    impl HttpOperationDataV1 {
        /// # Panics
        ///
        /// Panics if this serializable type cannot be encoded as JSON.
        pub fn into_protocol_data(&self) -> ProtocolData {
            ProtocolData {
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: Some(PROTOCOL_DATA_MEDIA_TYPE.into()),
                bytes: serde_json::to_vec(self).expect("HTTP operation data must serialize"),
            }
        }

        pub fn from_protocol_data(data: &ProtocolData) -> Result<Self, String> {
            if data.schema_version != PROTOCOL_DATA_SCHEMA_VERSION
                || data.media_type.as_deref() != Some(PROTOCOL_DATA_MEDIA_TYPE)
            {
                return Err("unsupported HTTP operation data version".into());
            }
            serde_json::from_slice(&data.bytes).map_err(|error| error.to_string())
        }
    }

    pub struct DecoderFactory {
        id: ProtocolId,
    }

    impl DecoderFactory {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
            }
        }
    }

    impl Default for DecoderFactory {
        fn default() -> Self {
            Self::new()
        }
    }

    impl chronicle_protocol::DecoderFactory for DecoderFactory {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn create(&self) -> Box<dyn chronicle_protocol::ProtocolDecoder> {
            Box::new(Decoder::new())
        }
    }

    pub struct Canonicalizer {
        id: ProtocolId,
    }

    impl Canonicalizer {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
            }
        }

        fn offset(
            stream: &ProtocolStream<'_>,
            sequence: u64,
        ) -> (RelativeTimeNanos, Vec<CanonicalWarning>) {
            match (
                stream.started_at,
                stream
                    .chunks
                    .iter()
                    .find(|chunk| chunk.sequence == sequence)
                    .and_then(|chunk| chunk.timestamp),
            ) {
                (Some(start), Some(timestamp)) => (
                    RelativeTimeNanos(
                        u64::try_from((timestamp - start).whole_nanoseconds().max(0))
                            .unwrap_or(u64::MAX),
                    ),
                    Vec::new(),
                ),
                _ => (
                    RelativeTimeNanos(0),
                    vec![CanonicalWarning {
                        code: "missing_timestamp".into(),
                        message: "operation timestamp unavailable; offset set to zero".into(),
                    }],
                ),
            }
        }

        fn effect(method: Option<&str>) -> OperationEffect {
            match method {
                Some("GET" | "HEAD" | "OPTIONS") => OperationEffect::Read,
                Some("POST" | "PUT" | "PATCH" | "DELETE") => OperationEffect::Write,
                _ => OperationEffect::Unknown,
            }
        }

        #[allow(clippy::too_many_lines)] // Canonical operation fields stay co-located for schema review.
        fn operation(
            stream: &ProtocolStream<'_>,
            request: MessageV1,
            response: Option<MessageV1>,
        ) -> CanonicalOperation {
            let (started_at_offset, mut warnings) = Self::offset(stream, request.sequence);
            let completed_at_offset = response
                .as_ref()
                .map(|message| Self::offset(stream, message.sequence).0);
            let mut codes = request.warnings.clone();
            if let Some(message) = &response {
                codes.extend(message.warnings.clone());
            }
            if response
                .as_ref()
                .is_some_and(|message| message.orphan_response)
            {
                codes.push("orphan_response".into());
            }
            warnings.extend(codes.iter().map(|code| CanonicalWarning {
                code: code.clone(),
                message: format!("HTTP decoder warning: {code}"),
            }));
            let response_status = response.as_ref().and_then(|message| message.status);
            let response_headers = response
                .as_ref()
                .map_or_else(Vec::new, |message| message.headers.clone());
            let response_reason = response.as_ref().and_then(|message| message.reason.clone());
            let response_sequence = response.as_ref().map(|message| message.sequence);
            let incomplete = response.is_none()
                || request.kind == MessageKindV1::Opaque
                || response
                    .as_ref()
                    .is_some_and(|message| message.kind == MessageKindV1::Opaque);
            let truncated = stream.truncated
                || codes
                    .iter()
                    .any(|code| code == WarningCode::TruncatedMessage.as_str());
            let replayable = !incomplete && request.pipeline_depth <= 1 && request.method.is_some();
            let captured_sensitive_headers = request
                .headers
                .iter()
                .any(|header| matches!(header.name.as_str(), "authorization" | "cookie"));
            let requires_runtime_authorization = request.headers.iter().any(|header| {
                matches!(
                    header.name.as_str(),
                    "authorization" | "proxy-authorization"
                )
            });
            let protocol_data = HttpOperationDataV1 {
                method: request.method.clone(),
                target: request.target.clone(),
                request_headers: request.headers.clone(),
                response_headers,
                response_status,
                response_reason,
                request_sequence: request.sequence,
                response_sequence,
                pipeline_depth: request.pipeline_depth,
                warnings: codes,
                replay: ReplayAttributesV1 {
                    target_form: if request.target.is_some() {
                        TargetFormV1::Origin
                    } else {
                        TargetFormV1::Opaque
                    },
                    captured_sensitive_headers,
                    replayable,
                },
                verification: VerificationMetadataV1 {
                    expected_status: response_status,
                    expects_response: response.is_some(),
                },
            }
            .into_protocol_data();
            let mut attributes = Attributes::new();
            if let Some(method) = &request.method {
                attributes.insert("http.method".into(), method.clone());
            }
            if let Some(target) = &request.target {
                attributes.insert("http.request_target".into(), target.clone());
            }
            if let Some(status) = response_status {
                attributes.insert("http.response_status".into(), status.to_string());
            }
            attributes.insert("chronicle.replayable".into(), replayable.to_string());
            if requires_runtime_authorization {
                attributes.insert(
                    "chronicle.replay.requires_runtime_authorization".into(),
                    "true".into(),
                );
            }
            CanonicalOperation {
                id: OperationId::new(),
                sequence: request.sequence,
                started_at_offset,
                completed_at_offset,
                kind: OperationKind::Request,
                effect: Self::effect(request.method.as_deref()),
                request: PayloadRef::Inline {
                    content_type: None,
                    bytes: request.body,
                },
                recorded_response: response.map(|message| PayloadRef::Inline {
                    content_type: None,
                    bytes: message.body,
                }),
                attributes,
                protocol_data,
                incomplete,
                truncated,
                redactions: Vec::new(),
                warnings,
            }
        }
    }

    impl Default for Canonicalizer {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ProtocolCanonicalizer for Canonicalizer {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }
        fn canonicalize(
            &self,
            stream: &ProtocolStream<'_>,
            frames: Vec<DecodedFrame>,
        ) -> Result<Vec<CanonicalOperation>, ProtocolError> {
            let mut pending = VecDeque::new();
            let mut operations = Vec::new();
            for frame in frames {
                let message: MessageV1 =
                    serde_json::from_slice(&frame.payload).map_err(|error| {
                        ProtocolError::Malformed {
                            protocol: self.id.clone(),
                            message: error.to_string(),
                        }
                    })?;
                match frame.direction {
                    Direction::ClientToServer => pending.push_back(message),
                    Direction::ServerToClient => {
                        if let Some(request) = pending.pop_front() {
                            operations.push(Self::operation(stream, request, Some(message)));
                        } else {
                            let mut opaque = message.clone();
                            opaque.kind = MessageKindV1::Opaque;
                            opaque.method = None;
                            opaque.target = None;
                            opaque.status = None;
                            opaque.reason = None;
                            opaque.headers.clear();
                            opaque.body.clear();
                            opaque.warnings.push("orphan_response".into());
                            operations.push(Self::operation(stream, opaque, Some(message)));
                        }
                    }
                }
            }
            operations.extend(
                pending
                    .into_iter()
                    .map(|request| Self::operation(stream, request, None)),
            );
            Ok(operations)
        }
    }

    pub struct Decoder {
        id: ProtocolId,
        client: DirectionBuffer,
        server: DirectionBuffer,
        pending_methods: VecDeque<String>,
    }

    impl Decoder {
        pub fn new() -> Self {
            Self {
                id: ProtocolId::new("http/1.1"),
                client: DirectionBuffer::default(),
                server: DirectionBuffer::default(),
                pending_methods: VecDeque::new(),
            }
        }

        /// Feed protocol-neutral reconstruction fragments into existing bounded HTTP parsing.
        pub fn push_reconstructed(
            &mut self,
            connection: &chronicle_protocol::ProtocolNeutralConnection,
        ) -> Result<Vec<chronicle_protocol::DecodedFrame>, chronicle_protocol::ProtocolError>
        {
            let mut decoded = Vec::new();
            for frame in chronicle_protocol::reconstructed_frames(connection) {
                decoded.extend(chronicle_protocol::ProtocolDecoder::push(self, frame)?);
            }
            Ok(decoded)
        }

        /// Finalize reconstructed input; close-delimited response bodies require trusted evidence.
        pub fn finish_reconstructed(
            &mut self,
            connection: &chronicle_protocol::ProtocolNeutralConnection,
        ) -> Result<Vec<chronicle_protocol::DecodedFrame>, chronicle_protocol::ProtocolError>
        {
            let trusted = trusted_close(
                connection.termination,
                connection.finalization,
                connection
                    .loss_windows
                    .iter()
                    .map(|loss| loss.classification),
            );
            if !trusted || self.server.bytes.is_empty() {
                return chronicle_protocol::ProtocolDecoder::finish(self);
            }
            let sequence = self.server.start_sequence.unwrap_or_default();
            let head = self
                .pending_methods
                .front()
                .is_some_and(|method| method == "HEAD");
            let Some((_, mut message)) =
                parse_close_delimited_response(&self.server.bytes, sequence, head)?
            else {
                return chronicle_protocol::ProtocolDecoder::finish(self);
            };
            message.pipeline_depth = self.pending_methods.len();
            message.orphan_response = self.pending_methods.pop_front().is_none();
            self.server.bytes.clear();
            self.server.start_sequence = None;
            encode_messages(Direction::ServerToClient, vec![message])
        }
    }

    fn trusted_close(
        termination: chronicle_protocol::DerivedTermination,
        finalization: Option<chronicle_protocol::ReconstructionFinalization>,
        losses: impl IntoIterator<Item = chronicle_protocol::LossWindowClassification>,
    ) -> bool {
        termination == chronicle_protocol::DerivedTermination::CleanClose
            && finalization.is_none()
            && losses
                .into_iter()
                .all(|loss| loss == chronicle_protocol::LossWindowClassification::Outside)
    }

    impl Default for Decoder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl chronicle_protocol::ProtocolDecoder for Decoder {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn push(
            &mut self,
            frame: chronicle_protocol::DecodedFrame,
        ) -> Result<Vec<chronicle_protocol::DecodedFrame>, chronicle_protocol::ProtocolError>
        {
            let mut messages = match frame.direction {
                Direction::ClientToServer => {
                    self.client.push(frame.sequence, &frame.payload);
                    self.client.decode(Direction::ClientToServer, false)
                }
                Direction::ServerToClient => {
                    self.server.push(frame.sequence, &frame.payload);
                    let mut messages = Vec::new();
                    while let Some(mut message) = self.server.decode_one(
                        Direction::ServerToClient,
                        self.pending_methods
                            .front()
                            .is_some_and(|method| method == "HEAD"),
                    ) {
                        if message.kind == MessageKindV1::Response {
                            message.pipeline_depth = self.pending_methods.len();
                            message.orphan_response = self.pending_methods.pop_front().is_none();
                        }
                        messages.push(message);
                    }
                    messages
                }
            };
            if frame.direction == Direction::ClientToServer {
                for message in &mut messages {
                    if let Some(method) = &message.method {
                        self.pending_methods.push_back(method.clone());
                        message.pipeline_depth = self.pending_methods.len();
                    }
                }
            }
            encode_messages(frame.direction, messages)
        }

        fn finish(
            &mut self,
        ) -> Result<Vec<chronicle_protocol::DecodedFrame>, chronicle_protocol::ProtocolError>
        {
            let mut frames = Vec::new();
            if let Some(message) = self.client.take_opaque(WarningCode::TruncatedMessage) {
                frames.extend(encode_messages(Direction::ClientToServer, vec![message])?);
            }
            if let Some(message) = self.server.take_opaque(WarningCode::TruncatedMessage) {
                frames.extend(encode_messages(Direction::ServerToClient, vec![message])?);
            }
            Ok(frames)
        }
    }

    #[derive(Default)]
    struct DirectionBuffer {
        bytes: Vec<u8>,
        start_sequence: Option<u64>,
    }

    impl DirectionBuffer {
        fn push(&mut self, sequence: u64, payload: &[u8]) {
            if self.bytes.is_empty() {
                self.start_sequence = Some(sequence);
            }
            self.bytes.extend_from_slice(payload);
        }

        fn decode(&mut self, direction: Direction, head_response: bool) -> Vec<MessageV1> {
            let mut messages = Vec::new();
            while let Some(message) = self.decode_one(direction, head_response) {
                messages.push(message);
            }
            messages
        }

        fn decode_one(&mut self, direction: Direction, head_response: bool) -> Option<MessageV1> {
            let sequence = self.start_sequence.unwrap_or_default();
            match parse_message(&self.bytes, direction, sequence, head_response) {
                Ok(Some((consumed, mut message))) => {
                    message.sequence = sequence;
                    self.bytes.drain(..consumed);
                    self.start_sequence = (!self.bytes.is_empty()).then_some(sequence);
                    Some(message)
                }
                Ok(None) => None,
                Err(error) => self.take_opaque(warning_for(&error)),
            }
        }

        fn take_opaque(&mut self, warning: WarningCode) -> Option<MessageV1> {
            (!self.bytes.is_empty()).then(|| MessageV1 {
                kind: MessageKindV1::Opaque,
                sequence: self.start_sequence.unwrap_or_default(),
                method: None,
                target: None,
                status: None,
                reason: None,
                headers: Vec::new(),
                body: std::mem::take(&mut self.bytes),
                pipeline_depth: 0,
                orphan_response: false,
                warnings: vec![warning.as_str().into()],
            })
        }
    }

    fn encode_messages(
        direction: Direction,
        messages: Vec<MessageV1>,
    ) -> Result<Vec<chronicle_protocol::DecodedFrame>, chronicle_protocol::ProtocolError> {
        messages
            .into_iter()
            .map(|message| {
                let sequence = message.sequence;
                let payload =
                    serde_json::to_vec(&message).map_err(|error| malformed(&error.to_string()))?;
                Ok(chronicle_protocol::DecodedFrame {
                    direction,
                    sequence,
                    payload,
                    attributes: Default::default(),
                })
            })
            .collect()
    }

    fn warning_for(error: &chronicle_protocol::ProtocolError) -> WarningCode {
        let message = error.to_string();
        if message.contains("transfer encoding") {
            WarningCode::UnsupportedTransferEncoding
        } else if message.contains("close-delimited") {
            WarningCode::UnsupportedCloseDelimitedBody
        } else if message.contains("informational") {
            WarningCode::UnsupportedInformationalResponse
        } else if message.contains("request target") || message.contains("CONNECT") {
            WarningCode::UnsupportedTarget
        } else if message.contains("upgrade") {
            WarningCode::UnsupportedUpgrade
        } else if message.contains("version") {
            WarningCode::UnsupportedVersion
        } else {
            WarningCode::Malformed
        }
    }

    fn parse_message(
        bytes: &[u8],
        direction: Direction,
        sequence: u64,
        head_response: bool,
    ) -> Result<Option<(usize, MessageV1)>, chronicle_protocol::ProtocolError> {
        match direction {
            Direction::ClientToServer => parse_request(bytes, sequence),
            Direction::ServerToClient => parse_response(bytes, sequence, head_response),
        }
    }

    fn parse_request(
        bytes: &[u8],
        sequence: u64,
    ) -> Result<Option<(usize, MessageV1)>, chronicle_protocol::ProtocolError> {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADER_COUNT];
        let mut request = httparse::Request::new(&mut headers);
        let head_bytes = match request
            .parse(bytes)
            .map_err(|error| malformed(&error.to_string()))?
        {
            httparse::Status::Partial => return partial_or_limit(bytes),
            httparse::Status::Complete(length) => length,
        };
        if request.version != Some(1) {
            return Err(malformed("unsupported HTTP request version"));
        }
        let method = request
            .method
            .ok_or_else(|| malformed("missing HTTP method"))?;
        if !is_token(method.as_bytes()) {
            return Err(malformed("invalid HTTP method"));
        }
        if method == "CONNECT" {
            return Err(malformed("unsupported CONNECT request target"));
        }
        let target = request
            .path
            .ok_or_else(|| malformed("missing HTTP request target"))?;
        if !target.starts_with('/') {
            return Err(malformed("unsupported HTTP request target"));
        }
        let headers = normalize_headers(request.headers)?;
        reject_unsupported_headers(&headers)?;
        let body_length = content_length(&headers)?;
        if body_length > MAX_RESPONSE_BYTES {
            return Err(malformed("request exceeds byte limit"));
        }
        let total = head_bytes.saturating_add(body_length);
        if bytes.len() < total {
            return Ok(None);
        }
        Ok(Some((
            total,
            MessageV1 {
                kind: MessageKindV1::Request,
                sequence,
                method: Some(method.into()),
                target: Some(target.into()),
                status: None,
                reason: None,
                headers,
                body: bytes[head_bytes..total].to_vec(),
                pipeline_depth: 0,
                orphan_response: false,
                warnings: Vec::new(),
            },
        )))
    }

    fn parse_response(
        bytes: &[u8],
        sequence: u64,
        head_response: bool,
    ) -> Result<Option<(usize, MessageV1)>, chronicle_protocol::ProtocolError> {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADER_COUNT];
        let mut response = httparse::Response::new(&mut headers);
        let head_bytes = match response
            .parse(bytes)
            .map_err(|error| malformed(&error.to_string()))?
        {
            httparse::Status::Partial => return partial_or_limit(bytes),
            httparse::Status::Complete(length) => length,
        };
        if response.version != Some(1) {
            return Err(malformed("unsupported HTTP response version"));
        }
        let status = response
            .code
            .ok_or_else(|| malformed("missing HTTP status"))?;
        if (100..200).contains(&status) {
            return Err(malformed("unsupported informational response"));
        }
        let headers = normalize_headers(response.headers)?;
        let chunked = headers
            .iter()
            .filter(|header| header.name == "transfer-encoding")
            .collect::<Vec<_>>();
        let (total, body) = if head_response || matches!(status, 204 | 304) {
            reject_unsupported_headers(&headers)?;
            (head_bytes, Vec::new())
        } else if !chunked.is_empty() {
            if chunked.len() != 1
                || !chunked[0]
                    .value
                    .trim_ascii()
                    .eq_ignore_ascii_case(b"chunked")
            {
                return Err(malformed("unsupported transfer encoding"));
            }
            reject_upgrade_headers(&headers)?;
            let Some((consumed, body)) = parse_chunked_body(&bytes[head_bytes..])? else {
                return Ok(None);
            };
            (head_bytes.saturating_add(consumed), body)
        } else if headers.iter().any(|header| header.name == "content-length") {
            reject_unsupported_headers(&headers)?;
            let body_length = content_length(&headers)?;
            if body_length > MAX_RESPONSE_BYTES {
                return Err(malformed("response exceeds byte limit"));
            }
            let total = head_bytes.saturating_add(body_length);
            if bytes.len() < total {
                return Ok(None);
            }
            (total, bytes[head_bytes..total].to_vec())
        } else {
            return Err(malformed("unsupported close-delimited response body"));
        };
        Ok(Some((
            total,
            MessageV1 {
                kind: MessageKindV1::Response,
                sequence,
                method: None,
                target: None,
                status: Some(status),
                reason: response.reason.map(|reason| reason.as_bytes().to_vec()),
                headers,
                body,
                pipeline_depth: 0,
                orphan_response: false,
                warnings: Vec::new(),
            },
        )))
    }

    fn parse_close_delimited_response(
        bytes: &[u8],
        sequence: u64,
        head_response: bool,
    ) -> Result<Option<(usize, MessageV1)>, chronicle_protocol::ProtocolError> {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADER_COUNT];
        let mut response = httparse::Response::new(&mut headers);
        let head_bytes = match response
            .parse(bytes)
            .map_err(|error| malformed(&error.to_string()))?
        {
            httparse::Status::Partial => return partial_or_limit(bytes),
            httparse::Status::Complete(length) => length,
        };
        if response.version != Some(1) {
            return Err(malformed("unsupported HTTP response version"));
        }
        let status = response
            .code
            .ok_or_else(|| malformed("missing HTTP status"))?;
        if (100..200).contains(&status) {
            return Err(malformed("unsupported informational response"));
        }
        let headers = normalize_headers(response.headers)?;
        reject_unsupported_headers(&headers)?;
        if head_response
            || matches!(status, 204 | 304)
            || headers.iter().any(|header| header.name == "content-length")
        {
            return parse_response(bytes, sequence, head_response);
        }
        Ok(Some((
            bytes.len(),
            MessageV1 {
                kind: MessageKindV1::Response,
                sequence,
                method: None,
                target: None,
                status: Some(status),
                reason: response.reason.map(|reason| reason.as_bytes().to_vec()),
                headers,
                body: bytes[head_bytes..].to_vec(),
                pipeline_depth: 0,
                orphan_response: false,
                warnings: Vec::new(),
            },
        )))
    }

    fn partial_or_limit(
        bytes: &[u8],
    ) -> Result<Option<(usize, MessageV1)>, chronicle_protocol::ProtocolError> {
        if bytes.len() >= MAX_HEAD_BYTES {
            Err(malformed("HTTP head exceeds decoder limit"))
        } else {
            Ok(None)
        }
    }

    fn normalize_headers(
        headers: &[httparse::Header<'_>],
    ) -> Result<Vec<HeaderV1>, chronicle_protocol::ProtocolError> {
        headers
            .iter()
            .map(|header| {
                if !header.name.is_ascii() {
                    return Err(malformed("HTTP header name is not ASCII"));
                }
                Ok(HeaderV1 {
                    name: header.name.to_ascii_lowercase(),
                    value: header.value.to_vec(),
                })
            })
            .collect()
    }

    fn reject_unsupported_headers(
        headers: &[HeaderV1],
    ) -> Result<(), chronicle_protocol::ProtocolError> {
        if headers
            .iter()
            .any(|header| header.name == "transfer-encoding")
        {
            return Err(malformed("unsupported transfer encoding"));
        }
        reject_upgrade_headers(headers)
    }

    fn reject_upgrade_headers(
        headers: &[HeaderV1],
    ) -> Result<(), chronicle_protocol::ProtocolError> {
        if headers.iter().any(|header| {
            header.name == "upgrade"
                || (header.name == "connection"
                    && header
                        .value
                        .split(|byte| *byte == b',')
                        .any(|value| value.trim_ascii().eq_ignore_ascii_case(b"upgrade")))
        }) {
            return Err(malformed("unsupported upgrade"));
        }
        Ok(())
    }

    fn parse_chunked_body(
        bytes: &[u8],
    ) -> Result<Option<(usize, Vec<u8>)>, chronicle_protocol::ProtocolError> {
        let mut cursor = 0;
        let mut body = Vec::new();
        loop {
            let Some(line_end) = bytes[cursor..].windows(2).position(|pair| pair == b"\r\n") else {
                return Ok(None);
            };
            let line_end = cursor + line_end;
            let size = std::str::from_utf8(
                bytes[cursor..line_end]
                    .split(|byte| *byte == b';')
                    .next()
                    .unwrap(),
            )
            .map_err(|_| malformed("invalid chunk size"))?;
            let size =
                usize::from_str_radix(size, 16).map_err(|_| malformed("invalid chunk size"))?;
            cursor = line_end + 2;
            if size == 0 {
                if bytes[cursor..].starts_with(b"\r\n") {
                    return Ok(Some((cursor + 2, body)));
                }
                let Some(trailer_end) = bytes[cursor..]
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                else {
                    return Ok(None);
                };
                return Ok(Some((cursor + trailer_end + 4, body)));
            }
            let end = cursor
                .checked_add(size)
                .ok_or_else(|| malformed("chunk exceeds byte limit"))?;
            if end.checked_add(2).is_none_or(|end| end > bytes.len()) {
                return Ok(None);
            }
            if &bytes[end..end + 2] != b"\r\n" {
                return Err(malformed("invalid chunk terminator"));
            }
            if body.len().saturating_add(size) > MAX_RESPONSE_BYTES {
                return Err(malformed("response exceeds byte limit"));
            }
            body.extend_from_slice(&bytes[cursor..end]);
            cursor = end + 2;
        }
    }

    fn content_length(headers: &[HeaderV1]) -> Result<usize, chronicle_protocol::ProtocolError> {
        let values = headers
            .iter()
            .filter(|header| header.name == "content-length")
            .collect::<Vec<_>>();
        if values.is_empty() {
            return Ok(0);
        }
        if values.len() != 1 {
            return Err(malformed("duplicate Content-Length"));
        }
        let value = std::str::from_utf8(&values[0].value)
            .map_err(|_| malformed("invalid Content-Length"))?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed("invalid Content-Length"));
        }
        value
            .parse()
            .map_err(|_| malformed("overflowing Content-Length"))
    }

    fn malformed(message: &str) -> chronicle_protocol::ProtocolError {
        chronicle_protocol::ProtocolError::Malformed {
            protocol: ProtocolId::new("http/1.1"),
            message: message.into(),
        }
    }

    pub fn registration() -> ProtocolRegistration {
        ProtocolRegistration {
            id: ProtocolId::new("http/1.1"),
            display_name: "HTTP/1.1",
            capabilities: ProtocolCapabilities {
                detection: CapabilityStatus::Available,
                decoding: CapabilityStatus::Available,
                canonicalization: CapabilityStatus::Available,
                replay: CapabilityStatus::Available,
                verification: CapabilityStatus::Available,
            },
            detector: Some(std::sync::Arc::new(Detector::new())),
            decoder_factory: Some(std::sync::Arc::new(DecoderFactory::new())),
            canonicalizer: Some(std::sync::Arc::new(Canonicalizer::new())),
            replay_adapter: Some(std::sync::Arc::new(HttpReplayAdapter::new())),
            verifier: Some(std::sync::Arc::new(HttpVerifier::new())),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use chronicle_protocol::{ProtocolStream, StreamChunk};

        fn stream(direction: Direction, bytes: &[u8]) -> ProtocolStream<'_> {
            ProtocolStream {
                started_at: None,
                chunks: vec![StreamChunk {
                    direction,
                    sequence: 1,
                    timestamp: None,
                    payload: bytes,
                }],
                truncated: false,
            }
        }

        fn verified_operation() -> CanonicalOperation {
            CanonicalOperation {
                id: OperationId::new(),
                sequence: 1,
                started_at_offset: RelativeTimeNanos(0),
                completed_at_offset: Some(RelativeTimeNanos(1)),
                kind: OperationKind::Request,
                effect: OperationEffect::Read,
                request: PayloadRef::Inline {
                    content_type: None,
                    bytes: Vec::new(),
                },
                recorded_response: Some(PayloadRef::Inline {
                    content_type: None,
                    bytes: b"expected-body".to_vec(),
                }),
                attributes: Attributes::new(),
                protocol_data: HttpOperationDataV1 {
                    method: Some("GET".into()),
                    target: Some("/".into()),
                    request_headers: Vec::new(),
                    response_headers: vec![
                        HeaderV1 {
                            name: "date".into(),
                            value: b"recorded-date".to_vec(),
                        },
                        HeaderV1 {
                            name: "authorization".into(),
                            value: b"recorded-secret".to_vec(),
                        },
                    ],
                    response_status: Some(200),
                    response_reason: Some(b"OK".to_vec()),
                    request_sequence: 1,
                    response_sequence: Some(2),
                    pipeline_depth: 1,
                    warnings: Vec::new(),
                    replay: ReplayAttributesV1 {
                        target_form: TargetFormV1::Origin,
                        captured_sensitive_headers: true,
                        replayable: true,
                    },
                    verification: VerificationMetadataV1 {
                        expected_status: Some(200),
                        expects_response: true,
                    },
                }
                .into_protocol_data(),
                incomplete: false,
                truncated: false,
                redactions: Vec::new(),
                warnings: Vec::new(),
            }
        }

        fn observed(status: u16, headers: Vec<HeaderV1>, body: &[u8]) -> ObservedResponse {
            ObservedResponse {
                payload: Some(PayloadRef::Inline {
                    content_type: None,
                    bytes: body.to_vec(),
                }),
                protocol_data: Some(HttpObservedResponseV1 { status, headers }.to_protocol_data()),
                attributes: BTreeMap::new(),
                error_category: None,
            }
        }

        #[test]
        fn verifier_compares_status_headers_and_body_without_values() {
            let operation = verified_operation();
            let verifier = HttpVerifier::new();
            let passed = observed(
                200,
                vec![
                    HeaderV1 {
                        name: "date".into(),
                        value: b"new-date".to_vec(),
                    },
                    HeaderV1 {
                        name: "authorization".into(),
                        value: b"recorded-secret".to_vec(),
                    },
                ],
                b"expected-body",
            );
            assert_eq!(
                verifier.verify(&operation, &passed).status,
                VerificationStatus::Passed
            );

            let status = verifier.verify(&operation, &observed(201, Vec::new(), b"expected-body"));
            assert_eq!(status.status, VerificationStatus::Failed);
            assert_eq!(status.details["expected_status"], "200");

            let headers = verifier.verify(
                &operation,
                &observed(
                    200,
                    vec![HeaderV1 {
                        name: "authorization".into(),
                        value: b"changed-secret".to_vec(),
                    }],
                    b"expected-body",
                ),
            );
            assert_eq!(headers.status, VerificationStatus::Failed);
            assert_eq!(headers.details["header"], "authorization");
            assert!(!format!("{headers:?}").contains("secret"));

            let body = verifier.verify(
                &operation,
                &observed(
                    200,
                    vec![HeaderV1 {
                        name: "authorization".into(),
                        value: b"recorded-secret".to_vec(),
                    }],
                    b"changed-body",
                ),
            );
            assert_eq!(body.status, VerificationStatus::Failed);
            assert!(body.details.contains_key("expected_body_sha256"));
            assert!(!format!("{body:?}").contains("changed-body"));
        }

        #[test]
        fn http_operation_data_round_trips_binary_duplicate_headers() {
            let data = HttpOperationDataV1 {
                method: Some("GET".into()),
                target: Some("/bytes".into()),
                request_headers: vec![
                    HeaderV1 {
                        name: "x-test".into(),
                        value: vec![0, 0xff],
                    },
                    HeaderV1 {
                        name: "x-test".into(),
                        value: b"second".to_vec(),
                    },
                ],
                response_headers: vec![HeaderV1 {
                    name: "content-type".into(),
                    value: b"application/octet-stream".to_vec(),
                }],
                response_status: Some(200),
                response_reason: Some(b"OK".to_vec()),
                request_sequence: 4,
                response_sequence: Some(5),
                pipeline_depth: 2,
                warnings: vec![WarningCode::Pipelined.as_str().into()],
                replay: ReplayAttributesV1 {
                    target_form: TargetFormV1::Origin,
                    captured_sensitive_headers: false,
                    replayable: false,
                },
                verification: VerificationMetadataV1 {
                    expected_status: Some(200),
                    expects_response: true,
                },
            };
            assert_eq!(
                HttpOperationDataV1::from_protocol_data(&data.into_protocol_data()).unwrap(),
                data
            );
        }

        #[tokio::test]
        async fn replay_reader_finishes_at_fixed_length_boundary_without_eof() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut peer, _) = listener.accept().await.unwrap();
                peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            });
            let mut client = TcpStream::connect(address).await.unwrap();
            let observed =
                tokio::time::timeout(Duration::from_millis(50), read_response(&mut client, "GET"))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                observed.payload,
                Some(PayloadRef::Inline {
                    content_type: None,
                    bytes: b"OK".to_vec(),
                })
            );
        }

        #[tokio::test]
        async fn replay_reader_rejects_informational_and_upgrade_responses() {
            for response in [
                b"HTTP/1.1 100 Continue\r\nContent-Length: 0\r\n\r\n".as_slice(),
                b"HTTP/1.1 200 OK\r\nUpgrade: websocket\r\nContent-Length: 0\r\n\r\n".as_slice(),
            ] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let response = response.to_vec();
                tokio::spawn(async move {
                    let (mut peer, _) = listener.accept().await.unwrap();
                    peer.write_all(&response).await.unwrap();
                });
                let mut client = TcpStream::connect(address).await.unwrap();
                let error = read_response(&mut client, "GET").await.unwrap_err();
                assert!(matches!(
                    error,
                    ProtocolError::Transport {
                        category: TransportErrorCategory::UnsupportedFraming,
                        ..
                    }
                ));
            }
        }

        #[tokio::test]
        async fn replay_adapter_requires_explicit_target_authorization() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let endpoint = Endpoint::new("127.0.0.1", address.port());
            let adapter = HttpReplayAdapter::new();
            assert!(
                adapter
                    .connect(&endpoint, &ReplayContext::default())
                    .await
                    .is_err()
            );

            let mut context = ReplayContext::default();
            context.authorize_execution_for(endpoint.clone());
            assert!(adapter.connect(&endpoint, &context).await.is_ok());
        }

        #[test]
        fn replay_target_address_requires_loopback_and_supports_ipv6() {
            assert_eq!(
                target_address(&Endpoint::new("::1", 8080)).unwrap(),
                "[::1]:8080".parse::<std::net::SocketAddr>().unwrap()
            );
            assert!(target_address(&Endpoint::new("127.0.0.1", 8080)).is_ok());
            assert!(target_address(&Endpoint::new("192.0.2.1", 8080)).is_err());
            assert!(target_address(&Endpoint::new("localhost", 8080)).is_err());
        }

        #[test]
        fn sanitizer_replaces_host_and_strips_sensitive_hop_by_hop_headers() {
            let headers = sanitize_request_headers(
                &[
                    HeaderV1 {
                        name: "host".into(),
                        value: b"recorded.invalid".to_vec(),
                    },
                    HeaderV1 {
                        name: "host".into(),
                        value: b"duplicate.invalid".to_vec(),
                    },
                    HeaderV1 {
                        name: "connection".into(),
                        value: b"x-remove, Keep-Alive".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-remove".into(),
                        value: b"gone".to_vec(),
                    },
                    HeaderV1 {
                        name: "authorization".into(),
                        value: b"captured-secret".to_vec(),
                    },
                    HeaderV1 {
                        name: "cookie".into(),
                        value: b"captured-cookie".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-forwarded-for".into(),
                        value: b"198.51.100.10".to_vec(),
                    },
                    HeaderV1 {
                        name: "transfer-encoding".into(),
                        value: b"chunked".to_vec(),
                    },
                    HeaderV1 {
                        name: "content-length".into(),
                        value: b"99".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-tag".into(),
                        value: b"one".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-tag".into(),
                        value: b"two".to_vec(),
                    },
                ],
                &chronicle_common::Endpoint::new("127.0.0.1", 8080),
                3,
            );

            assert_eq!(
                headers,
                vec![
                    HeaderV1 {
                        name: "host".into(),
                        value: b"127.0.0.1:8080".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-tag".into(),
                        value: b"one".to_vec(),
                    },
                    HeaderV1 {
                        name: "x-tag".into(),
                        value: b"two".to_vec(),
                    },
                    HeaderV1 {
                        name: "content-length".into(),
                        value: b"3".to_vec(),
                    },
                ]
            );
        }

        #[test]
        fn sanitizer_brackets_ipv6_target_authority() {
            let headers =
                sanitize_request_headers(&[], &chronicle_common::Endpoint::new("::1", 8080), 0);
            assert_eq!(headers[0].value, b"[::1]:8080");
            assert_eq!(headers[1].value, b"0");
        }

        #[test]
        fn detects_request_and_response_prefixes() {
            let detector = Detector::new();
            assert!(matches!(
                detector.detect(DetectionInput {
                    stream: &stream(Direction::ClientToServer, b"GET / HTTP/1.1\r\n"),
                    override_protocol: None
                }),
                DetectionResult::Confirmed { .. }
            ));
            assert!(matches!(
                detector.detect(DetectionInput {
                    stream: &stream(Direction::ServerToClient, b"HTTP/1.1 200 OK\r\n"),
                    override_protocol: None
                }),
                DetectionResult::Probable { .. }
            ));
        }

        #[test]
        fn reports_need_more_and_rejects_tls_and_http2() {
            let detector = Detector::new();
            assert!(matches!(
                detector.detect(DetectionInput {
                    stream: &stream(Direction::ClientToServer, b"GET / HT"),
                    override_protocol: None
                }),
                DetectionResult::NeedMoreData { .. }
            ));
            for bytes in [
                b"\x16\x03\x01".as_slice(),
                b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".as_slice(),
            ] {
                assert!(matches!(
                    detector.detect(DetectionInput {
                        stream: &stream(Direction::ClientToServer, bytes),
                        override_protocol: None
                    }),
                    DetectionResult::Rejected
                ));
            }
        }

        #[test]
        fn canonicalizer_maps_http_exchange_and_missing_response() {
            let canonicalizer = Canonicalizer::new();
            let request = MessageV1 {
                kind: MessageKindV1::Request,
                sequence: 1,
                method: Some("GET".into()),
                target: Some("/items".into()),
                status: None,
                reason: None,
                headers: vec![
                    HeaderV1 {
                        name: "x-duplicate".into(),
                        value: vec![1],
                    },
                    HeaderV1 {
                        name: "x-duplicate".into(),
                        value: vec![2],
                    },
                ],
                body: b"request".to_vec(),
                pipeline_depth: 1,
                orphan_response: false,
                warnings: Vec::new(),
            };
            let response = MessageV1 {
                kind: MessageKindV1::Response,
                sequence: 2,
                method: None,
                target: None,
                status: Some(201),
                reason: Some(b"Created".to_vec()),
                headers: Vec::new(),
                body: b"response".to_vec(),
                pipeline_depth: 1,
                orphan_response: false,
                warnings: Vec::new(),
            };
            let missing = MessageV1 {
                sequence: 3,
                method: Some("TRACE".into()),
                target: Some("/missing".into()),
                ..request.clone()
            };
            let frames = vec![
                DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: serde_json::to_vec(&request).unwrap(),
                    attributes: Attributes::new(),
                },
                DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 2,
                    payload: serde_json::to_vec(&response).unwrap(),
                    attributes: Attributes::new(),
                },
                DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 3,
                    payload: serde_json::to_vec(&missing).unwrap(),
                    attributes: Attributes::new(),
                },
            ];
            let operations = canonicalizer
                .canonicalize(&stream(Direction::ClientToServer, b""), frames)
                .unwrap();
            assert_eq!(operations.len(), 2);
            assert_eq!(operations[0].effect, OperationEffect::Read);
            assert!(matches!(
                operations[0].recorded_response.as_ref(),
                Some(PayloadRef::Inline { bytes, .. }) if bytes == b"response"
            ));
            let data =
                HttpOperationDataV1::from_protocol_data(&operations[0].protocol_data).unwrap();
            assert_eq!(data.response_status, Some(201));
            assert_eq!(data.request_headers.len(), 2);
            assert_eq!(operations[1].effect, OperationEffect::Unknown);
            assert!(operations[1].incomplete);
        }

        #[test]
        fn canonicalizer_preserves_orphan_response_as_incomplete_evidence() {
            let response = MessageV1 {
                kind: MessageKindV1::Response,
                sequence: 1,
                method: None,
                target: None,
                status: Some(200),
                reason: Some(b"OK".to_vec()),
                headers: vec![HeaderV1 {
                    name: "x-test".into(),
                    value: b"value".to_vec(),
                }],
                body: b"response".to_vec(),
                pipeline_depth: 0,
                orphan_response: true,
                warnings: Vec::new(),
            };
            let operations = Canonicalizer::new()
                .canonicalize(
                    &stream(Direction::ServerToClient, b""),
                    vec![DecodedFrame {
                        direction: Direction::ServerToClient,
                        sequence: 1,
                        payload: serde_json::to_vec(&response).unwrap(),
                        attributes: Attributes::new(),
                    }],
                )
                .unwrap();
            assert_eq!(operations.len(), 1);
            assert!(operations[0].incomplete);
            assert!(
                operations[0]
                    .warnings
                    .iter()
                    .any(|warning| warning.code == "orphan_response")
            );
            assert!(matches!(
                operations[0].recorded_response.as_ref(),
                Some(PayloadRef::Inline { bytes, .. }) if bytes == b"response"
            ));
        }

        #[test]
        fn decoder_uses_each_queued_method_for_coalesced_responses() {
            use chronicle_protocol::ProtocolDecoder;

            let mut decoder = Decoder::new();
            for (sequence, payload) in [
                (1, b"HEAD /head HTTP/1.1\r\n\r\n".as_slice()),
                (2, b"GET /get HTTP/1.1\r\n\r\n".as_slice()),
            ] {
                decoder
                    .push(DecodedFrame {
                        direction: Direction::ClientToServer,
                        sequence,
                        payload: payload.to_vec(),
                        attributes: Attributes::new(),
                    })
                    .unwrap();
            }
            let responses = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 3,
                    payload: b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
                    attributes: Attributes::new(),
                })
                .unwrap();
            assert_eq!(responses.len(), 2);
            let second: MessageV1 = serde_json::from_slice(&responses[1].payload).unwrap();
            assert_eq!(second.body, b"OK");
        }

        #[test]
        fn decoder_handles_fragmented_fixed_length_request_and_response() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            assert!(
                decoder
                    .push(DecodedFrame {
                        direction: Direction::ClientToServer,
                        sequence: 1,
                        payload: b"POST /bin HTTP/1.1\r\nContent-Length: 3\r\n".to_vec(),
                        attributes: Default::default(),
                    })
                    .unwrap()
                    .is_empty()
            );
            let request = decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 2,
                    payload: b"\r\n\x00\xff\x80".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let request: MessageV1 = serde_json::from_slice(&request[0].payload).unwrap();
            assert_eq!(request.method.as_deref(), Some("POST"));
            assert_eq!(request.body, [0, 255, 128]);

            let response = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 3,
                    payload: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let response: MessageV1 = serde_json::from_slice(&response[0].payload).unwrap();
            assert_eq!(response.status, Some(200));
            assert_eq!(response.body, b"OK");
        }

        #[test]
        fn close_delimited_response_requires_trusted_clean_source() {
            use chronicle_protocol::{
                DerivedTermination, LossWindowClassification, ReconstructionFinalization,
            };

            assert!(trusted_close(
                DerivedTermination::CleanClose,
                None,
                [LossWindowClassification::Outside]
            ));
            assert!(!trusted_close(
                DerivedTermination::UnknownTermination,
                None,
                [LossWindowClassification::Outside]
            ));
            assert!(!trusted_close(
                DerivedTermination::Reset,
                None,
                [LossWindowClassification::Outside]
            ));
            assert!(!trusted_close(
                DerivedTermination::CleanClose,
                Some(ReconstructionFinalization::Idle),
                [LossWindowClassification::Outside]
            ));
            assert!(!trusted_close(
                DerivedTermination::CleanClose,
                None,
                [LossWindowClassification::Overlaps]
            ));
        }

        #[test]
        fn close_delimited_response_requires_final_input_and_consumes_remaining_bytes() {
            assert!(parse_response(b"HTTP/1.1 200 OK\r\n\r\nbody", 1, false).is_err());
            let parsed = parse_close_delimited_response(b"HTTP/1.1 200 OK\r\n\r\nbody", 1, false)
                .unwrap()
                .unwrap();
            assert_eq!(parsed.0, b"HTTP/1.1 200 OK\r\n\r\nbody".len());
            assert_eq!(parsed.1.body, b"body");
        }

        #[test]
        fn chunked_response_decodes_binary_body_across_fragments_and_consumes_trailer() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"GET / HTTP/1.1\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            assert!(
                decoder
                    .push(DecodedFrame {
                        direction: Direction::ServerToClient,
                        sequence: 2,
                        payload: b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n\x00"
                            .to_vec(),
                        attributes: Default::default(),
                    })
                    .unwrap()
                    .is_empty()
            );
            let response = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 3,
                    payload: b"\xff\r\n0\r\nX-Trailer: ok\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let response: MessageV1 = serde_json::from_slice(&response[0].payload).unwrap();
            assert_eq!(response.body, [0, 255]);
            assert_eq!(parse_chunked_body(b"1\r\na\r\n0\r\n").unwrap(), None);
            assert!(parse_chunked_body(b"1\r\naX\r\n0\r\n\r\n").is_err());
        }

        #[test]
        fn decoder_consumes_zero_and_no_body_responses_without_waiting_for_body() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            let request = decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"POST /zero HTTP/1.1\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let request: MessageV1 = serde_json::from_slice(&request[0].payload).unwrap();
            assert_eq!(request.body, b"");

            for (method, status) in [("HEAD", 200), ("GET", 204), ("GET", 304)] {
                let mut decoder = Decoder::new();
                decoder
                    .push(DecodedFrame {
                        direction: Direction::ClientToServer,
                        sequence: 1,
                        payload: format!("{method} / HTTP/1.1\r\n\r\n").into_bytes(),
                        attributes: Default::default(),
                    })
                    .unwrap();
                let response = decoder
                    .push(DecodedFrame {
                        direction: Direction::ServerToClient,
                        sequence: 2,
                        payload: format!("HTTP/1.1 {status} OK\r\nContent-Length: 3\r\n\r\n")
                            .into_bytes(),
                        attributes: Default::default(),
                    })
                    .unwrap();
                let response: MessageV1 = serde_json::from_slice(&response[0].payload).unwrap();
                assert_eq!(response.status, Some(status));
                assert_eq!(response.body, b"");
            }
        }

        #[test]
        fn content_length_accepts_only_one_unsigned_decimal_field() {
            let header = |value: &[u8]| HeaderV1 {
                name: "content-length".into(),
                value: value.to_vec(),
            };
            assert_eq!(content_length(&[header(b"12")]).unwrap(), 12);
            assert!(content_length(&[header(b"1"), header(b"1")]).is_err());
            for value in [
                b"1, 1".as_slice(),
                b"+1".as_slice(),
                b"-1".as_slice(),
                b"".as_slice(),
                b"not-a-number".as_slice(),
                b"999999999999999999999999999999999999999".as_slice(),
            ] {
                assert!(content_length(&[header(value)]).is_err(), "{value:?}");
            }
        }

        #[test]
        fn decoder_preserves_malformed_message_as_opaque() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let payload = b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\na";
            let frames = Decoder::new()
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: payload.to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let message: MessageV1 = serde_json::from_slice(&frames[0].payload).unwrap();
            assert_eq!(message.kind, MessageKindV1::Opaque);
            assert_eq!(message.body, payload);
            assert_eq!(message.warnings, [WarningCode::Malformed.as_str()]);
        }

        #[test]
        fn decoder_uses_stable_opaque_warnings() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            for (payload, warning) in [
                (
                    b"GET / HTTP/1.0\r\n\r\n".as_slice(),
                    WarningCode::UnsupportedVersion,
                ),
                (
                    b"CONNECT host:80 HTTP/1.1\r\n\r\n".as_slice(),
                    WarningCode::UnsupportedTarget,
                ),
                (
                    b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
                    WarningCode::UnsupportedTransferEncoding,
                ),
                (
                    b"GET / HTTP/1.1\r\nConnection: upgrade\r\n\r\n".as_slice(),
                    WarningCode::UnsupportedUpgrade,
                ),
                (
                    b"HTTP/1.1 101 Switching Protocols\r\nContent-Length: 0\r\n\r\n".as_slice(),
                    WarningCode::UnsupportedInformationalResponse,
                ),
                (
                    b"HTTP/1.1 200 OK\r\n\r\nbody".as_slice(),
                    WarningCode::UnsupportedCloseDelimitedBody,
                ),
            ] {
                let direction = if payload.starts_with(b"HTTP/") {
                    Direction::ServerToClient
                } else {
                    Direction::ClientToServer
                };
                let frames = Decoder::new()
                    .push(DecodedFrame {
                        direction,
                        sequence: 1,
                        payload: payload.to_vec(),
                        attributes: Default::default(),
                    })
                    .unwrap();
                let message: MessageV1 = serde_json::from_slice(&frames[0].payload).unwrap();
                assert_eq!(message.body, payload);
                assert_eq!(message.warnings, [warning.as_str()]);
            }

            let mut decoder = Decoder::new();
            decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"GET / HTTP/1.1\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let frames = decoder.finish().unwrap();
            let message: MessageV1 = serde_json::from_slice(&frames[0].payload).unwrap();
            assert_eq!(message.warnings, [WarningCode::TruncatedMessage.as_str()]);

            let mut over_limit = b"GET / HTTP/1.1\r\n".to_vec();
            over_limit.resize(MAX_HEAD_BYTES + 1, b'x');
            let frames = Decoder::new()
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: over_limit.clone(),
                    attributes: Default::default(),
                })
                .unwrap();
            let message: MessageV1 = serde_json::from_slice(&frames[0].payload).unwrap();
            assert_eq!(message.body, over_limit);
            assert_eq!(message.warnings, [WarningCode::Malformed.as_str()]);

            let oversized = format!(
                "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                MAX_RESPONSE_BYTES + 1
            );
            assert!(parse_request(oversized.as_bytes(), 1).is_err());
            let oversized = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_RESPONSE_BYTES + 1
            );
            assert!(parse_response(oversized.as_bytes(), 1, false).is_err());
        }

        #[test]
        fn decoder_keeps_completed_exchange_before_later_opaque_data() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            let complete = decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"GET /ok HTTP/1.1\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<MessageV1>(&complete[0].payload)
                    .unwrap()
                    .kind,
                MessageKindV1::Request
            );
            let opaque = decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 2,
                    payload: b"GET / HTTP/1.0\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<MessageV1>(&opaque[0].payload)
                    .unwrap()
                    .kind,
                MessageKindV1::Opaque
            );
        }

        #[test]
        fn decoder_uses_pending_head_request_for_response_body_rule() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            decoder
                .push(DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"HEAD / HTTP/1.1\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let response = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 2,
                    payload: b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let response: MessageV1 = serde_json::from_slice(&response[0].payload).unwrap();
            assert!(response.body.is_empty());
        }

        #[test]
        fn decoder_tracks_pipelining_missing_and_orphan_responses() {
            use chronicle_protocol::{DecodedFrame, ProtocolDecoder};

            let mut decoder = Decoder::new();
            for (sequence, payload) in [
                (1, b"GET /one HTTP/1.1\r\n\r\n".as_slice()),
                (2, b"GET /two HTTP/1.1\r\n\r\n".as_slice()),
            ] {
                decoder
                    .push(DecodedFrame {
                        direction: Direction::ClientToServer,
                        sequence,
                        payload: payload.to_vec(),
                        attributes: Default::default(),
                    })
                    .unwrap();
            }
            assert_eq!(decoder.pending_methods.len(), 2);
            let responses = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 3,
                    payload: b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let messages = responses
                .iter()
                .map(|frame| serde_json::from_slice::<MessageV1>(&frame.payload).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(messages[0].pipeline_depth, 2);
            assert_eq!(messages[1].pipeline_depth, 1);
            assert!(decoder.pending_methods.is_empty());

            let orphan = decoder
                .push(DecodedFrame {
                    direction: Direction::ServerToClient,
                    sequence: 4,
                    payload: b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    attributes: Default::default(),
                })
                .unwrap();
            let orphan: MessageV1 = serde_json::from_slice(&orphan[0].payload).unwrap();
            assert!(orphan.orphan_response);
        }
    }
}

pub mod postgres {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("postgres", "PostgreSQL", PLANNED)
    }
}

pub mod mysql_family {
    //! Shared MySQL/MariaDB framing boundary.
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mysql-family", "MySQL family", PLANNED)
    }
}

pub mod mysql {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mysql", "MySQL", PLANNED)
    }
}

pub mod mariadb {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mariadb", "MariaDB", PLANNED)
    }
}

pub mod oracle {
    //! Oracle Net/TNS semantics remain research-only; opaque preservation is required.
    use super::{
        CapabilityStatus, ProtocolCapabilities, ProtocolRegistration, scaffold_registration,
    };
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration(
            "oracle",
            "Oracle Net",
            ProtocolCapabilities {
                detection: CapabilityStatus::Research,
                decoding: CapabilityStatus::Research,
                canonicalization: CapabilityStatus::Planned,
                replay: CapabilityStatus::Research,
                verification: CapabilityStatus::Research,
            },
        )
    }
}

pub mod mongodb {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mongodb", "MongoDB", PLANNED)
    }
}

pub mod kafka {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("kafka", "Kafka", PLANNED)
    }
}

pub mod nats {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("nats", "NATS", PLANNED)
    }
}

pub mod fake {
    use chronicle_canonical::{
        Attributes, CanonicalOperation, CanonicalWarning, OperationEffect, OperationKind,
        PayloadRef, ProtocolData, RelativeTimeNanos,
    };
    use chronicle_common::{Direction, Endpoint, OperationId, ProtocolId};
    use chronicle_protocol::{
        BoxFuture, CapabilityStatus, DecodedFrame, DecoderFactory, DetectionInput, DetectionResult,
        ObservedResponse, ProtocolCanonicalizer, ProtocolCapabilities, ProtocolDecoder,
        ProtocolDetector, ProtocolError, ProtocolRegistration, ProtocolStream, ReplayAdapter,
        ReplayConnection, ReplayContext, VerificationResult, VerificationStatus, Verifier,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct FakeDetector {
        id: ProtocolId,
    }

    impl ProtocolDetector for FakeDetector {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn detect(&self, input: DetectionInput<'_>) -> DetectionResult {
            if input
                .stream
                .chunks
                .iter()
                .any(|chunk| chunk.payload.starts_with(b"FAKE "))
            {
                DetectionResult::Confirmed {
                    confidence: 100,
                    evidence: "FAKE prefix".into(),
                }
            } else {
                DetectionResult::Unknown
            }
        }
    }

    struct FakeDecoderFactory {
        id: ProtocolId,
    }

    impl DecoderFactory for FakeDecoderFactory {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn create(&self) -> Box<dyn ProtocolDecoder> {
            Box::new(FakeDecoder {
                id: self.id.clone(),
            })
        }
    }

    struct FakeDecoder {
        id: ProtocolId,
    }

    impl ProtocolDecoder for FakeDecoder {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn push(&mut self, frame: DecodedFrame) -> Result<Vec<DecodedFrame>, ProtocolError> {
            Ok(vec![frame])
        }

        fn finish(&mut self) -> Result<Vec<DecodedFrame>, ProtocolError> {
            Ok(Vec::new())
        }
    }

    struct FakeCanonicalizer {
        id: ProtocolId,
    }

    impl FakeCanonicalizer {
        fn offset(
            stream: &ProtocolStream<'_>,
            sequence: u64,
        ) -> (RelativeTimeNanos, Vec<CanonicalWarning>) {
            let timestamp = stream
                .chunks
                .iter()
                .find(|chunk| chunk.sequence == sequence)
                .and_then(|chunk| chunk.timestamp);
            match (stream.started_at, timestamp) {
                (Some(started_at), Some(timestamp)) => (
                    RelativeTimeNanos(
                        u64::try_from((timestamp - started_at).whole_nanoseconds().max(0))
                            .unwrap_or(u64::MAX),
                    ),
                    Vec::new(),
                ),
                _ => (
                    RelativeTimeNanos(0),
                    vec![CanonicalWarning {
                        code: "missing_timestamp".into(),
                        message: "operation timestamp unavailable; offset set to zero".into(),
                    }],
                ),
            }
        }

        fn operation(
            stream: &ProtocolStream<'_>,
            request: DecodedFrame,
            response: Option<DecodedFrame>,
            truncated: bool,
        ) -> CanonicalOperation {
            let incomplete = response.is_none();
            let (started_at_offset, warnings) = Self::offset(stream, request.sequence);
            let completed_at_offset = response
                .as_ref()
                .map(|frame| Self::offset(stream, frame.sequence).0);
            CanonicalOperation {
                id: OperationId::new(),
                sequence: request.sequence,
                started_at_offset,
                completed_at_offset,
                kind: OperationKind::Request,
                effect: OperationEffect::Read,
                request: PayloadRef::Inline {
                    content_type: Some("application/x-chronicle-fake".into()),
                    bytes: request.payload.clone(),
                },
                recorded_response: response.map(|frame| PayloadRef::Inline {
                    content_type: Some("application/x-chronicle-fake".into()),
                    bytes: frame.payload,
                }),
                attributes: Attributes::new(),
                protocol_data: ProtocolData {
                    schema_version: 1,
                    media_type: Some("application/x-chronicle-fake".into()),
                    bytes: request.payload,
                },
                incomplete,
                truncated,
                redactions: Vec::new(),
                warnings,
            }
        }
    }

    impl ProtocolCanonicalizer for FakeCanonicalizer {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn canonicalize(
            &self,
            stream: &ProtocolStream<'_>,
            frames: Vec<DecodedFrame>,
        ) -> Result<Vec<CanonicalOperation>, ProtocolError> {
            let mut operations = Vec::new();
            let mut pending = None;
            for frame in frames {
                match frame.direction {
                    Direction::ClientToServer => {
                        if let Some(request) = pending.replace(frame) {
                            operations.push(Self::operation(
                                stream,
                                request,
                                None,
                                stream.truncated,
                            ));
                        }
                    }
                    Direction::ServerToClient => {
                        if let Some(request) = pending.take() {
                            operations.push(Self::operation(
                                stream,
                                request,
                                Some(frame),
                                stream.truncated,
                            ));
                        }
                    }
                }
            }
            if let Some(request) = pending {
                operations.push(Self::operation(stream, request, None, stream.truncated));
            }
            Ok(operations)
        }
    }

    struct FakeReplayAdapter {
        id: ProtocolId,
    }

    impl ReplayAdapter for FakeReplayAdapter {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn connect<'a>(
            &'a self,
            _target: &'a Endpoint,
            _context: &'a ReplayContext,
        ) -> BoxFuture<'a, Result<Box<dyn ReplayConnection>, ProtocolError>> {
            Box::pin(async { Ok(Box::new(FakeReplayConnection) as Box<dyn ReplayConnection>) })
        }
    }

    struct FakeReplayConnection;

    impl ReplayConnection for FakeReplayConnection {
        fn execute<'a>(
            &'a mut self,
            operation: &'a CanonicalOperation,
        ) -> BoxFuture<'a, Result<ObservedResponse, ProtocolError>> {
            Box::pin(async move {
                Ok(ObservedResponse {
                    payload: operation.recorded_response.clone(),
                    protocol_data: None,
                    attributes: BTreeMap::new(),
                    error_category: None,
                })
            })
        }
    }

    struct FakeVerifier {
        id: ProtocolId,
    }

    impl Verifier for FakeVerifier {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn verify(
            &self,
            operation: &CanonicalOperation,
            observed: &ObservedResponse,
        ) -> VerificationResult {
            let passed = operation.recorded_response == observed.payload;
            VerificationResult {
                status: if passed {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
                summary: if passed {
                    "fake response matched".into()
                } else {
                    "fake response differed".into()
                },
                details: BTreeMap::new(),
            }
        }
    }

    pub fn registration() -> ProtocolRegistration {
        let id = ProtocolId::new("fake");
        ProtocolRegistration {
            id: id.clone(),
            display_name: "Chronicle fake protocol",
            capabilities: ProtocolCapabilities {
                detection: CapabilityStatus::Available,
                decoding: CapabilityStatus::Available,
                canonicalization: CapabilityStatus::Available,
                replay: CapabilityStatus::Available,
                verification: CapabilityStatus::Available,
            },
            detector: Some(Arc::new(FakeDetector { id: id.clone() })),
            decoder_factory: Some(Arc::new(FakeDecoderFactory { id: id.clone() })),
            canonicalizer: Some(Arc::new(FakeCanonicalizer { id: id.clone() })),
            replay_adapter: Some(Arc::new(FakeReplayAdapter { id: id.clone() })),
            verifier: Some(Arc::new(FakeVerifier { id })),
        }
    }
}

pub fn registry() -> Result<ProtocolRegistry, ProtocolError> {
    let mut registry = ProtocolRegistry::new();
    for registration in [
        fake::registration(),
        http::registration(),
        postgres::registration(),
        mysql_family::registration(),
        mysql::registration(),
        mariadb::registration(),
        oracle::registration(),
        mongodb::registration(),
        kafka::registration(),
        nats::registration(),
    ] {
        registry.register(registration)?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_common::Direction;
    use chronicle_protocol::{ProtocolStream, StreamChunk};

    fn stream(payload: &[u8]) -> ProtocolStream<'_> {
        ProtocolStream {
            started_at: None,
            chunks: vec![StreamChunk {
                direction: Direction::ClientToServer,
                sequence: 1,
                timestamp: None,
                payload,
            }],
            truncated: false,
        }
    }

    #[test]
    fn fake_detects_and_unknown_remains_unknown() {
        let registry = registry().unwrap();
        let (registration, _) = registry.detect(&stream(b"FAKE request"), None).unwrap();
        assert_eq!(registration.id.as_str(), "fake");
        assert!(registry.detect(&stream(b"opaque"), None).is_none());
    }

    #[test]
    fn fake_canonicalizer_warns_when_timestamp_is_missing() {
        use chronicle_protocol::DecodedFrame;

        let registry = registry().unwrap();
        let registration = registry.get(&ProtocolId::new("fake")).unwrap();
        let operations = registration
            .canonicalizer
            .as_ref()
            .unwrap()
            .canonicalize(
                &stream(b"FAKE request"),
                vec![DecodedFrame {
                    direction: Direction::ClientToServer,
                    sequence: 1,
                    payload: b"FAKE request".to_vec(),
                    attributes: Default::default(),
                }],
            )
            .unwrap();
        assert_eq!(
            operations[0].started_at_offset,
            chronicle_canonical::RelativeTimeNanos(0)
        );
        assert_eq!(operations[0].warnings[0].code, "missing_timestamp");
    }

    #[test]
    fn http_registers_all_completed_capabilities() {
        let registry = registry().unwrap();
        let registration = registry.get(&ProtocolId::new("http/1.1")).unwrap();
        assert_eq!(
            registration.capabilities,
            ProtocolCapabilities {
                detection: CapabilityStatus::Available,
                decoding: CapabilityStatus::Available,
                canonicalization: CapabilityStatus::Available,
                replay: CapabilityStatus::Available,
                verification: CapabilityStatus::Available,
            }
        );
    }

    #[test]
    fn registrations_only_advertise_implemented_capabilities() {
        let registry = registry().unwrap();
        for registration in registry.registrations() {
            assert_eq!(
                registration.capabilities.detection == CapabilityStatus::Available,
                registration.detector.is_some()
            );
            assert_eq!(
                registration.capabilities.decoding == CapabilityStatus::Available,
                registration.decoder_factory.is_some()
            );
            assert_eq!(
                registration.capabilities.canonicalization == CapabilityStatus::Available,
                registration.canonicalizer.is_some()
            );
            assert_eq!(
                registration.capabilities.replay == CapabilityStatus::Available,
                registration.replay_adapter.is_some()
            );
            assert_eq!(
                registration.capabilities.verification == CapabilityStatus::Available,
                registration.verifier.is_some()
            );
        }
    }
}
