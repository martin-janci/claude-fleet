//! The release keys every build trusts (U1): minisign public keys, base64 as
//! in `minisign.pub`'s second line.
//!
//! The owner's release key, made once on their machine by
//! `scripts/release-key.sh` (update design §13, question 1; slice S2); its
//! secret half is only the `RELEASE_SIGNING_KEY` repository secret and the
//! owner's backup. A signature by any other key is refused, and with no key
//! every signature is refused, so nothing is ever offered: an update engine
//! that cannot verify is one that does nothing, never one that installs
//! unverified bytes. A rotation (design §10) adds the next key here *and*
//! in a signed channel document's `next_keys` (docs/RELEASING.md).
pub const RELEASE_KEYS: &[&str] = &[
    // Created by scripts/release-key.sh.
    "RWR7mtPD4eO3+OpcUzdJ0YLgvLn8dqjnQ1QBlXh33aYecfvKTmS19E9p",
];

/// [`RELEASE_KEYS`] as a [`TrustedKeys`](crate::TrustedKeys).
pub fn release_keys() -> crate::TrustedKeys {
    crate::TrustedKeys::from_base64(RELEASE_KEYS.iter().copied())
        .expect("a compiled-in release key must decode")
}

/// The public key `tauri-plugin-updater` wants for a bundle `signature`
/// (S7): the base64 of a `minisign.pub` file, picked from `keys` by the key
/// id the signature names. The plugin checks one key per bundle, so the
/// signature says which; `None` when no trusted key signed it (the plugin
/// would refuse it anyway, but this says so before a download).
pub fn tauri_pubkey_for(tauri_signature: &str, keys: &[&str]) -> Option<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    let sig_text = String::from_utf8(B64.decode(tauri_signature.trim()).ok()?).ok()?;
    let sig = B64.decode(sig_text.lines().nth(1)?.trim()).ok()?;
    let key_id = sig.get(2..10)?;
    keys.iter().find_map(|k| {
        let raw = B64.decode(k.trim()).ok()?;
        (raw.get(2..10)? == key_id).then(|| tauri_pubkey(k))
    })
}

/// One release key (base64, as in [`RELEASE_KEYS`]) in the form
/// tauri-plugin-updater reads: the base64 of a whole `minisign.pub` file.
/// `src-tauri/tauri.conf.json`'s `plugins.updater.pubkey` is this of the
/// current key (a test there holds it to that).
pub fn tauri_pubkey(key: &str) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    let raw = B64.decode(key.trim()).unwrap_or_default();
    let id: String = raw
        .get(2..10)
        .unwrap_or_default()
        .iter()
        .rev()
        .map(|b| format!("{b:02X}"))
        .collect();
    B64.encode(format!(
        "untrusted comment: minisign public key {id}\n{}\n",
        key.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::RELEASE_KEYS;

    #[test]
    fn the_tauri_pubkey_is_the_one_that_signed_the_bundle() {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;
        let (a, b) = (
            crate::testkit::TestKey::new(1),
            crate::testkit::TestKey::new(2),
        );
        let data = b"the bundle";
        let sig = B64.encode(b.sign(data));
        let (pa, pb) = (a.public(), b.public());
        let pk = super::tauri_pubkey_for(&sig, &[pa.as_str(), pb.as_str()]).unwrap();
        // What tauri-plugin-updater does with the two: decode, parse, verify.
        let pk_text = String::from_utf8(B64.decode(pk).unwrap()).unwrap();
        let sig_text = String::from_utf8(B64.decode(&sig).unwrap()).unwrap();
        let pk = minisign_verify::PublicKey::decode(&pk_text).unwrap();
        let sig = minisign_verify::Signature::decode(&sig_text).unwrap();
        pk.verify(data, &sig, true).unwrap();
        assert!(super::tauri_pubkey_for(&B64.encode(a.sign(data)), &[pb.as_str()]).is_none());
        assert!(super::tauri_pubkey_for("not base64!", &[pa.as_str()]).is_none());
    }

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
