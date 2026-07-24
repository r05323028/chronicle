//! Versioned append-only WAL framing and segmented writer.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const WAL_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 28;
pub const DEFAULT_MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAGIC: [u8; 4] = *b"CHWL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordKind {
    CaptureEvent = 1,
}

impl TryFrom<u16> for RecordKind {
    type Error = WalError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::CaptureEvent),
            other => Err(WalError::UnknownRecordKind(other)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalRecord {
    pub kind: RecordKind,
    pub flags: u16,
    pub sequence: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalCheckpoint {
    pub segment_first_sequence: u64,
    pub byte_offset: u64,
    pub next_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Record(WalRecord),
    End,
    PartialTail { offset: u64 },
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("WAL I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid WAL magic at byte offset {offset}")]
    InvalidMagic { offset: u64 },
    #[error("unsupported WAL version {0}")]
    UnsupportedVersion(u16),
    #[error("unknown WAL record kind {0}")]
    UnknownRecordKind(u16),
    #[error("WAL record payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("WAL record is {record_bytes} bytes; configured maximum is {max_record_bytes}")]
    RecordTooLarge {
        record_bytes: usize,
        max_record_bytes: usize,
    },
    #[error(
        "WAL checksum mismatch at sequence {sequence}: expected {expected:#010x}, got {actual:#010x}"
    )]
    Checksum {
        sequence: u64,
        expected: u32,
        actual: u32,
    },
    #[error("WAL sequence {current} does not follow {previous}")]
    OutOfOrder { previous: u64, current: u64 },
    #[error("WAL sequence space exhausted at {sequence}")]
    SequenceExhausted { sequence: u64 },
    #[error("WAL sequence {actual} does not match expected sequence {expected}")]
    UnexpectedSequence { expected: u64, actual: u64 },
    #[error("segment size must exceed WAL header size")]
    InvalidSegmentSize,
    #[error("WAL writer has no active segment")]
    NoActiveSegment,
}

pub fn encode_record(record: &WalRecord) -> Result<Vec<u8>, WalError> {
    let payload_len = u32::try_from(record.payload.len())
        .map_err(|_| WalError::PayloadTooLarge(record.payload.len()))?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + record.payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&WAL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(record.kind as u16).to_le_bytes());
    bytes.extend_from_slice(&record.flags.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(&record.sequence.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&record.payload);

    let checksum = record_checksum(&bytes[4..24], &record.payload);
    bytes[24..28].copy_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

fn record_checksum(fixed_header: &[u8], payload: &[u8]) -> u32 {
    let mut checksum_input = Vec::with_capacity(fixed_header.len() + payload.len());
    checksum_input.extend_from_slice(fixed_header);
    checksum_input.extend_from_slice(payload);
    crc32c::crc32c(&checksum_input)
}

pub struct WalReader<R> {
    reader: R,
    offset: u64,
    segment_first_sequence: u64,
    next_sequence: u64,
    max_record_bytes: usize,
}

impl<R: Read> WalReader<R> {
    pub fn new(reader: R, segment_first_sequence: u64) -> Self {
        Self::with_max_record_bytes(reader, segment_first_sequence, DEFAULT_MAX_RECORD_BYTES)
    }

    pub fn with_max_record_bytes(
        reader: R,
        segment_first_sequence: u64,
        max_record_bytes: usize,
    ) -> Self {
        Self {
            reader,
            offset: 0,
            segment_first_sequence,
            next_sequence: segment_first_sequence,
            max_record_bytes,
        }
    }

    pub fn checkpoint(&self) -> WalCheckpoint {
        WalCheckpoint {
            segment_first_sequence: self.segment_first_sequence,
            byte_offset: self.offset,
            next_sequence: self.next_sequence,
        }
    }

    pub fn next_record(&mut self) -> Result<ReadOutcome, WalError> {
        let record_offset = self.offset;
        let mut header = [0_u8; HEADER_LEN];
        let header_read = read_up_to(&mut self.reader, &mut header)?;
        if header_read == 0 {
            return Ok(ReadOutcome::End);
        }
        if header_read < HEADER_LEN {
            return Ok(ReadOutcome::PartialTail {
                offset: record_offset,
            });
        }
        if header[..4] != MAGIC {
            return Err(WalError::InvalidMagic {
                offset: record_offset,
            });
        }

        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != WAL_VERSION {
            return Err(WalError::UnsupportedVersion(version));
        }
        let kind = RecordKind::try_from(u16::from_le_bytes([header[6], header[7]]))?;
        let flags = u16::from_le_bytes([header[8], header[9]]);
        let payload_len = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        let sequence = u64::from_le_bytes([
            header[16], header[17], header[18], header[19], header[20], header[21], header[22],
            header[23],
        ]);
        if sequence != self.next_sequence {
            return Err(WalError::UnexpectedSequence {
                expected: self.next_sequence,
                actual: sequence,
            });
        }
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted { sequence })?;
        let expected = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
        let payload_len = usize::try_from(payload_len).map_err(|_| WalError::RecordTooLarge {
            record_bytes: usize::MAX,
            max_record_bytes: self.max_record_bytes,
        })?;
        let record_bytes = HEADER_LEN.saturating_add(payload_len);
        if record_bytes > self.max_record_bytes {
            return Err(WalError::RecordTooLarge {
                record_bytes,
                max_record_bytes: self.max_record_bytes,
            });
        }
        let mut payload = vec![0_u8; payload_len];
        if read_up_to(&mut self.reader, &mut payload)? < payload.len() {
            return Ok(ReadOutcome::PartialTail {
                offset: record_offset,
            });
        }
        let actual = record_checksum(&header[4..24], &payload);
        if actual != expected {
            return Err(WalError::Checksum {
                sequence,
                expected,
                actual,
            });
        }
        self.offset += u64::try_from(record_bytes).unwrap_or(u64::MAX);
        self.next_sequence = next_sequence;
        Ok(ReadOutcome::Record(WalRecord {
            kind,
            flags,
            sequence,
            payload,
        }))
    }
}

fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buffer.len() {
        match reader.read(&mut buffer[total..]) {
            Ok(0) => break,
            Ok(count) => total += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

pub trait WalWriter {
    fn append(&mut self, record: &WalRecord) -> Result<WalCheckpoint, WalError>;
    fn flush(&mut self) -> Result<(), WalError>;
}

pub struct SegmentedWalWriter {
    directory: PathBuf,
    max_segment_bytes: u64,
    file: Option<File>,
    segment_first_sequence: u64,
    segment_bytes: u64,
    last_sequence: Option<u64>,
}

impl SegmentedWalWriter {
    pub fn create(directory: impl AsRef<Path>, max_segment_bytes: u64) -> Result<Self, WalError> {
        if max_segment_bytes <= HEADER_LEN as u64 {
            return Err(WalError::InvalidSegmentSize);
        }
        fs::create_dir_all(directory.as_ref())?;
        #[cfg(unix)]
        fs::set_permissions(directory.as_ref(), fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            directory: directory.as_ref().to_path_buf(),
            max_segment_bytes,
            file: None,
            segment_first_sequence: 0,
            segment_bytes: 0,
            last_sequence: None,
        })
    }

    fn rotate(&mut self, first_sequence: u64) -> Result<(), WalError> {
        if let Some(file) = self.file.as_mut() {
            file.sync_data()?;
        }
        let path = self.directory.join(segment_file_name(first_sequence));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path)?;
        self.file = Some(file);
        self.segment_first_sequence = first_sequence;
        self.segment_bytes = 0;
        Ok(())
    }
}

impl WalWriter for SegmentedWalWriter {
    fn append(&mut self, record: &WalRecord) -> Result<WalCheckpoint, WalError> {
        let next_sequence = record
            .sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted {
                sequence: record.sequence,
            })?;
        if let Some(previous) = self.last_sequence {
            let expected = previous
                .checked_add(1)
                .ok_or(WalError::SequenceExhausted { sequence: previous })?;
            if record.sequence != expected {
                return Err(WalError::OutOfOrder {
                    previous,
                    current: record.sequence,
                });
            }
        }
        let encoded = encode_record(record)?;
        let encoded_len = u64::try_from(encoded.len())
            .map_err(|_| WalError::PayloadTooLarge(record.payload.len()))?;
        if self.file.is_none()
            || (self.segment_bytes > 0
                && self.segment_bytes.saturating_add(encoded_len) > self.max_segment_bytes)
        {
            self.rotate(record.sequence)?;
        }
        self.file
            .as_mut()
            .ok_or(WalError::NoActiveSegment)?
            .write_all(&encoded)?;
        self.segment_bytes += encoded_len;
        self.last_sequence = Some(record.sequence);
        Ok(WalCheckpoint {
            segment_first_sequence: self.segment_first_sequence,
            byte_offset: self.segment_bytes,
            next_sequence,
        })
    }

    fn flush(&mut self) -> Result<(), WalError> {
        if let Some(file) = self.file.as_mut() {
            file.sync_data()?;
        }
        Ok(())
    }
}

