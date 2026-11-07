//! Generic Bech32m (BIP-350): the checksum and character layer, with no
//! assumption about what the data means. Address policy lives in `lib.rs`.

use crate::AddressError;

/// Characters of the Bech32 data alphabet, indexed by 5-bit value.
pub const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
/// The Bech32m constant. Bech32 (BIP-173) used 1; the two never collide, so a
/// string valid under one is rejected under the other.
const BECH32M_CONST: u32 = 0x2bc8_30a3;
const GENERATOR: [u32; 5] = [
    0x3b6a_57b2,
    0x2650_8e6d,
    0x1ea1_19fa,
    0x3d42_33dd,
    0x2a14_62b3,
];
/// Checksum length in characters.
pub const CHECKSUM_LEN: usize = 6;
/// BIP-173 limit on the whole string, kept by BIP-350.
pub const MAX_LEN: usize = 90;

fn polymod(values: impl Iterator<Item = u32>) -> u32 {
    values.fold(1u32, |chk, v| {
        let top = chk >> 25;
        let mut next = ((chk & 0x01ff_ffff) << 5) ^ v;
        for (i, g) in GENERATOR.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                next ^= g;
            }
        }
        next
    })
}

fn expand_hrp(hrp: &str) -> impl Iterator<Item = u32> + '_ {
    let bytes = hrp.as_bytes();
    bytes
        .iter()
        .map(|&b| u32::from(b) >> 5)
        .chain(std::iter::once(0))
        .chain(bytes.iter().map(|&b| u32::from(b) & 31))
}

/// The six 5-bit checksum symbols for `hrp` and `data`.
#[must_use]
pub fn create_checksum(hrp: &str, data: &[u8]) -> [u8; CHECKSUM_LEN] {
    let values = expand_hrp(hrp)
        .chain(data.iter().map(|&d| u32::from(d)))
        .chain(std::iter::repeat_n(0, CHECKSUM_LEN));
    let residue = polymod(values) ^ BECH32M_CONST;
    std::array::from_fn(|i| ((residue >> (5 * (CHECKSUM_LEN - 1 - i))) & 31) as u8)
}

/// Splits `text` into its lowercased HRP and its 5-bit symbols (checksum
/// included), refusing anything that is not a well-formed Bech32m string.
///
/// # Errors
///
/// Returns the first structural fault: length, case, separator, HRP range,
/// data alphabet, or checksum.
pub fn split(text: &str) -> Result<(String, Vec<u8>), AddressError> {
    if text.len() > MAX_LEN {
        return Err(AddressError::Length(text.len()));
    }
    let has_lower = text.bytes().any(|b| b.is_ascii_lowercase());
    let has_upper = text.bytes().any(|b| b.is_ascii_uppercase());
    if has_lower && has_upper {
        return Err(AddressError::MixedCase);
    }
    let lower = text.to_ascii_lowercase();
    let sep = lower.rfind('1').ok_or(AddressError::NoSeparator)?;
    let hrp = &lower[..sep];
    if hrp.is_empty() {
        return Err(AddressError::EmptyHrp);
    }
    if let Some(bad) = hrp.bytes().find(|b| !(33..=126).contains(b)) {
        return Err(AddressError::InvalidCharacter(char::from(bad)));
    }
    let mut data = Vec::with_capacity(lower.len() - sep - 1);
    for c in lower[sep + 1..].chars() {
        let symbol = CHARSET
            .iter()
            .position(|&s| char::from(s) == c)
            .ok_or(AddressError::InvalidCharacter(c))?;
        data.push(u8::try_from(symbol).map_err(|_| AddressError::Payload)?);
    }
    if data.len() < CHECKSUM_LEN {
        return Err(AddressError::Payload);
    }
    let checked = expand_hrp(hrp).chain(data.iter().map(|&d| u32::from(d)));
    if polymod(checked) != BECH32M_CONST {
        return Err(AddressError::BadChecksum);
    }
    data.truncate(data.len() - CHECKSUM_LEN);
    Ok((hrp.to_owned(), data))
}

/// Renders `hrp` and 5-bit `data` as a Bech32m string.
#[must_use]
pub fn join(hrp: &str, data: &[u8]) -> String {
    let checksum = create_checksum(hrp, data);
    let mut out = String::with_capacity(hrp.len() + 1 + data.len() + CHECKSUM_LEN);
    out.push_str(hrp);
    out.push('1');
    for &symbol in data.iter().chain(checksum.iter()) {
        out.push(char::from(CHARSET[usize::from(symbol)]));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    // BIP-350, "Test vectors for Bech32m", valid strings.
    #[test]
    fn bip350_valid_strings_pass_the_checksum() {
        let valid = [
            "A1LQFN3A",
            "a1lqfn3a",
            "an83characterlonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11sg7hg6",
            "abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx",
            "11llllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllllludsr8",
            "split1checkupstagehandshakeupstreamerranterredcaperredlc445v",
            "?1v759aa",
        ];
        for text in valid {
            assert!(split(text).is_ok(), "BIP-350 valid vector refused: {text}");
        }
    }

    // BIP-350, "Test vectors for Bech32m", strings that must be refused.
    #[test]
    fn bip350_invalid_strings_are_refused() {
        let invalid = [
            "\u{20}1xj0phk",
            "\u{7f}1g6xzxy",
            "\u{80}1vctc34",
            "an84characterslonghumanreadablepartthatcontainsthetheexcludedcharactersbioandnumber11d6pts4",
            "qyrz8wqd2c9m",
            "1qyrz8wqd2c9m",
            "y1b0jsk6g",
            "lt1igcx5c0",
            "in1muywd",
            "mm1crxm3i",
            "au1s5cgom",
            "M1VUXWEZ",
            "16plkw9",
            "1p2gdwpf",
        ];
        for text in invalid {
            assert!(
                split(text).is_err(),
                "BIP-350 invalid vector accepted: {text:?}"
            );
        }
    }

    #[test]
    fn join_then_split_round_trips() {
        let data = [0u8, 1, 2, 30, 31];
        let text = join("test", &data);
        assert_eq!(split(&text).unwrap(), ("test".to_owned(), data.to_vec()));
    }

    #[test]
    fn one_wrong_character_breaks_the_checksum() {
        let text = join("maya", &[0u8; 10]);
        let mut chars: Vec<char> = text.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'q' { 'p' } else { 'q' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(split(&typo), Err(AddressError::BadChecksum));
    }
}
