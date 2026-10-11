#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use base64::Engine;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, RsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

    fn b64(v: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v)
    }

    pub(crate) struct EsSigner {
        pair: EcdsaKeyPair,
        rng: SystemRandom,
        pub kid: String,
    }

    impl EsSigner {
        pub(crate) fn new(kid: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair = EcdsaKeyPair::from_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                pkcs8.as_ref(),
                &rng,
            )
            .unwrap();
            Self {
                pair,
                rng,
                kid: kid.into(),
            }
        }
        pub(crate) fn jwk(&self) -> serde_json::Value {
            let pt = self.pair.public_key().as_ref(); // 0x04 || x || y
            serde_json::json!({"kty":"EC","crv":"P-256","kid":self.kid,"use":"sig","alg":"ES256",
                "x": b64(&pt[1..33]), "y": b64(&pt[33..65])})
        }
        pub(crate) fn sign(&self, claims: &serde_json::Value) -> String {
            let head = b64(
                serde_json::json!({"alg":"ES256","typ":"JWT","kid":self.kid})
                    .to_string()
                    .as_bytes(),
            );
            let body = b64(claims.to_string().as_bytes());
            let msg = format!("{head}.{body}");
            let sig = self.pair.sign(&self.rng, msg.as_bytes()).unwrap();
            format!("{msg}.{}", b64(sig.as_ref()))
        }
    }

    fn rsa_pair() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!("testdata/rsa2048.pk8")).unwrap()
    }

    fn rsa_jwk(pair: &RsaKeyPair, kid: &str) -> serde_json::Value {
        let c: ring::rsa::PublicKeyComponents<Vec<u8>> = pair.public().into();
        serde_json::json!({"kty":"RSA","kid":kid,"n":b64(&c.n),"e":b64(&c.e)})
    }

    fn rsa_sign(pair: &RsaKeyPair, alg: &str, kid: &str, claims: &serde_json::Value) -> String {
        let enc: &dyn ring::signature::RsaEncoding = match alg {
            "RS256" => &ring::signature::RSA_PKCS1_SHA256,
            "PS256" => &ring::signature::RSA_PSS_SHA256,
            _ => unreachable!(),
        };
        let head = b64(serde_json::json!({"alg":alg,"kid":kid})
            .to_string()
            .as_bytes());
        let msg = format!("{head}.{}", b64(claims.to_string().as_bytes()));
        let mut sig = vec![0; pair.public().modulus_len()];
        pair.sign(enc, &SystemRandom::new(), msg.as_bytes(), &mut sig)
            .unwrap();
        format!("{msg}.{}", b64(&sig))
    }

    fn keys(v: serde_json::Value) -> Vec<Jwk> {
        parse_jwks(&serde_json::json!({ "keys": [v] })).unwrap()
    }

    #[test]
    fn es256_rs256_and_ps256_verify() {
        let es = EsSigner::new("e1");
        assert_eq!(
            verify(&es.sign(&serde_json::json!({"a":1})), &keys(es.jwk())),
            Verdict::Valid
        );
        let rsa = rsa_pair();
        for alg in ["RS256", "PS256"] {
            let t = rsa_sign(&rsa, alg, "r1", &serde_json::json!({"a":1}));
            assert_eq!(verify(&t, &keys(rsa_jwk(&rsa, "r1"))), Verdict::Valid, "{alg}");
        }
    }

    #[test]
    fn a_tampered_body_or_another_key_is_invalid() {
        let es = EsSigner::new("e1");
        let t = es.sign(&serde_json::json!({"sub":"ada"}));
        let mut parts: Vec<&str> = t.split('.').collect();
        let forged = b64(br#"{"sub":"eve"}"#);
        parts[1] = &forged;
        assert!(matches!(
            verify(&parts.join("."), &keys(es.jwk())),
            Verdict::Invalid(_)
        ));
        let other = EsSigner::new("e1");
        assert!(matches!(verify(&t, &keys(other.jwk())), Verdict::Invalid(_)));
    }

    #[test]
    fn none_and_hmac_are_refused_whatever_the_keys() {
        let es = EsSigner::new("e1");
        for alg in ["none", "HS256"] {
            let head = b64(serde_json::json!({"alg":alg,"kid":"e1"})
                .to_string()
                .as_bytes());
            let t = format!("{head}.{}.", b64(b"{}"));
            assert!(
                matches!(verify(&t, &keys(es.jwk())), Verdict::Invalid(_)),
                "{alg}"
            );
        }
    }

    #[test]
    fn an_unknown_kid_is_unknown_key_not_invalid() {
        let es = EsSigner::new("new-key");
        let old = EsSigner::new("old-key");
        assert_eq!(
            verify(&es.sign(&serde_json::json!({})), &keys(old.jwk())),
            Verdict::UnknownKey
        );
    }

    #[test]
    fn an_encryption_key_never_verifies() {
        let es = EsSigner::new("e1");
        let mut jwk = es.jwk();
        jwk["use"] = "enc".into();
        assert_eq!(
            verify(&es.sign(&serde_json::json!({})), &keys(jwk)),
            Verdict::UnknownKey
        );
    }
}
