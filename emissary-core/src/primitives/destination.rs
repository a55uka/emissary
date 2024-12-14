// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Destination
//!
//! https://geti2p.net/spec/common-structures#destination

use crate::{
    crypto::{base64_decode, base64_encode, sha256::Sha256, SigningPublicKey},
    primitives::LOG_TARGET,
};

use bytes::{BufMut, Bytes, BytesMut};
use nom::{
    bytes::complete::take,
    error::{make_error, ErrorKind},
    number::complete::{be_u16, be_u8},
    sequence::tuple,
    Err, IResult,
};

use alloc::{string::String, sync::Arc, vec, vec::Vec};
use core::fmt;

/// Null certificate.
const NULL_CERTIFICATE: u8 = 0x00;

/// Null certificate length.
const NULL_CERTIFICATE_LEN: u16 = 0x00;

/// Key certificate.
const KEY_CERTIFICATE: u8 = 0x05;

/// Key certificate length.
const KEY_CERTIFICATE_LEN: u16 = 0x04;

/// Key kind for `EdDSA_SHA512_Ed25519`.
///
/// https://geti2p.net/spec/common-structures#key-certificates
const KEY_KIND_EDDSA_SHA512_ED25519: u16 = 0x0007;

/// Serialized [`Destination`] length with key certificate
const DESTINATION_WITH_KEY_CERT_LEN: usize = 391usize;

/// Serialized [`Destination`] length with NULL certificate.
const DESTINATION_WITH_NULL_CERT_LEN: usize = 387usize;

/// Minimum serialized length of [`Destination`].
const DESTINATION_MINIMUM_LEN: usize = 387usize;

/// Serialized [`Destination`] length without certificate.
const DESTINATION_LEN_NO_CERTIFICATE: usize = 384usize;

/// Short destination identity hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DestinationId(Arc<String>);

impl DestinationId {
    #[cfg(test)]
    pub fn random() -> DestinationId {
        use rand::Rng;

        DestinationId::from(rand::thread_rng().gen::<[u8; 32]>())
    }

    /// Copy [`DestinationId`] into a byte vector.
    pub fn to_vec(&self) -> Vec<u8> {
        base64_decode(&self.0.as_bytes()).expect("to succeed")
    }
}

impl fmt::Display for DestinationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", &self.0[..8])
    }
}

impl<T: AsRef<[u8]>> From<T> for DestinationId {
    fn from(value: T) -> Self {
        DestinationId(Arc::new(base64_encode(value)))
    }
}

/// Destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// Destination ID.
    destination_id: DestinationId,

    /// Destinations' identity hash.
    identity_hash: Bytes,

    /// Serialized destination, if any.
    serialized: Option<Bytes>,

    /// Destination's verifying key.
    verifying_key: Option<SigningPublicKey>,
}

impl Destination {
    /// Create new [`Destination`] from `verifying_key`.
    pub fn new(verifying_key: SigningPublicKey) -> Self {
        let identity_hash = Sha256::new()
            .update(
                Self {
                    destination_id: DestinationId::from(vec![0]),
                    identity_hash: Bytes::from(vec![0]),
                    serialized: None,
                    verifying_key: Some(verifying_key.clone()),
                }
                .serialize(),
            )
            .finalize();

        Self {
            destination_id: DestinationId::from(identity_hash.clone()),
            identity_hash: Bytes::from(identity_hash),
            serialized: None,
            verifying_key: Some(verifying_key),
        }
    }

    /// Parse [`Destination`] from `input`, returning rest of `input` and parsed [`Destination`].
    pub fn parse_frame(input: &[u8]) -> IResult<&[u8], Destination> {
        if input.len() < DESTINATION_MINIMUM_LEN {
            tracing::warn!(
                target: LOG_TARGET,
                serialized_len = ?input.len(),
                "not enough bytes in destination",
            );

            return Err(Err::Error(make_error(input, ErrorKind::Fail)));
        }

        let (_, (initial_bytes, rest)) = tuple((
            take(DESTINATION_LEN_NO_CERTIFICATE),
            take(input.len() - DESTINATION_LEN_NO_CERTIFICATE),
        ))(input)?;

        let (rest, certificate_kind) = be_u8(rest)?;
        let (rest, certificate_len) = be_u16(rest)?;

        let (rest, verifying_key, destination_len) = match (certificate_kind, certificate_len) {
            (NULL_CERTIFICATE, _) => (rest, None, DESTINATION_WITH_NULL_CERT_LEN),
            (KEY_CERTIFICATE, KEY_CERTIFICATE_LEN) => {
                let (rest, signing_key_kind) = be_u16(rest)?;
                let (rest, _public_key_type) = be_u16(rest)?;

                match signing_key_kind {
                    KEY_KIND_EDDSA_SHA512_ED25519 => (
                        rest,
                        Some(
                            SigningPublicKey::from_bytes(&initial_bytes[384 - 32..384])
                                .ok_or_else(|| Err::Error(make_error(input, ErrorKind::Fail)))?,
                        ),
                        DESTINATION_WITH_KEY_CERT_LEN,
                    ),
                    key_kind => {
                        tracing::warn!(
                            target: LOG_TARGET,
                            ?key_kind,
                            "unsupported key kind for destination",
                        );
                        return Err(Err::Error(make_error(input, ErrorKind::Fail)));
                    }
                }
            }
            (certificate_kind, certificate_len) => {
                tracing::debug!(
                    target: LOG_TARGET,
                    ?certificate_kind,
                    ?certificate_len,
                    "unsupported certificate type for destination",
                );
                return Err(Err::Error(make_error(input, ErrorKind::Fail)));
            }
        };

        let identity_hash = Bytes::from(Sha256::new().update(&input[..destination_len]).finalize());
        let serialized = Some(Bytes::from(input[..destination_len].to_vec()));

        Ok((
            rest,
            Destination {
                destination_id: DestinationId::from(identity_hash.clone()),
                identity_hash,
                serialized,
                verifying_key,
            },
        ))
    }

