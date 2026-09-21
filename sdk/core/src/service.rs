//! The service record: what a seller promises, to whom, and by when.
//!
//! The record is bound into the agreement, so changing any field changes the commitment and
//! invalidates every authorization over the previous terms. This is what keeps a payment
//! authorization from being reused for a different purchased resource or a different access
//! recipient (roadmap M1: "Service-record mutations invalidate authorization").
//!
//! Payment status and delivery status are deliberately not fields here. They live in
//! [`crate::settlement`], because a service record is a promise, not an outcome.

use crate::encoding::{EncodingError, Reader, Writer};
use crate::ids::{BaseUnits, IdError, KeyBytes, MAX_KEY_BYTES};

/// Longest accepted resource identifier, in bytes.
pub const MAX_RESOURCE_BYTES: usize = 256;
/// Longest accepted unit name, in bytes.
pub const MAX_UNIT_BYTES: usize = 32;
/// Longest accepted fulfillment method name, in bytes.
pub const MAX_FULFILLMENT_METHOD_BYTES: usize = 64;

/// What a seller promises to deliver, and to whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    /// Resource identifier, for example `gpu.h100.hour`.
    pub resource: String,
    /// Number of units purchased; must be greater than zero.
    pub quantity: BaseUnits,
    /// Unit name, for example `gpu-hour`.
    pub unit: String,
    /// Key that receives the delivered service. This is distinct from the payment recipient.
    pub access_recipient: KeyBytes,
    /// Unix timestamp by which delivery must occur.
    pub delivery_deadline: u64,
    /// Fulfillment method identifier, for example `http-access`.
    pub fulfillment_method: String,
    /// Commitment to out-of-band fulfillment parameters; all zero when there are none.
    pub fulfillment_digest: [u8; 32],
}

/// A service record was internally inconsistent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// The resource identifier was empty or too long.
    #[error("resource must be 1 to 256 bytes, found {0}")]
    InvalidResourceLength(usize),
    /// The quantity was zero.
    #[error("service quantity must be greater than zero")]
    ZeroQuantity,
    /// The unit name was empty or too long.
    #[error("unit must be 1 to 32 bytes, found {0}")]
    InvalidUnitLength(usize),
    /// The delivery deadline was zero.
    #[error("delivery deadline must be greater than zero")]
    ZeroDeliveryDeadline,
    /// The fulfillment method was empty or too long.
    #[error("fulfillment method must be 1 to 64 bytes, found {0}")]
    InvalidFulfillmentMethodLength(usize),
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A canonical encoding could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
}

impl ServiceRecord {
    /// Checks every field's bounds and consistency rules.
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.resource.is_empty() || self.resource.len() > MAX_RESOURCE_BYTES {
            return Err(ServiceError::InvalidResourceLength(self.resource.len()));
        }
        if self.quantity.is_zero() {
            return Err(ServiceError::ZeroQuantity);
        }
        if self.unit.is_empty() || self.unit.len() > MAX_UNIT_BYTES {
            return Err(ServiceError::InvalidUnitLength(self.unit.len()));
        }
        if self.delivery_deadline == 0 {
            return Err(ServiceError::ZeroDeliveryDeadline);
        }
        if self.fulfillment_method.is_empty()
            || self.fulfillment_method.len() > MAX_FULFILLMENT_METHOD_BYTES
        {
            return Err(ServiceError::InvalidFulfillmentMethodLength(
                self.fulfillment_method.len(),
            ));
        }
        Ok(())
    }

    pub(crate) fn write(&self, writer: &mut Writer) {
        writer.text(&self.resource);
        writer.u128(self.quantity.get());
        writer.text(&self.unit);
        writer.bytes(self.access_recipient.as_bytes());
        writer.u64(self.delivery_deadline);
        writer.text(&self.fulfillment_method);
        writer.fixed(&self.fulfillment_digest);
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, ServiceError> {
        let resource = reader.text("resource", 1, MAX_RESOURCE_BYTES)?.to_owned();
        let quantity = BaseUnits::new(reader.u128("quantity")?);
        let unit = reader.text("unit", 1, MAX_UNIT_BYTES)?.to_owned();
        let access_recipient =
            KeyBytes::from_validated(reader.bytes("access_recipient", 1, MAX_KEY_BYTES)?.to_vec());
        let delivery_deadline = reader.u64("delivery_deadline")?;
        let fulfillment_method = reader
            .text("fulfillment_method", 1, MAX_FULFILLMENT_METHOD_BYTES)?
            .to_owned();
        let fulfillment_digest = reader.fixed("fulfillment_digest")?;
        Ok(Self {
            resource,
            quantity,
            unit,
            access_recipient,
            delivery_deadline,
            fulfillment_method,
            fulfillment_digest,
        })
    }

    /// Encodes the record after validating it.
    pub fn encode(&self) -> Result<Vec<u8>, ServiceError> {
        self.validate()?;
        let mut writer = Writer::new();
        self.write(&mut writer);
        Ok(writer.finish())
    }

    /// Decodes a complete record from its canonical bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ServiceError> {
        let mut reader = Reader::new(bytes);
        let record = Self::read(&mut reader)?;
        reader.finish()?;
        record.validate()?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ServiceRecord {
        ServiceRecord {
            resource: "gpu.h100.hour".to_owned(),
            quantity: BaseUnits::new(500),
            unit: "gpu-hour".to_owned(),
            access_recipient: KeyBytes::new(vec![0x22; 20]).expect("valid key"),
            delivery_deadline: 1_800_000_000,
            fulfillment_method: "http-access".to_owned(),
            fulfillment_digest: [0u8; 32],
        }
    }

    #[test]
    fn round_trips_through_the_canonical_encoding() {
        let record = record();
        let bytes = record.encode().expect("encodes");
        assert_eq!(ServiceRecord::decode(&bytes), Ok(record));
    }

    #[test]
    fn zero_quantity_is_rejected() {
        let mut record = record();
        record.quantity = BaseUnits::new(0);
        assert_eq!(record.validate(), Err(ServiceError::ZeroQuantity));
    }

    #[test]
    fn empty_resource_is_rejected() {
        let mut record = record();
        record.resource = String::new();
        assert_eq!(
            record.validate(),
            Err(ServiceError::InvalidResourceLength(0))
        );
    }

    #[test]
    fn over_long_resource_is_rejected_at_decode_and_validate() {
        let mut record = record();
        record.resource = "x".repeat(MAX_RESOURCE_BYTES + 1);
        assert_eq!(
            record.validate(),
            Err(ServiceError::InvalidResourceLength(MAX_RESOURCE_BYTES + 1))
        );
        // The writer is deliberately unchecked; the decoder is the strict side.
        let mut writer = Writer::new();
        record.write(&mut writer);
        let bytes = writer.finish();
        assert_eq!(
            ServiceRecord::decode(&bytes),
            Err(ServiceError::Encoding(EncodingError::LengthOutOfRange(
                "resource",
                MAX_RESOURCE_BYTES + 1
            )))
        );
    }
}
