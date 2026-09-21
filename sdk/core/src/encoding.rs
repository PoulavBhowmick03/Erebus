//! Canonical byte encoding for agreement data.
//!
//! The rules are deliberately small. Every integer is big-endian at its full fixed width;
//! every variable byte string and text field is length-prefixed with a `u16`; every field
//! order is fixed by the schema that documents it. A decoder accepts exactly one byte
//! sequence per value: truncated fields, trailing bytes, over-long lengths, invalid tags,
//! and invalid UTF-8 are all errors.
//!
//! Two encoders that follow the rules cannot disagree on the bytes. That property is what
//! lets a commitment computed in Rust be recomputed by an EVM contract, and it is pinned by
//! the vectors under `tests/fixtures/agreement-v1-vectors.json`.
//!
//! This module owns no schema. Each schema writes and reads its fields in its own documented
//! order using [`Writer`] and [`Reader`].

/// Longest length accepted for any length-prefixed field.
pub const MAX_FIELD_BYTES: usize = 4096;

/// A canonical byte sequence could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncodingError {
    /// A field ended before its declared width.
    #[error("field `{0}` is truncated")]
    Truncated(&'static str),
    /// Bytes remained after the last field of the schema.
    #[error("trailing bytes after the last field")]
    TrailingBytes,
    /// A tag byte named no known variant.
    #[error("field `{0}` has unknown tag {1}")]
    UnknownTag(&'static str, u8),
    /// A length prefix was outside the range the field permits.
    #[error("field `{0}` has length {1}, outside the permitted range")]
    LengthOutOfRange(&'static str, usize),
    /// A text field was not valid UTF-8.
    #[error("field `{0}` is not valid UTF-8")]
    InvalidUtf8(&'static str),
    /// An optional field was tagged with a value other than 0 or 1.
    #[error("field `{0}` has optional tag {1}, expected 0 or 1")]
    InvalidOptionalTag(&'static str, u8),
}

/// Appends canonical fields to a byte buffer.
///
/// The writer does not enforce schema bounds. Values in this crate are bounded when they are
/// constructed, so a value that exists is encodable; the reader re-checks every bound.
#[derive(Debug, Default)]
pub struct Writer {
    buffer: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes one byte.
    pub fn u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    /// Writes a 16-bit big-endian integer.
    pub fn u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a 32-bit big-endian integer.
    pub fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a 64-bit big-endian integer.
    pub fn u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a 128-bit big-endian integer.
    pub fn u128(&mut self, value: u128) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a fixed-width byte string with no length prefix.
    ///
    /// Panics on a value longer than [`MAX_FIELD_BYTES`]: fixed-width fields are protocol
    /// constants and an over-long one is a programming error, not input.
    pub fn fixed(&mut self, value: &[u8]) {
        assert!(
            value.len() <= MAX_FIELD_BYTES,
            "fixed field exceeds MAX_FIELD_BYTES"
        );
        self.buffer.extend_from_slice(value);
    }

    /// Writes a length-prefixed byte string.
    ///
    /// Panics on a value longer than [`MAX_FIELD_BYTES`]; see [`Self::fixed`].
    pub fn bytes(&mut self, value: &[u8]) {
        assert!(
            value.len() <= MAX_FIELD_BYTES,
            "length-prefixed field exceeds MAX_FIELD_BYTES"
        );
        let length = u16::try_from(value.len()).expect("MAX_FIELD_BYTES fits in u16");
        self.u16(length);
        self.buffer.extend_from_slice(value);
    }

    /// Writes a length-prefixed UTF-8 string.
    pub fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// Writes the presence tag for an optional field.
    pub fn presence(&mut self, present: bool) {
        self.u8(u8::from(present));
    }

    /// Returns the encoded bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buffer
    }
}

/// Reads canonical fields from a byte slice.
pub struct Reader<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// Creates a reader over a complete encoding.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, position: 0 }
    }

    fn take(&mut self, field: &'static str, count: usize) -> Result<&'a [u8], EncodingError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(EncodingError::Truncated(field))?;
        if end > self.input.len() {
            return Err(EncodingError::Truncated(field));
        }
        let slice = &self.input[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// Reads one byte.
    pub fn u8(&mut self, field: &'static str) -> Result<u8, EncodingError> {
        Ok(self.take(field, 1)?[0])
    }

    /// Reads a 16-bit big-endian integer.
    pub fn u16(&mut self, field: &'static str) -> Result<u16, EncodingError> {
        let bytes = self.take(field, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Reads a 32-bit big-endian integer.
    pub fn u32(&mut self, field: &'static str) -> Result<u32, EncodingError> {
        let bytes = self.take(field, 4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a 64-bit big-endian integer.
    pub fn u64(&mut self, field: &'static str) -> Result<u64, EncodingError> {
        let bytes = self.take(field, 8)?;
        let mut value = [0u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(value))
    }

    /// Reads a 128-bit big-endian integer.
    pub fn u128(&mut self, field: &'static str) -> Result<u128, EncodingError> {
        let bytes = self.take(field, 16)?;
        let mut value = [0u8; 16];
        value.copy_from_slice(bytes);
        Ok(u128::from_be_bytes(value))
    }

    /// Reads a fixed-width byte string with no length prefix.
    pub fn fixed<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], EncodingError> {
        let bytes = self.take(field, N)?;
        let mut value = [0u8; N];
        value.copy_from_slice(bytes);
        Ok(value)
    }

    /// Reads a length-prefixed byte string, requiring a length in `min..=max`.
    pub fn bytes(
        &mut self,
        field: &'static str,
        min: usize,
        max: usize,
    ) -> Result<&'a [u8], EncodingError> {
        let length = usize::from(self.u16(field)?);
        if length < min || length > max {
            return Err(EncodingError::LengthOutOfRange(field, length));
        }
        self.take(field, length)
    }

    /// Reads a length-prefixed UTF-8 string, requiring a byte length in `min..=max`.
    pub fn text(
        &mut self,
        field: &'static str,
        min: usize,
        max: usize,
    ) -> Result<&'a str, EncodingError> {
        let bytes = self.bytes(field, min, max)?;
        core::str::from_utf8(bytes).map_err(|_| EncodingError::InvalidUtf8(field))
    }

    /// Reads a presence tag for an optional field.
    pub fn optional(&mut self, field: &'static str) -> Result<bool, EncodingError> {
        match self.u8(field)? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(EncodingError::InvalidOptionalTag(field, tag)),
        }
    }

    /// Requires that the entire input was consumed.
    pub fn finish(self) -> Result<(), EncodingError> {
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(EncodingError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_full_width_big_endian() {
        let mut writer = Writer::new();
        writer.u8(0x01);
        writer.u16(0x0203);
        writer.u32(0x0405_0607);
        writer.u64(0x0809_0a0b_0c0d_0e0f);
        writer.u128(0x1011_1213_1415_1617_1819_1a1b_1c1d_1e1f);
        let bytes = writer.finish();
        assert_eq!(
            bytes,
            [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
                0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
                0x1d, 0x1e, 0x1f,
            ]
        );
    }

    #[test]
    fn lengths_are_self_delimiting() {
        let mut writer = Writer::new();
        writer.text("ab");
        writer.text("c");
        let mut other = Writer::new();
        other.text("a");
        other.text("bc");
        assert_ne!(writer.finish(), other.finish());
    }

    #[test]
    fn truncation_is_reported_at_the_field() {
        let mut writer = Writer::new();
        writer.bytes(b"abc");
        let bytes = writer.finish();
        for end in 0..bytes.len() {
            let mut reader = Reader::new(&bytes[..end]);
            let error = reader.bytes("field", 1, 16).expect_err("must truncate");
            assert_eq!(error, EncodingError::Truncated("field"));
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut reader = Reader::new(&[0x00, 0x00, 0x00]);
        reader.bytes("field", 0, 16).expect("empty bytes");
        assert_eq!(reader.finish(), Err(EncodingError::TrailingBytes));
    }

    #[test]
    fn bounds_and_tags_are_strict() {
        let mut writer = Writer::new();
        writer.bytes(&[0u8; 4]);
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.bytes("field", 1, 2),
            Err(EncodingError::LengthOutOfRange("field", 4))
        );

        let mut reader = Reader::new(&[0x02]);
        assert_eq!(
            reader.optional("flag"),
            Err(EncodingError::InvalidOptionalTag("flag", 2))
        );
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut writer = Writer::new();
        writer.bytes(&[0xff, 0xfe]);
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.text("text", 0, 8),
            Err(EncodingError::InvalidUtf8("text"))
        );
    }
}