    /// Try to parse router information from `bytes`.
    pub fn parse(input: impl AsRef<[u8]>) -> Option<Self> {
        Some(Self::parse_frame(input.as_ref()).ok()?.1)
    }

    /// Serialize [`Destination`] into a byte vector.
    pub fn serialize(&self) -> Bytes {
        if let Some(serialized) = &self.serialized {
            return serialized.clone();
        }

        let mut out = BytesMut::with_capacity(self.serialized_len());

        out.put_slice(&[0u8; 32]);
        out.put_slice(&[0u8; 320]);

        match &self.verifying_key {
            None => {
                out.put_slice(&[0u8; 32]); // empty verifying key
                out.put_u8(NULL_CERTIFICATE);
                out.put_u16(NULL_CERTIFICATE_LEN);
            }
            Some(signing_key) => {
                out.put_slice(&signing_key.to_bytes());
                out.put_u8(KEY_CERTIFICATE);
                out.put_u16(KEY_CERTIFICATE_LEN);
                out.put_u16(KEY_KIND_EDDSA_SHA512_ED25519); // verifying key type
                out.put_u16(0u16); // public key type
            }
        }

        out.freeze()
    }

    /// Get serialized length of [`Destination`].
    pub fn serialized_len(&self) -> usize {
        let certificate_payload_len = self.verifying_key.as_ref().map_or(0usize, |_| 4usize);

        32usize // all zeros public key
            .saturating_add(320usize) // padding
            .saturating_add(32usize) // signing public key
            .saturating_add(1usize) // certificate type
            .saturating_add(2usize) // certificate length
            .saturating_add(certificate_payload_len)
    }

    /// Get [`DestinationId`].
    pub fn id(&self) -> DestinationId {
        self.destination_id.clone()
    }

