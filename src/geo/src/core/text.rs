use super::country::CountryCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidGeoText {
    #[error("geographic text is blank")]
    Blank,
    #[error("geographic text exceeds {max_bytes} UTF-8 bytes")]
    TooLong { max_bytes: usize },
    #[error("geographic text contains a forbidden control character")]
    ControlCharacter,
}

fn validate(value: &str, max_bytes: usize, multiline: bool) -> Result<(), InvalidGeoText> {
    if value.len() > max_bytes {
        return Err(InvalidGeoText::TooLong { max_bytes });
    }
    if value.trim().is_empty() {
        return Err(InvalidGeoText::Blank);
    }
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if multiline && (ch == '\n' || (ch == '\r' && chars.peek() == Some(&'\n'))) {
            continue;
        }
        if ch.is_control() || (!multiline && matches!(ch, '\u{2028}' | '\u{2029}')) {
            return Err(InvalidGeoText::ControlCharacter);
        }
    }
    Ok(())
}

/// An observation of international address text, preserved byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AddressText(String);

impl AddressText {
    pub const MAX_BYTES: usize = 4096;
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidGeoText> {
        let value = value.into();
        validate(&value, Self::MAX_BYTES, true)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Postal text is not proof of existence or deliverability, and is never numeric.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PostalCode(String);

impl PostalCode {
    pub const MAX_BYTES: usize = 64;
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidGeoText> {
        let value = value.into();
        validate(&value, Self::MAX_BYTES, false)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Comparison only; never use this key to rewrite the source text.
    /// GB and CA: fold ASCII case and remove one separator only for recognized shapes.
    /// Other countries and unfamiliar shapes retain the exact text.
    pub fn comparison_key(&self, country: CountryCode) -> String {
        let compact: String = self.0.chars().filter(|ch| *ch != ' ').collect();
        let ascii_alnum = compact.bytes().all(|b| b.is_ascii_alphanumeric());
        // Accept compact form or one space before the final three characters.
        // Unfamiliar spacing is evidence rather than something to silently discard.
        let conventional_spacing = self.0 == compact
            || (compact.len() >= 3
                && ascii_alnum
                && self.0
                    == format!(
                        "{} {}",
                        &compact[..compact.len() - 3],
                        &compact[compact.len() - 3..]
                    ));
        let canonical = compact.to_ascii_uppercase();
        let supported = conventional_spacing
            && match country {
                CountryCode::CAN => canadian_postal_shape(canonical.as_bytes()),
                CountryCode::GBR => uk_postal_shape(canonical.as_bytes()),
                _ => false,
            };
        if supported { canonical } else { self.0.clone() }
    }

    pub fn equivalent_in(&self, other: &Self, country: CountryCode) -> bool {
        self.comparison_key(country) == other.comparison_key(country)
    }
}

fn canadian_postal_shape(bytes: &[u8]) -> bool {
    // Canada Post's PCCF reference: D/F/I/O/Q/U are unused, plus W/Z initially.
    matches!(bytes, [a, b, c, d, e, f]
        if b"ABCEGHJKLMNPRSTVXY".contains(a)
            && b.is_ascii_digit()
            && b"ABCEGHJKLMNPRSTVWXYZ".contains(c)
            && d.is_ascii_digit()
            && b"ABCEGHJKLMNPRSTVWXYZ".contains(e)
            && f.is_ascii_digit())
}

fn uk_postal_shape(bytes: &[u8]) -> bool {
    // GIR 0AA is the established exception; other GIR inward codes stay verbatim.
    if bytes == b"GIR0AA" {
        return true;
    }
    if !(5..=7).contains(&bytes.len()) {
        return false;
    }
    let (outward, inward) = bytes.split_at(bytes.len() - 3);
    let inward_shape =
        inward[0].is_ascii_digit() && inward[1..].iter().all(u8::is_ascii_alphabetic);
    // GOV.UK's documented shapes. Unrecognized forms are preserved, never rejected.
    let outward_shape = match outward {
        [a, b] => a.is_ascii_alphabetic() && b.is_ascii_digit(),
        [a, b, c] => {
            a.is_ascii_alphabetic()
                && ((b.is_ascii_digit() && c.is_ascii_alphanumeric())
                    || (b"ABCDEFGHJKLMNOPQRSTUVWXY".contains(b) && c.is_ascii_digit()))
        }
        [a, b, c, d] => {
            a.is_ascii_alphabetic()
                && b"ABCDEFGHJKLMNOPQRSTUVWXY".contains(b)
                && c.is_ascii_digit()
                && d.is_ascii_alphanumeric()
        }
        _ => false,
    };
    outward_shape && inward_shape
}
