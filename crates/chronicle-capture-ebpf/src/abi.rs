use crate::error::EbpfCaptureError;

const ABI_MAGIC: [u8; 4] = *b"CHRN";
const ABI_VERSION: u16 = 1;
const ABI_HEADER_BYTES: usize = 12;
const LITTLE_ENDIAN: u8 = 1;

const KIND_CONNECT4: u8 = 1;
const KIND_CONNECT6: u8 = 2;
const KIND_SOCKOPS: u8 = 3;
const KIND_INGRESS: u8 = 4;
const KIND_EGRESS: u8 = 5;
const KIND_LOSS_COUNTERS: u8 = 6;

const SIGNAL_ACTIVE_ESTABLISHED: u8 = 1;
const SIGNAL_CLOSE: u8 = 2;
const SIGNAL_RESET: u8 = 3;
const SIGNAL_AMBIGUOUS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawNetworkFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawDirection {
    Ingress,
    Egress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawLifecycleSignal {
    ActiveEstablished,
    Close,
    Reset,
    Ambiguous(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawSocketObservation {
    pub timestamp_ns: u64,
    pub socket_cookie: u64,
    pub first_seen_ns: u64,
    pub network_namespace: Option<u64>,
    pub cgroup_id: u64,
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawConnectObservation {
    pub socket: RawSocketObservation,
    pub family: RawNetworkFamily,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawLifecycleObservation {
    pub socket: RawSocketObservation,
    pub signal: RawLifecycleSignal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawPayloadObservation {
    pub socket: RawSocketObservation,
    pub family: RawNetworkFamily,
    pub direction: RawDirection,
    pub tcp_sequence: u32,
    pub continuation_position: u32,
    pub observed_length: Option<u32>,
    pub truncated: bool,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawLossCounters {
    pub timestamp_ns: u64,
    pub generation: Option<u64>,
    pub per_cpu: Vec<u64>,
}

/// Decoded private kernel evidence. Never exported outside this adapter crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RawKernelObservation {
    Connect(RawConnectObservation),
    Lifecycle(RawLifecycleObservation),
    Payload(RawPayloadObservation),
    LossCounters(RawLossCounters),
}

pub(crate) fn decode_raw_kernel_observation(
    bytes: &[u8],
) -> Result<RawKernelObservation, EbpfCaptureError> {
    if bytes.len() < ABI_HEADER_BYTES {
        return Err(decode_error("header", "item is shorter than ABI header"));
    }
    if bytes[..4] != ABI_MAGIC {
        return Err(decode_error("header", "invalid ABI magic"));
    }
    if read_u16(bytes, 4, "header")? != ABI_VERSION {
        return Err(decode_error("header", "unsupported ABI version"));
    }
    if bytes[7] != LITTLE_ENDIAN {
        return Err(decode_error("header", "unsupported ABI byte order"));
    }
    let declared_size = usize::try_from(read_u32(bytes, 8, "header")?)
        .map_err(|_| decode_error("header", "declared size does not fit usize"))?;
    if declared_size != bytes.len() {
        return Err(decode_error(
            "header",
            "declared size differs from item size",
        ));
    }

    let kind = bytes[6];
    let body = &bytes[ABI_HEADER_BYTES..];
    match kind {
        KIND_CONNECT4 | KIND_CONNECT6 => {
            if body.len() != 48 {
                return Err(decode_error("connect", "invalid connect item size"));
            }
            Ok(RawKernelObservation::Connect(RawConnectObservation {
                socket: decode_socket(body, "connect")?,
                family: if kind == KIND_CONNECT4 {
                    RawNetworkFamily::Ipv4
                } else {
                    RawNetworkFamily::Ipv6
                },
            }))
        }
        KIND_SOCKOPS => decode_lifecycle(body),
        KIND_INGRESS | KIND_EGRESS => decode_payload(body, kind),
        KIND_LOSS_COUNTERS => decode_loss_counters(body),
        _ => Err(decode_error("header", "unknown ABI discriminant")),
    }
}

fn decode_socket(
    bytes: &[u8],
    source: &'static str,
) -> Result<RawSocketObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    if bytes.len() < SOCKET_BYTES {
        return Err(decode_error(source, "item is shorter than socket evidence"));
    }
    let socket_cookie = read_u64(bytes, 8, source)?;
    let first_seen_ns = read_u64(bytes, 16, source)?;
    if socket_cookie == 0 {
        return Err(EbpfCaptureError::MissingIdentity("socket cookie"));
    }
    if first_seen_ns == 0 {
        return Err(EbpfCaptureError::MissingIdentity(
            "first-seen monotonic timestamp",
        ));
    }
    let network_namespace = read_u64(bytes, 24, source)?;
    Ok(RawSocketObservation {
        timestamp_ns: read_u64(bytes, 0, source)?,
        socket_cookie,
        first_seen_ns,
        network_namespace: (network_namespace != 0).then_some(network_namespace),
        cgroup_id: read_u64(bytes, 32, source)?,
        pid: read_u32(bytes, 40, source)?,
        tid: read_u32(bytes, 44, source)?,
    })
}

fn decode_lifecycle(bytes: &[u8]) -> Result<RawKernelObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    if bytes.len() != SOCKET_BYTES + 5 {
        return Err(decode_error("sockops", "invalid lifecycle item size"));
    }
    let signal = match bytes[SOCKET_BYTES] {
        SIGNAL_ACTIVE_ESTABLISHED => RawLifecycleSignal::ActiveEstablished,
        SIGNAL_CLOSE => RawLifecycleSignal::Close,
        SIGNAL_RESET => RawLifecycleSignal::Reset,
        SIGNAL_AMBIGUOUS => {
            RawLifecycleSignal::Ambiguous(read_u32(bytes, SOCKET_BYTES + 1, "sockops")?)
        }
        _ => return Err(decode_error("sockops", "unknown lifecycle signal")),
    };
    Ok(RawKernelObservation::Lifecycle(RawLifecycleObservation {
        socket: decode_socket(bytes, "sockops")?,
        signal,
    }))
}

fn decode_payload(bytes: &[u8], kind: u8) -> Result<RawKernelObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    const PAYLOAD_HEADER_BYTES: usize = 18;
    if bytes.len() < SOCKET_BYTES + PAYLOAD_HEADER_BYTES {
        return Err(decode_error(
            "payload",
            "item is shorter than payload header",
        ));
    }
    let family = match bytes[SOCKET_BYTES] {
        4 => RawNetworkFamily::Ipv4,
        6 => RawNetworkFamily::Ipv6,
        _ => return Err(decode_error("payload", "unknown network family")),
    };
    let captured_length = usize::try_from(read_u32(bytes, SOCKET_BYTES + 13, "payload")?)
        .map_err(|_| decode_error("payload", "captured length does not fit usize"))?;
    if bytes.len() != SOCKET_BYTES + PAYLOAD_HEADER_BYTES + captured_length {
        return Err(EbpfCaptureError::InvalidPayload {
            context: "payload",
            reason: "captured length exceeds ABI item bounds",
        });
    }
    let observed_length = read_u32(bytes, SOCKET_BYTES + 9, "payload")?;
    Ok(RawKernelObservation::Payload(RawPayloadObservation {
        socket: decode_socket(bytes, "payload")?,
        family,
        direction: if kind == KIND_INGRESS {
            RawDirection::Ingress
        } else {
            RawDirection::Egress
        },
        tcp_sequence: read_u32(bytes, SOCKET_BYTES + 1, "payload")?,
        continuation_position: read_u32(bytes, SOCKET_BYTES + 5, "payload")?,
        observed_length: (observed_length != 0).then_some(observed_length),
        truncated: bytes[SOCKET_BYTES + 17] != 0,
        payload: bytes[SOCKET_BYTES + PAYLOAD_HEADER_BYTES..].to_vec(),
    }))
}

fn decode_loss_counters(bytes: &[u8]) -> Result<RawKernelObservation, EbpfCaptureError> {
    if bytes.len() < 12 || !(bytes.len() - 12).is_multiple_of(8) {
        return Err(decode_error("loss_counters", "invalid counter item size"));
    }
    let timestamp_ns = read_u64(bytes, 0, "loss_counters")?;
    let generation = read_u32(bytes, 8, "loss_counters")?;
    let per_cpu = bytes[12..]
        .chunks_exact(8)
        .enumerate()
        .map(|(index, _)| read_u64(bytes, 12 + index * 8, "loss_counters"))
        .collect::<Result<Vec<_>, _>>()?;
    if per_cpu.is_empty() {
        return Err(EbpfCaptureError::RingLoss(
            "loss counters have no CPU values",
        ));
    }
    Ok(RawKernelObservation::LossCounters(RawLossCounters {
        timestamp_ns,
        generation: (generation != 0).then_some(u64::from(generation)),
        per_cpu,
    }))
}

fn read_u16(bytes: &[u8], offset: usize, source: &'static str) -> Result<u16, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u16::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u16"))?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize, source: &'static str) -> Result<u32, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u32::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u32"))?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize, source: &'static str) -> Result<u64, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u64::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u64"))?,
    ))
}

