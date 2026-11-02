//! Shared property for the decoder targets that do not go through
//! `TxKind::decode`.
//!
//! Each of these decoders is reachable from a transaction payload or a state
//! record, and each either refuses its input or accepts it. An accepted input
//! must re-encode to the exact bytes it came from, and the re-encoding must
//! decode back to the same value. Two byte strings decoding to one value would
//! mean one object with two hashes, the property `payload_decode` already
//! asserts for the whole tag space.

use custom_l1_node::core::codec::ByteReader;

/// Checks one typed section read through a [`ByteReader`].
///
/// Trailing bytes are not a failure here: a section is often the tail of a
/// frame. Canonicality is asserted only when the section was the whole input,
/// as `payload_decode` does.
pub fn check_section<T, E>(
    payload: &[u8],
    label: &str,
    decode: impl for<'a> Fn(&mut ByteReader<'a>) -> Result<T, E>,
    encode: impl Fn(&T, &mut Vec<u8>),
) where
    T: PartialEq + std::fmt::Debug,
    E: std::fmt::Debug,
{
    let mut reader = ByteReader::new(payload);
    let Ok(value) = decode(&mut reader) else {
        return;
    };
    if reader.finish().is_err() {
        return;
    }

    let mut reencoded = Vec::new();
    encode(&value, &mut reencoded);
    assert_eq!(
        reencoded.as_slice(),
        payload,
        "non-canonical {label} accepted"
    );

    let mut again = ByteReader::new(&reencoded);
    let redecoded = match decode(&mut again) {
        Ok(value) => value,
        Err(error) => panic!("{label}: its own encoding was refused: {error:?}"),
    };
    assert_eq!(
        redecoded, value,
        "{label}: decode is not the inverse of encode"
    );
}

/// Checks one whole-record decoder that takes a byte slice and must consume it
/// exactly, such as the `rwa` state records.
pub fn check_record<T, E>(
    bytes: &[u8],
    label: &str,
    decode: impl Fn(&[u8]) -> Result<T, E>,
    encode: impl Fn(&T) -> Vec<u8>,
) where
    T: PartialEq + std::fmt::Debug,
    E: std::fmt::Debug,
{
    let Ok(value) = decode(bytes) else {
        return;
    };
    assert_eq!(
        encode(&value).as_slice(),
        bytes,
        "non-canonical {label} accepted"
    );

    let again = match decode(&encode(&value)) {
        Ok(value) => value,
        Err(error) => panic!("{label}: its own encoding was refused: {error:?}"),
    };
    assert_eq!(again, value, "{label}: decode is not the inverse of encode");
}
