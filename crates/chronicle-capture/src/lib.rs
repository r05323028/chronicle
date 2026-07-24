//! Protocol-neutral capture boundary. Events model ordered socket byte chunks, not packets.

use chronicle_common::{ConnectionKey, Direction, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;

pub const CAPTURE_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMetadata {
    pub pid: u32,
    pub tid: Option<u32>,
    pub executable: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerMetadata {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFlags(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub schema_version: u16,
    pub monotonic_sequence: u64,
    pub wall_time: Option<Timestamp>,
    pub connection: ConnectionKey,
    pub direction: Direction,
    pub payload: Vec<u8>,
    pub process: Option<ProcessMetadata>,
    pub container: Option<ContainerMetadata>,
    pub file_descriptor: Option<i32>,
    pub truncated: bool,
    pub flags: CaptureFlags,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture source failed: {0}")]
    Source(String),
    #[error("capture event encoding failed: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("unsupported capture event schema {0}")]
    UnsupportedSchema(u16),
}

pub trait CaptureSource: Send {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError>;
}

#[derive(Default)]
pub struct InMemoryCaptureSource {
    events: VecDeque<CaptureEvent>,
}

impl InMemoryCaptureSource {
    pub fn new(events: impl IntoIterator<Item = CaptureEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
        }
    }
}

impl CaptureSource for InMemoryCaptureSource {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        Ok(self.events.pop_front())
    }
}

pub fn encode_event(event: &CaptureEvent) -> Result<Vec<u8>, CaptureError> {
    if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
        return Err(CaptureError::UnsupportedSchema(event.schema_version));
    }
    Ok(serde_json::to_vec(event)?)
}

pub fn decode_event(bytes: &[u8]) -> Result<CaptureEvent, CaptureError> {
    let event: CaptureEvent = serde_json::from_slice(bytes)?;
    if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
        return Err(CaptureError::UnsupportedSchema(event.schema_version));
    }
    Ok(event)
}