const fn decode_error(source: &'static str, reason: &'static str) -> EbpfCaptureError {
    EbpfCaptureError::Decode {
        context: source,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket_body() -> Vec<u8> {
        let mut body = vec![0; 48];
        body[0..8].copy_from_slice(&10_u64.to_le_bytes());
        body[8..16].copy_from_slice(&20_u64.to_le_bytes());
        body[16..24].copy_from_slice(&5_u64.to_le_bytes());
        body[32..40].copy_from_slice(&30_u64.to_le_bytes());
        body[40..44].copy_from_slice(&40_u32.to_le_bytes());
        body[44..48].copy_from_slice(&41_u32.to_le_bytes());
        body
    }

    fn abi_item(kind: u8, body: Vec<u8>) -> Vec<u8> {
        let mut item = Vec::with_capacity(ABI_HEADER_BYTES + body.len());
        item.extend_from_slice(&ABI_MAGIC);
        item.extend_from_slice(&ABI_VERSION.to_le_bytes());
        item.push(kind);
        item.push(LITTLE_ENDIAN);
        item.extend_from_slice(
            &u32::try_from(ABI_HEADER_BYTES + body.len())
                .unwrap()
                .to_le_bytes(),
        );
        item.extend(body);
        item
    }

    #[test]
    fn ipv4_and_ipv6_connect_items_decode_without_architecture_input() {
        for (kind, expected_family) in [
            (KIND_CONNECT4, RawNetworkFamily::Ipv4),
            (KIND_CONNECT6, RawNetworkFamily::Ipv6),
        ] {
            let observation =
                decode_raw_kernel_observation(&abi_item(kind, socket_body())).unwrap();
            assert!(matches!(
                observation,
                RawKernelObservation::Connect(RawConnectObservation {
                    family,
                    socket: RawSocketObservation {
                        socket_cookie: 20,
                        first_seen_ns: 5,
                        ..
                    },
                }) if family == expected_family
            ));
        }
    }

    #[test]
    fn exact_item_and_connect_sizes_are_enforced() {
        let mut wrong_declared_size = abi_item(KIND_CONNECT4, socket_body());
        wrong_declared_size[8] = 1;
        let error = decode_raw_kernel_observation(&wrong_declared_size).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("declared size differs from item size")
        );

        for invalid_body in [
            {
                let mut body = socket_body();
                body.pop();
                body
            },
            {
                let mut body = socket_body();
                body.push(0);
                body
            },
        ] {
            let error =
                decode_raw_kernel_observation(&abi_item(KIND_CONNECT6, invalid_body)).unwrap_err();
            assert!(error.to_string().contains("invalid connect item size"));
        }
    }

    #[test]
    fn invalid_magic_version_kind_and_endianness_are_rejected() {
        let item = abi_item(KIND_CONNECT4, socket_body());
        for (offset, replacement, expected) in [
            (0, b'X', "invalid ABI magic"),
            (4, 2, "unsupported ABI version"),
            (6, 99, "unknown ABI discriminant"),
            (7, 2, "unsupported ABI byte order"),
        ] {
            let mut invalid = item.clone();
            invalid[offset] = replacement;
            let error = decode_raw_kernel_observation(&invalid).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn invalid_family_and_lifecycle_signal_are_rejected() {
        let mut payload = socket_body();
        payload.extend_from_slice(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let error = decode_raw_kernel_observation(&abi_item(KIND_INGRESS, payload)).unwrap_err();
        assert!(error.to_string().contains("unknown network family"));

        let mut lifecycle = socket_body();
        lifecycle.extend_from_slice(&[99, 0, 0, 0, 0]);
        let error = decode_raw_kernel_observation(&abi_item(KIND_SOCKOPS, lifecycle)).unwrap_err();
        assert!(error.to_string().contains("unknown lifecycle signal"));
    }

    #[test]
    fn malformed_payload_metadata_is_not_silently_dropped() {
        let mut body = socket_body();
        body.push(4);
        body.extend_from_slice(&100_u32.to_le_bytes());
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&8_u32.to_le_bytes());
        body.extend_from_slice(&8_u32.to_le_bytes());
        body.push(0);
        let error = decode_raw_kernel_observation(&abi_item(KIND_EGRESS, body)).unwrap_err();
        assert!(matches!(
            error,
            EbpfCaptureError::InvalidPayload {
                context: "payload",
                ..
            }
        ));
    }
}
