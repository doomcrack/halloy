//! Hex account addresses — the Logos identity primitive. Replaces the
//! IRC-era `User`/`Nick` taxonomy: addresses have no casemapping, no
//! normalization, and compare byte-for-byte.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Characters shown for a shortened address (QML `Identity` parity).
const SHORT_LABEL_CHARS: usize = 8;

/// Characters used for avatar initials (QML `Identity` parity).
const INITIALS_CHARS: usize = 2;

/// FNV-1a 32-bit offset basis.
const FNV_OFFSET: u32 = 2_166_136_261;

/// FNV-1a 32-bit prime.
const FNV_PRIME: u32 = 16_777_619;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Address(Arc<str>);

impl Address {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First [`SHORT_LABEL_CHARS`] characters; the whole string when
    /// shorter.
    pub fn short_label(&self) -> &str {
        short_label(&self.0)
    }

    /// First [`INITIALS_CHARS`] characters, lowercased.
    pub fn initials(&self) -> String {
        initials(&self.0)
    }

    /// Ramp index for this address's avatar. The QML client hashes the
    /// short label everywhere it paints an avatar, so parity requires
    /// hashing the short label here too — not the full address.
    pub fn avatar_ramp(&self, ramp_count: u32) -> u32 {
        avatar_ramp(self.short_label(), ramp_count)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Address {
    fn from(address: &str) -> Self {
        Self(Arc::from(address))
    }
}

impl From<String> for Address {
    fn from(address: String) -> Self {
        Self(Arc::from(address))
    }
}

/// First [`INITIALS_CHARS`] characters of an identity string, lowercased.
/// Takes any identity — the QML client seeds conversation avatars with the
/// conversation id, not the peer address (`Identity::initials` parity).
pub fn initials(identity: &str) -> String {
    identity
        .chars()
        .take(INITIALS_CHARS)
        .flat_map(char::to_lowercase)
        .collect()
}

/// First [`SHORT_LABEL_CHARS`] characters of an identity string, on a char
/// boundary; the whole string when shorter.
pub fn short_label(identity: &str) -> &str {
    identity
        .char_indices()
        .nth(SHORT_LABEL_CHARS)
        .map_or(identity, |(index, _)| &identity[..index])
}

/// FNV-1a 32-bit hash of `identity`, mod `ramp_count`. Deterministic across
/// instances and restarts, matching the QML client's `Identity::avatarRamp`
/// exactly (offset 2166136261, prime 16777619, UTF-8 bytes). Callers must
/// pass the string the avatar displays — the short label — since that is
/// what the QML client hashes at every call site.
pub fn avatar_ramp(identity: &str, ramp_count: u32) -> u32 {
    identity.bytes().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(FNV_PRIME)
    }) % ramp_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_label_truncates_to_eight_chars() {
        let address = Address::from("deadbeef1234");
        assert_eq!(address.short_label(), "deadbeef");
        assert_eq!(Address::from("abc").short_label(), "abc");
    }

    #[test]
    fn initials_are_two_lowercased_chars() {
        assert_eq!(Address::from("ABCDEF").initials(), "ab");
        assert_eq!(Address::from("x").initials(), "x");
    }

    #[test]
    fn avatar_ramp_matches_fnv1a_reference_vectors() {
        // Independently computed FNV-1a 32-bit values, mod 5 — the QML
        // client's kAvatarRampCount.
        assert_eq!(avatar_ramp("", 5), 2_166_136_261 % 5);
        assert_eq!(avatar_ramp("a", 5), 3_826_002_220 % 5);
        assert_eq!(avatar_ramp("foobar", 5), 3_214_735_720 % 5);
        assert_eq!(avatar_ramp("deadbeef", 5), 3_493_560_501 % 5);
    }

    #[test]
    fn avatar_ramp_is_deterministic_and_hashes_the_short_label() {
        let address = Address::from("deadbeef1234");
        assert_eq!(address.avatar_ramp(5), address.avatar_ramp(5));
        // QML parity: the ramp comes from the 8-char short label, so the
        // full address's tail must not change the result.
        assert_eq!(address.avatar_ramp(5), avatar_ramp("deadbeef", 5));
        assert_ne!(avatar_ramp("deadbeef", 5), avatar_ramp("deadbeef1234", 5));
    }
}
