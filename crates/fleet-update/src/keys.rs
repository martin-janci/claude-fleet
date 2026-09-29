//! The release keys every build trusts (U1): minisign public keys, base64 as
//! in `minisign.pub`'s second line.
//!
//! The owner's key, made on their machine by `scripts/release-key.sh`
//! (update design §13, question 1); its secret half is only the
//! `RELEASE_SIGNING_KEY` repository secret and the owner's backup. With no
//! key every signature is refused, so nothing is ever offered: an update
//! engine that cannot verify is one that does nothing, never one that
//! installs unverified bytes. A rotation adds the next key here *and* in a
//! signed channel document's `next_keys` (docs/RELEASING.md).
pub const RELEASE_KEYS: &[&str] = &[
    // Created by scripts/release-key.sh.
    "RWR7mtPD4eO3+OpcUzdJ0YLgvLn8dqjnQ1QBlXh33aYecfvKTmS19E9p",
];

/// [`RELEASE_KEYS`] as a [`TrustedKeys`](crate::TrustedKeys).
pub fn release_keys() -> crate::TrustedKeys {
    crate::TrustedKeys::from_base64(RELEASE_KEYS.iter().copied())
        .expect("a compiled-in release key must decode")
}

#[cfg(test)]
mod tests {
    use super::RELEASE_KEYS;

    #[test]
    fn the_compiled_in_keys_decode() {
        // A build with no key would trust nothing and offer nothing.
        assert!(!RELEASE_KEYS.is_empty(), "keys.rs names no release key");
        // Every key decodes, and none is listed twice (`TrustedKeys` drops a
        // repeat, so a duplicate would show as a shorter list).
        let keys = super::release_keys().to_base64();
        assert_eq!(
            keys.len(),
            RELEASE_KEYS.len(),
            "a release key is listed twice"
        );
        assert_eq!(
            keys,
            RELEASE_KEYS
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
        );
    }
}