pub fn segment_file_name(first_sequence: u64) -> String {
    format!("segment-{first_sequence:020}.wal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn record() -> WalRecord {
        WalRecord {
            kind: RecordKind::CaptureEvent,
            flags: 2,
            sequence: 7,
            payload: b"payload".to_vec(),
        }
    }

    #[test]
    fn record_round_trip() {
        let encoded = encode_record(&record()).unwrap();
        let mut reader = WalReader::new(Cursor::new(encoded), 7);
        assert_eq!(reader.next_record().unwrap(), ReadOutcome::Record(record()));
        assert_eq!(reader.next_record().unwrap(), ReadOutcome::End);
    }

    #[test]
    fn detects_corruption() {
        let mut encoded = encode_record(&record()).unwrap();
        *encoded.last_mut().unwrap() ^= 0xff;
        let error = WalReader::new(Cursor::new(encoded), 7)
            .next_record()
            .unwrap_err();
        assert!(matches!(error, WalError::Checksum { sequence: 7, .. }));
    }

    #[test]
    fn reports_partial_tail_without_advancing_checkpoint() {
        let mut encoded = encode_record(&record()).unwrap();
        encoded.pop();
        let mut reader = WalReader::new(Cursor::new(encoded), 7);
        assert_eq!(
            reader.next_record().unwrap(),
            ReadOutcome::PartialTail { offset: 0 }
        );
        assert_eq!(reader.checkpoint().byte_offset, 0);
    }

    #[test]
    fn rejects_record_larger_than_reader_limit_before_allocation() {
        let encoded = encode_record(&record()).unwrap();
        let mut reader = WalReader::with_max_record_bytes(Cursor::new(encoded), 7, HEADER_LEN + 6);
        let error = reader.next_record().unwrap_err();
        assert!(matches!(
            error,
            WalError::RecordTooLarge {
                record_bytes,
                max_record_bytes
            } if record_bytes == HEADER_LEN + 7 && max_record_bytes == HEADER_LEN + 6
        ));
    }

    #[test]
    fn reader_rejects_unexpected_sequence() {
        let error = WalReader::new(Cursor::new(encode_record(&record()).unwrap()), 6)
            .next_record()
            .unwrap_err();
        assert!(matches!(
            error,
            WalError::UnexpectedSequence {
                expected: 6,
                actual: 7
            }
        ));
    }

    #[test]
    fn rejects_exhausted_sequence_space() {
        let exhausted = WalRecord {
            sequence: u64::MAX,
            ..record()
        };
        let reader_error =
            WalReader::new(Cursor::new(encode_record(&exhausted).unwrap()), u64::MAX)
                .next_record()
                .unwrap_err();
        assert!(matches!(
            reader_error,
            WalError::SequenceExhausted { sequence: u64::MAX }
        ));

        let mut writer = SegmentedWalWriter {
            directory: PathBuf::new(),
            max_segment_bytes: 1_024,
            file: None,
            segment_first_sequence: 0,
            segment_bytes: 0,
            last_sequence: None,
        };
        assert!(matches!(
            writer.append(&exhausted),
            Err(WalError::SequenceExhausted { sequence: u64::MAX })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn writer_hardens_directory_and_segment_permissions() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "chronicle-wal-permissions-{}-{unique}",
            std::process::id()
        ));
        let mut writer = SegmentedWalWriter::create(&directory, 1024).unwrap();
        writer.append(&record()).unwrap();
        writer.flush().unwrap();

        let directory_mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
        let segment_mode = fs::metadata(directory.join(segment_file_name(7)))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(segment_mode, 0o600);
        fs::remove_dir_all(directory).unwrap();
    }
}
