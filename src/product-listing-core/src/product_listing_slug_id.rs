domain_primitives::slug_id_newtype!(ProductListingSlugId, 6);

use crate::source_listing_id::SourceListingId;
use std::fmt;

const SHA256_SUFFIX_HEX_LENGTH: usize = 12;

/// Stable, source-scoped URL identity for an opaque `SourceListingId`.
///
/// The readable body is derived from the raw ID; the SHA-256 suffix preserves
/// identity when distinct source IDs normalize to the same slug body.
#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SourceListingSlugId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid source listing slug ID")]
pub struct InvalidSourceListingSlugId;

impl SourceListingSlugId {
    pub fn from_source_listing_id(source_listing_id: &SourceListingId) -> Self {
        let raw = source_listing_id.as_ref();
        let body = slug_body(raw);
        let suffix = sha256_hex(raw)
            .chars()
            .take(SHA256_SUFFIX_HEX_LENGTH)
            .collect::<String>();
        Self(format!("{body}-{suffix}"))
    }

    pub fn raw(value: &str) -> Result<Self, InvalidSourceListingSlugId> {
        let Some((body, suffix)) = value.rsplit_once('-') else {
            return Err(InvalidSourceListingSlugId);
        };
        if body.is_empty()
            || suffix.len() != SHA256_SUFFIX_HEX_LENGTH
            || !suffix
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
            || !body.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
            || body.starts_with('-')
            || body.ends_with('-')
            || body.contains("--")
        {
            return Err(InvalidSourceListingSlugId);
        }
        Ok(Self(value.to_owned()))
    }
}

impl AsRef<str> for SourceListingSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceListingSlugId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

fn slug_body(value: &str) -> String {
    let mut body = String::new();
    let mut previous_was_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            body.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !body.is_empty() {
            body.push('-');
            previous_was_separator = true;
        }
    }
    let body = body.trim_end_matches('-');
    if body.is_empty() {
        "listing".to_owned()
    } else {
        body.to_owned()
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = sha256(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_length = (input.len() as u64).wrapping_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = INITIAL;
    for offset in (0..message.len()).step_by(64) {
        let chunk = &message[offset..offset + 64];
        let mut words = [0_u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let mut bytes = [0_u8; 4];
            bytes.copy_from_slice(&chunk[index * 4..index * 4 + 4]);
            *word = u32::from_be_bytes(bytes);
        }
        for index in 16..64 {
            let small_sigma_0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let small_sigma_1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(small_sigma_0)
                .wrapping_add(words[index - 7])
                .wrapping_add(small_sigma_1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for (index, constant) in K.iter().enumerate() {
            let choice = (e & f) ^ ((!e) & g);
            let big_sigma_1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let temporary_1 = h
                .wrapping_add(big_sigma_1)
                .wrapping_add(choice)
                .wrapping_add(*constant)
                .wrapping_add(words[index]);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let big_sigma_0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let temporary_2 = big_sigma_0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_1);
            d = c;
            c = b;
            b = a;
            a = temporary_1.wrapping_add(temporary_2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::SourceListingSlugId;
    use crate::source_listing_id::SourceListingId;

    #[test]
    fn should_derive_stable_slug_with_sha256_suffix_from_raw_source_listing_id() {
        let raw = SourceListingId::try_from("SKU  #42/Blue")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        let slug = SourceListingSlugId::from_source_listing_id(&raw);

        assert_eq!("sku-42-blue-f4b0c19f13dd", slug.as_ref());
        assert_eq!(slug, SourceListingSlugId::from_source_listing_id(&raw));
    }

    #[test]
    fn should_keep_distinct_raw_ids_distinct_when_slug_bodies_match() {
        let first = SourceListingId::try_from("SKU/42")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        let second = SourceListingId::try_from("SKU 42")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        assert_ne!(
            SourceListingSlugId::from_source_listing_id(&first),
            SourceListingSlugId::from_source_listing_id(&second)
        );
    }
}
