//! Statically linked protocol registrations.
//!
//! Only `fake` is functional. Real protocol modules declare honest target status.

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
    use chronicle_common::{Direction, OperationId, ProtocolId};
    use chronicle_protocol::{
        CapabilityStatus, DecodedFrame, DetectionInput, DetectionResult, ProtocolCanonicalizer,
        ProtocolCapabilities, ProtocolDetector, ProtocolError, ProtocolStream,
    };
    use std::collections::VecDeque;

    pub const MAX_HEAD_BYTES: usize = 64 * 1024;
    pub const MAX_HEADER_COUNT: usize = 128;
    pub const PROTOCOL_DATA_SCHEMA_VERSION: u16 = 1;
    pub const PROTOCOL_DATA_MEDIA_TYPE: &str =
        "application/vnd.chronicle.http-operation+json;version=1";

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
                    captured_sensitive_headers: request
                        .headers
                        .iter()
                        .any(|header| matches!(header.name.as_str(), "authorization" | "cookie")),
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
                    let head_response = self
                        .pending_methods
                        .front()
                        .is_some_and(|method| method == "HEAD");
                    self.server.decode(Direction::ServerToClient, head_response)
                }
            };
            match frame.direction {
                Direction::ClientToServer => {
                    for message in &mut messages {
                        if let Some(method) = &message.method {
                            self.pending_methods.push_back(method.clone());
                            message.pipeline_depth = self.pending_methods.len();
                        }
                    }
                }
                Direction::ServerToClient => {
                    for message in &mut messages {
                        if message.kind == MessageKindV1::Response {
                            message.pipeline_depth = self.pending_methods.len();
                            message.orphan_response = self.pending_methods.pop_front().is_none();
                        }
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
            loop {
                let sequence = self.start_sequence.unwrap_or_default();
                match parse_message(&self.bytes, direction, sequence, head_response) {
                    Ok(Some((consumed, mut message))) => {
                        message.sequence = sequence;
                        self.bytes.drain(..consumed);
                        self.start_sequence = (!self.bytes.is_empty()).then_some(sequence);
                        messages.push(message);
                    }
                    Ok(None) => break,
                    Err(error) => {
                        if let Some(message) = self.take_opaque(warning_for(&error)) {
                            messages.push(message);
                        }
                        break;
                    }
                }
            }
            messages
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
        reject_unsupported_headers(&headers)?;
        let body_length = if head_response || matches!(status, 204 | 304) {
            0
        } else if headers.iter().any(|header| header.name == "content-length") {
            content_length(&headers)?
        } else {
            return Err(malformed("unsupported close-delimited response body"));
        };
        let total = head_bytes.saturating_add(body_length);
        if bytes.len() < total {
            return Ok(None);
        }
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
                body: bytes[head_bytes..total].to_vec(),
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
                replay: CapabilityStatus::Planned,
                verification: CapabilityStatus::Planned,
            },
            detector: Some(std::sync::Arc::new(Detector::new())),
            decoder_factory: Some(std::sync::Arc::new(DecoderFactory::new())),
            canonicalizer: Some(std::sync::Arc::new(Canonicalizer::new())),
            replay_adapter: None,
            verifier: None,
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
