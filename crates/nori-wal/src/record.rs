//! WAL record format with varint encoding and CRC32C checksumming.
//!
//! Record format:
//! - klen: varint
//! - vlen: varint
//! - flags: u8 (bits: 0=tombstone, 1=ttl_present, 2-3=compression, 4-7=reserved)
//! - ttl_ms?: varint (if ttl_present bit set)
//! - key: bytes[klen]
//! - value: bytes[vlen]
//! - crc32c: u32 (little-endian)

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io::{self, ErrorKind};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecordError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("CRC mismatch: expected {expected:#x}, got {actual:#x}")]
    CrcMismatch { expected: u32, actual: u32 },
    #[error("Invalid compression type: {0}")]
    InvalidCompression(u8),
    #[error("Incomplete record")]
    Incomplete,
}

/// Compression type for record values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Lz4 = 1,
    Zstd = 2,
}

impl Compression {
    fn from_bits(bits: u8) -> Result<Self, RecordError> {
        match bits {
            0 => Ok(Compression::None),
            1 => Ok(Compression::Lz4),
            2 => Ok(Compression::Zstd),
            v => Err(RecordError::InvalidCompression(v)),
        }
    }

    fn to_bits(self) -> u8 {
        self as u8
    }
}

bitflags::bitflags! {
    struct Flags: u8 {
        const TOMBSTONE = 0b0000_0001;
        const TTL_PRESENT = 0b0000_0010;
        const COMPRESSION_MASK = 0b0000_1100;
    }
}

/// A WAL record representing a key-value operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub key: Bytes,
    pub value: Bytes,
    pub tombstone: bool,
    pub ttl: Option<Duration>,
    pub compression: Compression,
}

impl Record {
    /// Creates a new PUT record.
    pub fn put(key: impl Into<Bytes>, value: impl Into<Bytes>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            tombstone: false,
            ttl: None,
            compression: Compression::None,
        }
    }

    /// Creates a new PUT record with TTL.
    pub fn put_with_ttl(key: impl Into<Bytes>, value: impl Into<Bytes>, ttl: Duration) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            tombstone: false,
            ttl: Some(ttl),
            compression: Compression::None,
        }
    }

    /// Creates a new DELETE record (tombstone).
    pub fn delete(key: impl Into<Bytes>) -> Self {
        Self {
            key: key.into(),
            value: Bytes::new(),
            tombstone: true,
            ttl: None,
            compression: Compression::None,
        }
    }

    /// Sets the compression type for this record.
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }

    /// Encodes the record into bytes with CRC32C checksum.
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::new();

        // Encode klen and vlen as varints
        encode_varint(&mut buf, self.key.len() as u64);
        encode_varint(&mut buf, self.value.len() as u64);

        // Encode flags
        let mut flags = Flags::empty();
        if self.tombstone {
            flags |= Flags::TOMBSTONE;
        }
        if self.ttl.is_some() {
            flags |= Flags::TTL_PRESENT;
        }
        let compression_bits = (self.compression.to_bits() & 0b11) << 2;
        buf.put_u8(flags.bits() | compression_bits);

        // Encode TTL if present
        if let Some(ttl) = self.ttl {
            encode_varint(&mut buf, ttl.as_millis() as u64);
        }

        // Encode key and value
        buf.put_slice(&self.key);
        buf.put_slice(&self.value);

        // Calculate and append CRC32C
        let crc = crc32c::crc32c(&buf);
        buf.put_u32_le(crc);

        buf.freeze()
    }

    /// Decodes a record from bytes, validating the CRC32C checksum.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), RecordError> {
        let original_data = data;
        let original_len = data.len();
        let mut cursor = data;

        // Need at least varint headers + flags + crc (minimum ~6 bytes)
        if cursor.len() < 6 {
            return Err(RecordError::Incomplete);
        }

        // Decode klen and vlen
        let klen = decode_varint(&mut cursor)?;
        let vlen = decode_varint(&mut cursor)?;

        // Decode flags
        if cursor.is_empty() {
            return Err(RecordError::Incomplete);
        }
        let flags_byte = cursor[0];
        cursor.advance(1);

        let flags = Flags::from_bits_truncate(flags_byte);
        let tombstone = flags.contains(Flags::TOMBSTONE);
        let ttl_present = flags.contains(Flags::TTL_PRESENT);
        let compression_bits = (flags_byte & 0b0000_1100) >> 2;
        let compression = Compression::from_bits(compression_bits)?;

        // Decode TTL if present
        let ttl = if ttl_present {
            let ttl_ms = decode_varint(&mut cursor)?;
            Some(Duration::from_millis(ttl_ms))
        } else {
            None
        };

        // Decode key and value
        if cursor.len() < (klen + vlen + 4) as usize {
            return Err(RecordError::Incomplete);
        }

        let key = Bytes::copy_from_slice(&cursor[..klen as usize]);
        cursor.advance(klen as usize);

        let value = Bytes::copy_from_slice(&cursor[..vlen as usize]);
        cursor.advance(vlen as usize);

        // Verify CRC32C
        if cursor.len() < 4 {
            return Err(RecordError::Incomplete);
        }

        let stored_crc = cursor.get_u32_le();
        let bytes_consumed = original_len - cursor.len();

        // Calculate CRC over everything except the CRC itself
        let data_for_crc = &original_data[..bytes_consumed - 4];
        let calculated_crc = crc32c::crc32c(data_for_crc);

        if stored_crc != calculated_crc {
            return Err(RecordError::CrcMismatch {
                expected: stored_crc,
                actual: calculated_crc,
            });
        }

        let record = Record {
            key,
            value,
            tombstone,
            ttl,
            compression,
        };

        Ok((record, bytes_consumed))
    }
}

