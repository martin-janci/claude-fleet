//! The release keys every build trusts (U1): minisign public keys, base64 as
//! in `minisign.pub`'s second line.
//!
//! Empty until the owner generates the release key (update design §13,
//! question 1; slice S2). With no key every signature is refused, so nothing
//! is ever offered: an update engine that cannot verify is one that does
//! nothing, never one that installs unverified bytes. A rotation adds the
//! next key here *and* in a signed channel document's `next_keys`.
pub const RELEASE_KEYS: &[&str] = &[];

/// [`RELEASE_KEYS`] as a [`TrustedKeys`](crate::TrustedKeys).
pub fn release_keys() -> crate::TrustedKeys {
    crate::TrustedKeys::from_base64(RELEASE_KEYS.iter().copied())
        .expect("a compiled-in release key must decode")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_compiled_in_keys_decode() {
        let _ = super::release_keys();
    }
}