    /// Get reference to `SigningPublicKey` of the [`Destination`].
    pub fn verifying_key(&self) -> Option<&SigningPublicKey> {
        self.verifying_key.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_and_parse_destination() {
        let signing_key = SigningPublicKey::from_private_ed25519(&[0xaa; 32]).unwrap();
        let destination = Destination::new(signing_key.clone());

        let serialized = destination.clone().serialize();
        let parsed = Destination::parse(&serialized).unwrap();

        assert_eq!(parsed.destination_id, destination.destination_id);
        assert_eq!(
            parsed.verifying_key.unwrap().to_bytes(),
            destination.verifying_key.unwrap().to_bytes()
        );
    }

    #[test]
    fn too_small_input() {
        assert!(Destination::parse(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).is_none());
    }

    #[test]
    fn deserialize_valid_destination() {
        let input = vec![
            216, 194, 83, 151, 200, 24, 84, 242, 184, 222, 34, 89, 37, 175, 254, 228, 173, 78, 114,
            24, 15, 104, 241, 215, 166, 166, 102, 40, 136, 138, 22, 252, 114, 203, 192, 101, 32,
            156, 212, 74, 177, 120, 153, 172, 221, 181, 175, 190, 178, 17, 71, 33, 39, 211, 208,
            241, 30, 35, 222, 99, 215, 32, 242, 40, 202, 70, 197, 171, 84, 202, 52, 173, 221, 153,
            242, 77, 240, 30, 133, 4, 126, 31, 105, 24, 87, 209, 111, 140, 122, 58, 242, 224, 61,
            95, 144, 26, 25, 129, 202, 69, 130, 238, 88, 201, 34, 132, 197, 242, 129, 223, 50, 194,
            130, 227, 102, 209, 79, 209, 70, 202, 48, 248, 32, 124, 230, 90, 90, 104, 199, 23, 141,
            60, 213, 122, 20, 94, 223, 251, 48, 64, 30, 97, 36, 40, 194, 119, 98, 83, 29, 0, 31,
            113, 241, 52, 72, 175, 208, 221, 251, 220, 146, 82, 235, 83, 172, 33, 118, 249, 114,
            218, 149, 42, 76, 8, 137, 125, 81, 209, 156, 68, 75, 58, 79, 245, 124, 41, 243, 228,
            244, 162, 239, 31, 176, 185, 44, 202, 6, 202, 200, 127, 247, 43, 80, 178, 76, 120, 211,
            75, 157, 84, 199, 229, 62, 10, 51, 143, 31, 218, 237, 8, 232, 227, 1, 168, 159, 119,
            35, 41, 43, 67, 241, 91, 87, 213, 118, 129, 172, 192, 92, 176, 79, 63, 80, 251, 160,
            212, 50, 194, 46, 229, 59, 15, 48, 93, 62, 80, 237, 86, 159, 203, 194, 165, 80, 22,
            108, 18, 64, 58, 210, 130, 124, 26, 198, 206, 159, 132, 252, 96, 155, 124, 35, 108,
            231, 22, 53, 246, 114, 232, 108, 192, 249, 122, 24, 236, 5, 210, 53, 149, 124, 6, 12,
            36, 59, 144, 19, 176, 11, 159, 46, 184, 45, 193, 58, 134, 179, 130, 176, 122, 34, 177,
            172, 147, 35, 19, 123, 22, 176, 182, 216, 78, 246, 104, 110, 62, 111, 117, 110, 174,
            49, 132, 214, 130, 96, 112, 30, 211, 159, 113, 131, 151, 166, 156, 206, 227, 20, 21,
            115, 66, 8, 218, 103, 153, 78, 46, 127, 199, 169, 197, 168, 124, 158, 232, 115, 71,
            104, 19, 165, 200, 234, 67, 168, 253, 137, 220, 5, 0, 4, 0, 7, 0, 0,
        ];

        let destination = Destination::parse(&input).unwrap();
        let serialized = destination.serialize();

        assert_eq!(input, *serialized);
    }

    #[test]
    fn null_certificate_works() {
        let input = vec![
            89, 215, 97, 216, 78, 133, 203, 37, 193, 23, 180, 175, 81, 129, 202, 116, 223, 175,
            141, 253, 255, 55, 171, 170, 65, 99, 94, 4, 52, 204, 208, 253, 247, 98, 56, 144, 8,
            235, 50, 121, 218, 227, 152, 54, 102, 88, 90, 215, 80, 151, 201, 45, 105, 194, 111,
            150, 231, 41, 236, 223, 147, 139, 131, 104, 204, 163, 254, 235, 195, 27, 252, 175, 45,
            87, 5, 129, 195, 214, 73, 71, 123, 5, 241, 160, 202, 111, 179, 169, 193, 181, 171, 80,
            220, 51, 203, 223, 186, 127, 148, 75, 182, 26, 152, 25, 102, 180, 46, 140, 103, 104,
            254, 252, 136, 42, 206, 104, 44, 134, 43, 90, 241, 162, 207, 9, 243, 64, 3, 164, 186,
            123, 101, 12, 142, 59, 70, 237, 2, 23, 151, 26, 76, 121, 206, 249, 118, 65, 221, 38,
            85, 86, 111, 58, 228, 247, 63, 16, 130, 187, 183, 96, 137, 52, 83, 59, 88, 128, 76, 3,
            52, 22, 230, 247, 2, 39, 177, 177, 225, 175, 113, 237, 1, 246, 180, 217, 7, 32, 69, 90,
            145, 55, 99, 231, 65, 123, 170, 80, 155, 59, 71, 191, 244, 244, 86, 79, 18, 248, 162,
            33, 197, 41, 145, 141, 197, 123, 34, 229, 95, 91, 32, 64, 80, 94, 25, 224, 61, 233,
            185, 90, 62, 246, 77, 25, 222, 138, 156, 215, 96, 124, 184, 12, 121, 188, 121, 73, 44,
            66, 248, 222, 10, 100, 196, 140, 7, 62, 92, 130, 137, 208, 23, 127, 230, 216, 113, 197,
            69, 34, 60, 231, 58, 153, 52, 110, 87, 245, 178, 77, 243, 155, 124, 210, 91, 98, 191,
            85, 181, 122, 207, 25, 157, 5, 184, 122, 205, 117, 175, 179, 43, 188, 147, 87, 207,
            150, 230, 72, 126, 184, 215, 34, 72, 189, 46, 170, 35, 195, 137, 36, 218, 69, 84, 18,
            16, 73, 114, 195, 251, 222, 147, 107, 42, 203, 64, 246, 152, 195, 251, 141, 103, 231,
            151, 104, 78, 134, 229, 214, 33, 138, 227, 124, 196, 123, 5, 35, 84, 36, 117, 165, 26,
            85, 153, 239, 12, 103, 236, 69, 186, 82, 228, 107, 105, 81, 176, 67, 111, 34, 228, 116,
            251, 171, 27, 72, 187, 116, 221, 112, 0, 0, 0, 0, 0, 2, 6, 31, 139, 8, 0, 0, 1, 0, 0,
            2, 17, 1, 239, 1, 16, 254, 186, 2, 0, 215, 250, 169, 148, 79, 97,
        ];

        let destination = Destination::parse(&input[..387]).unwrap();
        let serialized = destination.serialize();

        assert_eq!(&input[..387], &*serialized);
    }
}