/// Encodes a u64 as a varint (LEB128).
fn encode_varint(buf: &mut BytesMut, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.put_u8(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decodes a varint (LEB128) from bytes.
fn decode_varint(data: &mut &[u8]) -> Result<u64, RecordError> {
    let mut result = 0u64;
    let mut shift = 0;

    loop {
        if data.is_empty() {
            return Err(RecordError::Incomplete);
        }

        let byte = data[0];
        data.advance(1);

        if shift >= 64 {
            return Err(io::Error::new(ErrorKind::InvalidData, "varint overflow").into());
        }

        result |= ((byte & 0x7F) as u64) << shift;

        if byte & 0x80 == 0 {
            break;
        }

        shift += 7;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_encoding() {
        let test_cases = vec![0u64, 127, 128, 255, 16383, 16384, u64::MAX];

        for value in test_cases {
            let mut buf = BytesMut::new();
            encode_varint(&mut buf, value);
            let mut slice = &buf[..];
            let decoded = decode_varint(&mut slice).unwrap();
            assert_eq!(value, decoded, "varint roundtrip failed for {}", value);
        }
    }

    #[test]
    fn test_record_put_roundtrip() {
        let record = Record::put(b"hello".as_slice(), b"world".as_slice());
        let encoded = record.encode();
        let (decoded, size) = Record::decode(&encoded).unwrap();

        assert_eq!(record, decoded);
        assert_eq!(size, encoded.len());
    }

    #[test]
    fn test_record_delete_roundtrip() {
        let record = Record::delete(b"key_to_delete".as_slice());
        let encoded = record.encode();
        let (decoded, size) = Record::decode(&encoded).unwrap();

        assert_eq!(record, decoded);
        assert!(decoded.tombstone);
        assert_eq!(size, encoded.len());
    }

    #[test]
    fn test_record_with_ttl() {
        let record = Record::put_with_ttl(
            b"tempkey".as_slice(),
            b"tempval".as_slice(),
            Duration::from_millis(5000),
        );
        let encoded = record.encode();
        let (decoded, _) = Record::decode(&encoded).unwrap();

        assert_eq!(record, decoded);
        assert_eq!(decoded.ttl, Some(Duration::from_millis(5000)));
    }

    #[test]
    fn test_record_with_compression() {
        let record =
            Record::put(b"key".as_slice(), b"value".as_slice()).with_compression(Compression::Lz4);
        let encoded = record.encode();
        let (decoded, _) = Record::decode(&encoded).unwrap();

        assert_eq!(record, decoded);
        assert_eq!(decoded.compression, Compression::Lz4);
    }

    #[test]
    fn test_crc_mismatch() {
        let record = Record::put(b"test".as_slice(), b"data".as_slice());
        let encoded = record.encode();

        // Corrupt the data
        let mut corrupted = encoded.to_vec();
        corrupted[5] ^= 0xFF; // Flip some bits in the middle

        let result = Record::decode(&corrupted);
        assert!(matches!(result, Err(RecordError::CrcMismatch { .. })));
    }

    #[test]
    fn test_incomplete_record() {
        let record = Record::put(b"key".as_slice(), b"value".as_slice());
        let encoded = record.encode();

        // Try to decode truncated data
        let result = Record::decode(&encoded[..5]);
        assert!(matches!(result, Err(RecordError::Incomplete)));
    }

    #[test]
    fn test_empty_key_value() {
        let record = Record::put(b"".as_slice(), b"".as_slice());
        let encoded = record.encode();
        let (decoded, _) = Record::decode(&encoded).unwrap();

        assert_eq!(record, decoded);
        assert_eq!(decoded.key.len(), 0);
        assert_eq!(decoded.value.len(), 0);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_record_roundtrip(
            key in prop::collection::vec(any::<u8>(), 0..1024),
            value in prop::collection::vec(any::<u8>(), 0..1024),
            tombstone in any::<bool>(),
            ttl_ms in prop::option::of(0u64..86400000),
        ) {
            let record = Record {
                key: Bytes::from(key),
                value: Bytes::from(value),
                tombstone,
                ttl: ttl_ms.map(Duration::from_millis),
                compression: Compression::None,
            };

            let encoded = record.encode();
            let (decoded, size) = Record::decode(&encoded).unwrap();

            prop_assert_eq!(record, decoded);
            prop_assert_eq!(size, encoded.len());
        }

        #[test]
        fn prop_varint_roundtrip(value in any::<u64>()) {
            let mut buf = BytesMut::new();
            encode_varint(&mut buf, value);
            let mut slice = &buf[..];
            let decoded = decode_varint(&mut slice).unwrap();
            prop_assert_eq!(value, decoded);
        }

        #[test]
        fn prop_corruption_detected(
            key in prop::collection::vec(any::<u8>(), 1..100),
            value in prop::collection::vec(any::<u8>(), 1..100),
            corrupt_index in 0usize..20,
        ) {
            let record = Record::put(Bytes::from(key), Bytes::from(value));
            let encoded = record.encode();

            if corrupt_index < encoded.len() - 4 { // Don't corrupt the CRC itself
                let mut corrupted = encoded.to_vec();
                corrupted[corrupt_index] ^= 0xFF;

                let result = Record::decode(&corrupted);
                // Should either detect corruption via CRC or fail gracefully
                prop_assert!(result.is_err());
            }
        }
    }
}
