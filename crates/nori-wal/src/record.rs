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
    #[error("Compression failed: {0}")]
    CompressionFailed(String),
    #[error("Decompression failed: {0}")]
    DecompressionFailed(String),
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

        // Compress value if needed
        let value_to_write = match self.compression {
            Compression::None => self.value.clone(),
            Compression::Lz4 => {
                // Prepend original size for decompression
                let compressed = lz4::block::compress(&self.value, None, false).unwrap_or_else(|_| self.value.to_vec());
                let mut buf = BytesMut::new();
                encode_varint(&mut buf, self.value.len() as u64);
                buf.put_slice(&compressed);
                buf.freeze()
            }
            Compression::Zstd => {
                Bytes::from(zstd::encode_all(&self.value[..], 3).unwrap_or_else(|_| self.value.to_vec()))
            }
        };

        // Encode klen and vlen as varints (vlen is compressed size)
        encode_varint(&mut buf, self.key.len() as u64);
        encode_varint(&mut buf, value_to_write.len() as u64);

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

        // Encode key and value (value is already compressed if needed)
        buf.put_slice(&self.key);
        buf.put_slice(&value_to_write);

        // Calculate and append CRC32C
        let crc = crc32c::crc32c(&buf);
        buf.put_u32_le(crc);

        buf.freeze()
    }

    /// Decodes a record from bytes, validating the CRC32C checksum.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), RecordError> {
        if data.len() < 6 {
            return Err(RecordError::Incomplete);
        }

        let mut cursor = data;

        // Parse header: lengths, flags, and optional TTL
        let klen = decode_varint(&mut cursor)?;
        let vlen = decode_varint(&mut cursor)?;
        let (tombstone, ttl, compression) = Self::decode_flags_and_ttl(&mut cursor)?;

        // Extract key and compressed value
        let key = Self::extract_bytes(&mut cursor, klen)?;
        let compressed_value = Self::extract_bytes(&mut cursor, vlen)?;

        // Verify CRC
        let bytes_consumed = data.len() - cursor.len() + 4;
        Self::verify_crc(data, bytes_consumed, &mut cursor)?;

        // Decompress value
        let value = Self::decompress_value(compressed_value, compression)?;

        Ok((
            Record {
                key,
                value,
                tombstone,
                ttl,
                compression,
            },
            bytes_consumed,
        ))
    }

    fn decode_flags_and_ttl(
        cursor: &mut &[u8],
    ) -> Result<(bool, Option<Duration>, Compression), RecordError> {
        if cursor.is_empty() {
            return Err(RecordError::Incomplete);
        }

        let flags_byte = cursor[0];
        cursor.advance(1);

        let flags = Flags::from_bits_truncate(flags_byte);
        let tombstone = flags.contains(Flags::TOMBSTONE);
        let compression_bits = (flags_byte & 0b0000_1100) >> 2;
        let compression = Compression::from_bits(compression_bits)?;

        let ttl = if flags.contains(Flags::TTL_PRESENT) {
            let ttl_ms = decode_varint(cursor)?;
            Some(Duration::from_millis(ttl_ms))
        } else {
            None
        };

        Ok((tombstone, ttl, compression))
    }

    fn extract_bytes(cursor: &mut &[u8], len: u64) -> Result<Bytes, RecordError> {
        let len = len as usize;
        if cursor.len() < len {
            return Err(RecordError::Incomplete);
        }

        let bytes = Bytes::copy_from_slice(&cursor[..len]);
        cursor.advance(len);
        Ok(bytes)
    }

    fn verify_crc(data: &[u8], bytes_consumed: usize, cursor: &mut &[u8]) -> Result<(), RecordError> {
        if cursor.len() < 4 {
            return Err(RecordError::Incomplete);
        }

        let stored_crc = cursor.get_u32_le();
        let data_for_crc = &data[..bytes_consumed - 4];
        let calculated_crc = crc32c::crc32c(data_for_crc);

        if stored_crc != calculated_crc {
            return Err(RecordError::CrcMismatch {
                expected: stored_crc,
                actual: calculated_crc,
            });
        }

        Ok(())
    }

    fn decompress_value(compressed: Bytes, compression: Compression) -> Result<Bytes, RecordError> {
        match compression {
            Compression::None => Ok(compressed),
            Compression::Lz4 => {
                let mut cursor = &compressed[..];
                let original_size = decode_varint(&mut cursor)? as usize;
                let decompressed = lz4::block::decompress(cursor, Some(original_size as i32))
                    .map_err(|e| RecordError::DecompressionFailed(e.to_string()))?;
                Ok(Bytes::from(decompressed))
            }
            Compression::Zstd => {
                let decompressed = zstd::decode_all(&compressed[..])
                    .map_err(|e| RecordError::DecompressionFailed(e.to_string()))?;
                Ok(Bytes::from(decompressed))
            }
        }
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
    fn test_record_with_lz4_compression() {
        // Create a record with compressible data
        let value = Bytes::from(b"hello world ".repeat(100)); // Highly compressible
        let key = Bytes::from(&b"key"[..]);
        let record = Record::put(key.clone(), value.clone())
            .with_compression(Compression::Lz4);

        let encoded = record.encode();
        let (decoded, size) = Record::decode(&encoded).unwrap();

        // Verify decompressed value matches original
        assert_eq!(decoded.value, value);
        assert_eq!(decoded.compression, Compression::Lz4);
        assert_eq!(size, encoded.len());

        // Verify compression actually reduced size
        let uncompressed_record = Record::put(key, value);
        let uncompressed_encoded = uncompressed_record.encode();
        assert!(encoded.len() < uncompressed_encoded.len());
    }

    #[test]
    fn test_record_with_zstd_compression() {
        // Create a record with compressible data
        let value = Bytes::from(b"the quick brown fox ".repeat(50));
        let key = Bytes::from(&b"mykey"[..]);
        let record = Record::put(key.clone(), value.clone())
            .with_compression(Compression::Zstd);

        let encoded = record.encode();
        let (decoded, size) = Record::decode(&encoded).unwrap();

        // Verify decompressed value matches original
        assert_eq!(decoded.value, value);
        assert_eq!(decoded.compression, Compression::Zstd);
        assert_eq!(size, encoded.len());

        // Verify compression actually reduced size
        let uncompressed_record = Record::put(key, value);
        let uncompressed_encoded = uncompressed_record.encode();
        assert!(encoded.len() < uncompressed_encoded.len());
    }

    #[test]
    fn test_compression_with_random_data() {
        // Random data shouldn't compress well
        let value: Vec<u8> = (0..100).map(|i| (i * 37 + 13) as u8).collect();
        let key = Bytes::from(&b"key"[..]);

        let lz4_record = Record::put(key.clone(), Bytes::from(value.clone()))
            .with_compression(Compression::Lz4);
        let lz4_encoded = lz4_record.encode();
        let (lz4_decoded, _) = Record::decode(&lz4_encoded).unwrap();

        assert_eq!(lz4_decoded.value.as_ref(), value.as_slice());

        let zstd_record = Record::put(key, Bytes::from(value.clone()))
            .with_compression(Compression::Zstd);
        let zstd_encoded = zstd_record.encode();
        let (zstd_decoded, _) = Record::decode(&zstd_encoded).unwrap();

        assert_eq!(zstd_decoded.value.as_ref(), value.as_slice());
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
