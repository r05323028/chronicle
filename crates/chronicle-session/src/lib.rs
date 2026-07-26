//! Bounded reconstruction of ordered bidirectional socket byte streams.

use chronicle_capture::{CaptureEvent, CaptureEventV1};
use chronicle_common::ConnectionKey;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct SessionLimits {
    pub max_connections: usize,
    pub max_bytes_per_connection: usize,
    pub max_chunks_per_connection: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_connections: 1_024,
            max_bytes_per_connection: 8 * 1024 * 1024,
            max_chunks_per_connection: 65_536,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectionStream {
    pub key: ConnectionKey,
    pub chunks: Vec<CaptureEventV1>,
    pub total_bytes: usize,
    pub truncated: bool,
}

impl ConnectionStream {
    pub fn first_sequence(&self) -> Option<u64> {
        self.chunks.first().map(|event| event.monotonic_sequence)
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("connection limit {limit} exceeded")]
    ConnectionLimit { limit: usize },
    #[error("connection byte limit {limit} exceeded for {attempted} bytes")]
    ByteLimit { limit: usize, attempted: usize },
    #[error("capture event schema {schema_version} cannot enter session assembly")]
    UnsupportedCaptureEvent { schema_version: u16 },
    #[error("connection chunk limit {limit} exceeded")]
    ChunkLimit { limit: usize },
}

pub struct SessionAssembler {
    limits: SessionLimits,
    streams: BTreeMap<ConnectionKey, ConnectionStream>,
}

impl SessionAssembler {
    pub fn new(limits: SessionLimits) -> Self {
        Self {
            limits,
            streams: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, event: CaptureEvent) -> Result<(), SessionError> {
        let event = match event {
            CaptureEvent::V1(event) => event,
            event => {
                return Err(SessionError::UnsupportedCaptureEvent {
                    schema_version: event.schema_version(),
                });
            }
        };
        let existing_bytes = self
            .streams
            .get(&event.connection)
            .map_or(0, |stream| stream.total_bytes);
        let attempted = existing_bytes.saturating_add(event.payload.len());
        if attempted > self.limits.max_bytes_per_connection {
            return Err(SessionError::ByteLimit {
                limit: self.limits.max_bytes_per_connection,
                attempted,
            });
        }
        let existing_chunks = self
            .streams
            .get(&event.connection)
            .map_or(0, |stream| stream.chunks.len());
        if existing_chunks >= self.limits.max_chunks_per_connection {
            return Err(SessionError::ChunkLimit {
                limit: self.limits.max_chunks_per_connection,
            });
        }
        if !self.streams.contains_key(&event.connection)
            && self.streams.len() >= self.limits.max_connections
        {
            return Err(SessionError::ConnectionLimit {
                limit: self.limits.max_connections,
            });
        }
        let stream = self
            .streams
            .entry(event.connection.clone())
            .or_insert_with(|| ConnectionStream {
                key: event.connection.clone(),
                chunks: Vec::new(),
                total_bytes: 0,
                truncated: false,
            });
        stream.total_bytes = attempted;
        stream.truncated |= event.truncated;
        stream.chunks.push(event);
        Ok(())
    }

    pub fn finish(self) -> Vec<ConnectionStream> {
        let mut streams: Vec<_> = self.streams.into_values().collect();
        for stream in &mut streams {
            stream.chunks.sort_by_key(|event| event.monotonic_sequence);
        }
        streams.sort_by_key(ConnectionStream::first_sequence);
        streams
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_capture::{CAPTURE_EVENT_SCHEMA_VERSION, CaptureEventV1, CaptureFlags};
    use chronicle_common::{Direction, Endpoint, TransportProtocol};

    fn event(sequence: u64) -> CaptureEvent {
        event_for(sequence, "client", 1)
    }

    fn event_for(sequence: u64, client: &str, payload_bytes: usize) -> CaptureEvent {
        CaptureEvent::V1(CaptureEventV1 {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: sequence,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new(client, 10),
                Endpoint::new("server", 20),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: vec![u8::try_from(sequence).unwrap(); payload_bytes],
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        })
    }

    #[test]
    fn orders_chunks_by_monotonic_sequence() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        assembler.push(event(2)).unwrap();
        assembler.push(event(1)).unwrap();
        let stream = assembler.finish().pop().unwrap();
        let sequences: Vec<_> = stream
            .chunks
            .iter()
            .map(|event| event.monotonic_sequence)
            .collect();
        assert_eq!(sequences, [1, 2]);
    }

    #[test]
    fn rejected_oversized_new_event_does_not_consume_connection_capacity() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 1,
        };
        let mut assembler = SessionAssembler::new(limits);
        assert!(matches!(
            assembler.push(event_for(1, "oversized", 2)),
            Err(SessionError::ByteLimit { .. })
        ));
        assembler.push(event_for(2, "accepted", 1)).unwrap();
        let streams = assembler.finish();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].key.client.host, "accepted");
    }

    #[test]
    fn enforces_connection_and_existing_stream_byte_limits() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 2,
        };
        let mut assembler = SessionAssembler::new(limits);
        assembler.push(event_for(1, "first", 1)).unwrap();
        assert!(matches!(
            assembler.push(event_for(2, "first", 1)),
            Err(SessionError::ByteLimit { .. })
        ));
        assert!(matches!(
            assembler.push(event_for(3, "second", 1)),
            Err(SessionError::ConnectionLimit { limit: 1 })
        ));
    }

    #[test]
    fn limits_zero_byte_chunks() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 1,
        };
        let mut assembler = SessionAssembler::new(limits);
        assembler.push(event_for(1, "first", 0)).unwrap();
        assert!(matches!(
            assembler.push(event_for(2, "first", 0)),
            Err(SessionError::ChunkLimit { limit: 1 })
        ));
    }

    #[test]
    fn preserves_direction_order_and_truncation() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        let mut response = event_for(2, "client", 1);
        if let CaptureEvent::V1(event) = &mut response {
            event.direction = Direction::ServerToClient;
            event.truncated = true;
        }
        assembler.push(response).unwrap();
        assembler.push(event_for(1, "client", 1)).unwrap();

        let stream = assembler.finish().pop().unwrap();
        assert_eq!(
            stream
                .chunks
                .iter()
                .map(|chunk| chunk.monotonic_sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(stream.chunks[1].direction, Direction::ServerToClient);
        assert!(stream.truncated);
    }
}
