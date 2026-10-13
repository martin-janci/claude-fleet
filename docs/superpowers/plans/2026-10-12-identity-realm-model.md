# Identity realm model + user-management hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the hub a Keycloak-shaped realm model (hierarchical groups, realm / client / composite roles, role mappings) that drives fleet authorization, and harden user management (JWKS-verified SSO, audit log, re-enable, device-token expiry, per-person privileges).

**Architecture:** New tables in fleet-core's SQLite store are the source of truth; `org_members` becomes a projection written only by `reproject_org_memberships`, so every existing reader (ViewScope, `org_admin::Authority`, grants) is unchanged. Realm roles reach authorization as three person-id sets loaded beside the token rows in `AuthRows` and applied to the resolved `Caller` (`is_fleet_admin`, per-person trust). A new `realm_admin` tool (MCP + desktop routing) and `fleet-hub realm` CLI administer it.

**Tech Stack:** Rust (fleet-core, fleet-hub, src-tauri), rusqlite / SQLite, `ring` 0.17 (already in `Cargo.lock`), rmcp tool router, Svelte 5 declarative pages.

**Spec:** `docs/superpowers/specs/2026-10-11-identity-realm-model-design.md` — read it before any task.

## Global Constraints

- Work in the worktree `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet-identity-realm` only; every git command as `git -C <that path> …`. Never pull, push, rebase, checkout or stash.
- Validation ladder (CLAUDE.md): after each edit `cargo fleet-fast-check`; per task `scripts/verify.sh`; before a PR `scripts/verify.sh full`. Never narrow with `-p`; narrow with a test filter: `cargo fleet-test -- <filter>`. Run long commands in the foreground with a 600000 ms timeout.
- Migration numbers: take the next free number from `git -C <wt> ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1` right before creating the file. This plan assumes 172 is the last on main, so it uses **173, 174, 175, 176**; if main moved, renumber (and `scripts/renumber-migrations.sh` after a merge).
- Every new migration writes `INSERT OR IGNORE INTO schema_version (version) VALUES (N);` and must be safe to re-run (the upgrade tests delete `schema_version >= N` and migrate again).
- No foreign keys from new tables to `people` / `orgs` (migration-066 rationale). Every new table that decides what a caller resolves to gets the `auth_epoch` insert / update / delete trigger trio (template: `migrations/107_org_members.sql`).
- Store methods return `Result<T, IpcError>` with `codes::E_VALIDATE | E_NOTFOUND | E_EXISTS | E_FORBIDDEN | E_INTERNAL`; helpers that run inside a caller's transaction end in `_in_tx`, take `&rusqlite::Connection`, and never open a transaction.
- `guard::access_allows` must not take a store (R6-l). Role decisions are precomputed onto `Caller`.
- Child processes only through `fleet_core::proc`; shell values only through `crate::shell::quote`; never hold the store `Mutex` guard across `.await`.
- Seeded names (exact): client `fleet`; realm roles `fleet-admin`, `fleet-trusted`, `assets-admin`; client roles of `fleet`: `org-admin`, `org-member`, `org-viewer` with composites `org-admin ⊃ org-member ⊃ org-viewer`; group `/orgs` and per org `/orgs/<org name>` with managed subgroups `admins`, `members`, `viewers`.
- Settings keys (exact): `auth.device_token_ttl_days` (default 90, 0 = never), `auth.device_token_max_age_days` (default 0 = off), `auth.device_token_ttl_since` (written by migration, not a registered setting), `auth.audit_retention_days` (default 365, 0 = forever).
- Commit messages: Conventional Commits, no attribution lines.
- Generated files are regenerated, never hand-edited: `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`, `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen`, `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current`, `REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current`, `REGEN_HUB_CONTRACT=1 …` (a REGEN run fails on purpose; run again without the variable).

## PR stack

| PR | Tasks | Branch (from the previous one) |
|---|---|---|
| 1 | 1 | `feat/oidc-jwks` (from `origin/main`) |
| 2 | 2, 3, 4, 5 | `feat/realm-model` |
| 3 | 6, 7 | `feat/realm-authz` |
| 4 | 8 | `feat/realm-audit` |
| 5 | 9 | `feat/device-token-expiry` |
| 6 | 10, 11, 12, 13 | `feat/realm-admin` |

The spec and this plan live on `docs/identity-realm-model`; PR 1 branches from `origin/main` and the docs branch is merged first or cherry-picked onto PR 1.

---

### Task 1: Verify the ID token's signature against the issuer's JWKS

**Files:**
- Modify: `crates/fleet-core/Cargo.toml` (add `ring = "0.17"` beside `sha2`)
- Create: `crates/fleet-core/src/mcp/oidc/jwks.rs`
- Modify: `crates/fleet-core/src/mcp/oidc.rs` (`Endpoints` :199-205, `endpoints()` :313-361, `OidcProvider` :256-298, `finish` :495-500, module docs :21-25, `jwt_claims` comment :676-677)
- Create: `crates/fleet-core/src/mcp/oidc/testdata/rsa2048.pk8`
- Modify: `crates/fleet-core/src/mcp/oidc/tests.rs`
- Modify: `docs/hub.md` (SSO section ~:1061-1141: one paragraph on signature verification)

**Interfaces:**
- Produces: `pub(super) fn parse_jwks(doc: &serde_json::Value) -> Result<Vec<Jwk>, String>`, `pub(super) fn verify(jwt: &str, keys: &[Jwk]) -> Verdict`, `pub(super) enum Verdict { Valid, UnknownKey, Invalid(String) }`; `OidcProvider::verify_id_token(&self, jwt: &str) -> Result<(), Refusal>`.

- [ ] **Step 1: Create the RSA fixture**

```bash
mkdir -p crates/fleet-core/src/mcp/oidc/testdata
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -outform DER -out crates/fleet-core/src/mcp/oidc/testdata/rsa2048.pk8
```

(A test-only key; it never signs anything outside tests.)

- [ ] **Step 2: Write the jwks unit tests (failing — module missing)**

At the bottom of the new `jwks.rs`, a `#[cfg(test)] mod tests` that builds keys with ring and checks the verdicts:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, RsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

    fn b64(v: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v)
    }

    pub(crate) struct EsSigner { pair: EcdsaKeyPair, rng: SystemRandom, pub kid: String }

    impl EsSigner {
        pub(crate) fn new(kid: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
            Self { pair, rng, kid: kid.into() }
        }
        pub(crate) fn jwk(&self) -> serde_json::Value {
            let pt = self.pair.public_key().as_ref(); // 0x04 || x || y
            serde_json::json!({"kty":"EC","crv":"P-256","kid":self.kid,"use":"sig","alg":"ES256",
                "x": b64(&pt[1..33]), "y": b64(&pt[33..65])})
        }
        pub(crate) fn sign(&self, claims: &serde_json::Value) -> String {
            let head = b64(serde_json::json!({"alg":"ES256","typ":"JWT","kid":self.kid}).to_string().as_bytes());
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
        let enc = match alg {
            "RS256" => &ring::signature::RSA_PKCS1_SHA256,
            "PS256" => &ring::signature::RSA_PSS_SHA256,
            _ => unreachable!(),
        };
        let head = b64(serde_json::json!({"alg":alg,"kid":kid}).to_string().as_bytes());
        let msg = format!("{head}.{}", b64(claims.to_string().as_bytes()));
        let mut sig = vec![0; pair.public().modulus_len()];
        pair.sign(enc, &SystemRandom::new(), msg.as_bytes(), &mut sig).unwrap();
        format!("{msg}.{}", b64(&sig))
    }

    fn keys(v: serde_json::Value) -> Vec<Jwk> {
        parse_jwks(&serde_json::json!({ "keys": [v] })).unwrap()
    }

    #[test]
    fn es256_rs256_and_ps256_verify() {
        let es = EsSigner::new("e1");
        assert_eq!(verify(&es.sign(&serde_json::json!({"a":1})), &keys(es.jwk())), Verdict::Valid);
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
        assert!(matches!(verify(&parts.join("."), &keys(es.jwk())), Verdict::Invalid(_)));
        let other = EsSigner::new("e1");
        assert!(matches!(verify(&t, &keys(other.jwk())), Verdict::Invalid(_)));
    }

    #[test]
    fn none_and_hmac_are_refused_whatever_the_keys() {
        let es = EsSigner::new("e1");
        for alg in ["none", "HS256"] {
            let head = b64(serde_json::json!({"alg":alg,"kid":"e1"}).to_string().as_bytes());
            let t = format!("{head}.{}.", b64(b"{}"));
            assert!(matches!(verify(&t, &keys(es.jwk())), Verdict::Invalid(_)), "{alg}");
        }
    }

    #[test]
    fn an_unknown_kid_is_unknown_key_not_invalid() {
        let es = EsSigner::new("new-key");
        let old = EsSigner::new("old-key");
        assert_eq!(verify(&es.sign(&serde_json::json!({})), &keys(old.jwk())), Verdict::UnknownKey);
    }

    #[test]
    fn an_encryption_key_never_verifies() {
        let es = EsSigner::new("e1");
        let mut jwk = es.jwk();
        jwk["use"] = "enc".into();
        assert_eq!(verify(&es.sign(&serde_json::json!({})), &keys(jwk)), Verdict::UnknownKey);
    }
}
```

- [ ] **Step 3: Run to see it fail**

Run: `cargo fleet-test -- mcp::oidc::jwks`
Expected: compile error — `jwks` module / `parse_jwks` / `verify` not found.

- [ ] **Step 4: Implement `jwks.rs`**

```rust
//! The issuer's signing keys (its JWKS) and the ID-token signature check.
//! Only asymmetric algorithms a provider publishes keys for: RS256, PS256,
//! ES256. `none` and every HMAC algorithm are refused before a key is
//! looked at — an HMAC "key" would be the client secret, which the hub
//! shares with the provider and must never accept a signature from.

use base64::Engine;
use ring::signature::{self, UnparsedPublicKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Jwk {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Valid,
    /// No published key fits the token's `kid` / algorithm: the provider may
    /// have rotated, so the caller refetches once.
    UnknownKey,
    Invalid(String),
}

const ALGS: [&str; 3] = ["RS256", "PS256", "ES256"];

pub(super) fn parse_jwks(doc: &serde_json::Value) -> Result<Vec<Jwk>, String> {
    let keys = doc["keys"].as_array().ok_or("the JWKS has no `keys` array")?;
    let s = |v: &serde_json::Value, k: &str| v[k].as_str().map(str::to_string);
    Ok(keys
        .iter()
        // A key published for encryption never verifies a signature.
        .filter(|k| k["use"].as_str().is_none_or(|u| u == "sig"))
        .filter_map(|k| {
            Some(Jwk {
                kid: s(k, "kid"),
                kty: s(k, "kty")?,
                alg: s(k, "alg"),
                n: s(k, "n"),
                e: s(k, "e"),
                crv: s(k, "crv"),
                x: s(k, "x"),
                y: s(k, "y"),
            })
        })
        .collect())
}

fn b64d(v: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(v).ok()
}

pub(super) fn verify(jwt: &str, keys: &[Jwk]) -> Verdict {
    let mut parts = jwt.split('.');
    let (Some(h), Some(b), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Verdict::Invalid("not a three-part JWT".into());
    };
    let Some(header) = b64d(h).and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok()) else {
        return Verdict::Invalid("unreadable JWT header".into());
    };
    let alg = header["alg"].as_str().unwrap_or("");
    if !ALGS.contains(&alg) {
        return Verdict::Invalid(format!("algorithm {alg:?} is not accepted"));
    }
    let Some(sig) = b64d(sig) else {
        return Verdict::Invalid("unreadable signature".into());
    };
    let kty = if alg == "ES256" { "EC" } else { "RSA" };
    let kid = header["kid"].as_str();
    let candidates: Vec<&Jwk> = keys
        .iter()
        .filter(|k| k.kty == kty)
        .filter(|k| k.alg.as_deref().is_none_or(|a| a == alg))
        .filter(|k| kid.is_none() || k.kid.as_deref() == kid)
        .collect();
    if candidates.is_empty() {
        return Verdict::UnknownKey;
    }
    let msg = format!("{h}.{b}");
    if candidates.iter().any(|k| verify_one(alg, k, msg.as_bytes(), &sig)) {
        Verdict::Valid
    } else {
        Verdict::Invalid("the signature does not verify".into())
    }
}

fn verify_one(alg: &str, k: &Jwk, msg: &[u8], sig: &[u8]) -> bool {
    match alg {
        "RS256" | "PS256" => {
            let (Some(n), Some(e)) = (k.n.as_deref().and_then(b64d), k.e.as_deref().and_then(b64d)) else {
                return false;
            };
            let params = if alg == "RS256" {
                &signature::RSA_PKCS1_2048_8192_SHA256
            } else {
                &signature::RSA_PSS_2048_8192_SHA256
            };
            signature::RsaPublicKeyComponents { n: &n, e: &e }.verify(params, msg, sig).is_ok()
        }
        "ES256" => {
            if k.crv.as_deref() != Some("P-256") {
                return false;
            }
            let (Some(x), Some(y)) = (k.x.as_deref().and_then(b64d), k.y.as_deref().and_then(b64d)) else {
                return false;
            };
            if x.len() != 32 || y.len() != 32 {
                return false;
            }
            let mut pt = Vec::with_capacity(65);
            pt.push(0x04);
            pt.extend_from_slice(&x);
            pt.extend_from_slice(&y);
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, pt).verify(msg, sig).is_ok()
        }
        _ => false,
    }
}
```

Declare it in `oidc.rs` next to the tests module: `mod jwks;`. If `ring::rsa::PublicKeyComponents: From<&PublicKey>` does not exist in the locked ring version, build `n`/`e` in the test from `pair.public().modulus().big_endian_without_leading_zero()` / `.exponent()` instead.

- [ ] **Step 5: Run the jwks tests**

Run: `cargo fleet-test -- mcp::oidc::jwks`
Expected: 5 passed.

- [ ] **Step 6: Wire it into the provider (tests first)**

In `tests.rs`:
1. Add `"jwks_uri": format!("{ISS}/protocol/openid-connect/certs")` to `discovery()`.
2. Replace `jwt()` with a signer shared per test thread, and serve its JWKS from `fake_with_discovery()`:

```rust
thread_local! {
    static SIGNER: super::jwks::tests::EsSigner = super::jwks::tests::EsSigner::new("k1");
}

fn jwt(claims: &serde_json::Value) -> String {
    SIGNER.with(|s| s.sign(claims))
}

fn jwks_doc() -> serde_json::Value {
    SIGNER.with(|s| serde_json::json!({ "keys": [s.jwk()] }))
}
```

and in `fake_with_discovery()` add `fake.always(Method::Get, "/protocol/openid-connect/certs", Ok(HttpResponse::json(200, &jwks_doc())));`. (Make `jwks::tests` `pub(super)` and `EsSigner` `pub(crate)` as written above.)

3. New tests:

```rust
#[tokio::test]
async fn an_id_token_signed_by_another_key_is_refused() {
    let fake = fake_with_discovery();
    let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
    let redirect = format!("{HUB}/auth/oidc/callback");
    let (url, state) = oidc.begin(&redirect, None).await.unwrap();
    let forger = super::jwks::tests::EsSigner::new("k1");
    let id = forger.sign(&id_claims(&query_param(&url, "nonce")));
    fake.once(Method::Post, "/protocol/openid-connect/token", Ok(HttpResponse::json(200,
        &serde_json::json!({"id_token": id, "access_token": jwt(&serde_json::json!({})), "token_type": "Bearer"}))));
    let err = oidc.finish(&redirect, "c", &state).await.unwrap_err();
    assert!(matches!(err, Refusal::Invalid(_)), "{err:?}");
}

#[tokio::test]
async fn an_unknown_kid_refetches_the_jwks_once() {
    let fake = fake_with_discovery();
    let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
    let redirect = format!("{HUB}/auth/oidc/callback");
    // Prime the cache with the current key by one good sign-in.
    let (url, state) = oidc.begin(&redirect, None).await.unwrap();
    token_answer(&fake, &id_claims(&query_param(&url, "nonce")), &[]);
    oidc.finish(&redirect, "c", &state).await.unwrap();
    // A token under a kid nobody published: exactly one more JWKS fetch, then refused.
    let before = fake.count("/protocol/openid-connect/certs");
    let (url, state) = oidc.begin(&redirect, None).await.unwrap();
    let stranger = super::jwks::tests::EsSigner::new("k-rotated");
    fake.once(Method::Post, "/protocol/openid-connect/token", Ok(HttpResponse::json(200,
        &serde_json::json!({"id_token": stranger.sign(&id_claims(&query_param(&url, "nonce"))),
            "access_token": jwt(&serde_json::json!({})), "token_type": "Bearer"}))));
    assert!(matches!(oidc.finish(&redirect, "c", &state).await, Err(Refusal::Invalid(_))));
    assert_eq!(fake.count("/protocol/openid-connect/certs"), before + 1);
}
```

Add to `discovery_must_name_this_issuer_and_stay_on_its_host` a case where `jwks_uri` is `https://evil.example.com/certs` → `Refusal::Provider`, and one where `jwks_uri` is absent → `Refusal::Provider`.

- [ ] **Step 7: Run to see them fail**

Run: `cargo fleet-test -- mcp::oidc`
Expected: the two new tests and the discovery cases FAIL (signature not checked, `jwks_uri` not read).

- [ ] **Step 8: Implement in `oidc.rs`**

1. `struct Endpoints { authorization: String, token: String, jwks: String }`; in `endpoints()` read `jwks: endpoint("jwks_uri")?` through the same https + same-host closure.
2. New field on `OidcProvider`: `jwks: tokio::sync::Mutex<JwksCache>` with

```rust
#[derive(Default)]
struct JwksCache {
    keys: Vec<jwks::Jwk>,
    fetched_at: Option<std::time::Instant>,
    /// When an unknown `kid` last forced a refetch.
    refetched_at: Option<std::time::Instant>,
}
/// Keys are re-read at most this often when nothing forces it.
const JWKS_TTL: std::time::Duration = std::time::Duration::from_secs(3600);
/// An unknown `kid` refetches at most this often, so a stream of forged
/// tokens cannot turn the hub into a load generator against the provider.
const JWKS_REFETCH_FLOOR: std::time::Duration = std::time::Duration::from_secs(60);
```

initialised `Default::default()` in `with_transport`.
3. Methods:

```rust
async fn fetch_jwks(&self, cache: &mut JwksCache) -> Result<(), Refusal> {
    let url = self.endpoints().await?.jwks;
    let resp = self.transport.send(Request::get(&url)).await
        .map_err(|e| Refusal::Provider(format!("the JWKS could not be fetched: {e}")))?;
    if !resp.is_success() {
        return Err(Refusal::Provider("the JWKS request did not succeed".into()));
    }
    let doc: serde_json::Value = resp.parse_json()
        .map_err(|e| Refusal::Provider(format!("the JWKS is not JSON: {e}")))?;
    cache.keys = jwks::parse_jwks(&doc).map_err(Refusal::Provider)?;
    cache.fetched_at = Some(std::time::Instant::now());
    Ok(())
}

async fn verify_id_token(&self, jwt: &str) -> Result<(), Refusal> {
    let mut cache = self.jwks.lock().await;
    if cache.fetched_at.is_none_or(|t| t.elapsed() > JWKS_TTL) {
        self.fetch_jwks(&mut cache).await?;
    }
    match jwks::verify(jwt, &cache.keys) {
        jwks::Verdict::Valid => return Ok(()),
        jwks::Verdict::Invalid(why) => return Err(Refusal::Invalid(format!("id_token: {why}"))),
        jwks::Verdict::UnknownKey => {}
    }
    let unknown = || Refusal::Invalid("id_token: signed by a key the provider does not publish".into());
    if cache.refetched_at.is_some_and(|t| t.elapsed() < JWKS_REFETCH_FLOOR) {
        return Err(unknown());
    }
    cache.refetched_at = Some(std::time::Instant::now());
    self.fetch_jwks(&mut cache).await?;
    match jwks::verify(jwt, &cache.keys) {
        jwks::Verdict::Valid => Ok(()),
        jwks::Verdict::Invalid(why) => Err(Refusal::Invalid(format!("id_token: {why}"))),
        jwks::Verdict::UnknownKey => Err(unknown()),
    }
}
```

4. In `finish`, call `self.verify_id_token(id_token).await?;` immediately before `jwt_claims(id_token)` (:498). The access token stays decoded unverified — it is only read for roles of a token the verified ID token's exchange returned; say so in the comment.
5. Rewrite the module doc (:21-25) and the `jwt_claims` comment (:676-677): the ID token's signature IS verified against the issuer's JWKS (same host as the issuer, RS256 / PS256 / ES256 only).

- [ ] **Step 9: Run**

Run: `cargo fleet-test -- mcp::oidc`
Expected: all oidc tests pass.

- [ ] **Step 10: Docs, verify, commit**

In `docs/hub.md`'s SSO section replace any sentence saying the signature is not checked with: "The hub verifies the ID token's signature against the provider's published keys (`jwks_uri` from discovery, on the issuer's host; RS256, PS256 or ES256). It re-reads the keys hourly, and once more when a token names a key it does not hold." Run `scripts/verify.sh`.

```bash
git -C <wt> add crates/fleet-core/Cargo.toml Cargo.lock crates/fleet-core/src/mcp/oidc.rs crates/fleet-core/src/mcp/oidc docs/hub.md
git -C <wt> commit -m "fix(sso): verify the ID token's signature against the issuer's JWKS"
```

---

### Task 2: People get a Keycloak profile (migration 173)

**Files:**
- Create: `crates/fleet-core/migrations/173_people_profile.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (guard fn `people_has_uuid`, MIGRATIONS entry, `Store::migrate` backfill call)
- Modify: `crates/fleet-core/src/store/people.rs` (`PersonRow`, `PERSON_COLUMNS`, `map_person`, `create_person`, `mint_personal_owner`)
- Test: `crates/fleet-core/src/store/schema/tests_upgrade.rs`, `crates/fleet-core/src/store/people.rs` tests

**Interfaces:**
- Produces: `PersonRow { …, uuid: String, email: Option<String>, email_verified: bool, first_name: Option<String>, last_name: Option<String>, attributes: serde_json::Value /* object of string arrays */ }`; `Store::update_person_profile(&self, id: i64, p: &PersonProfile) -> Result<PersonRow, IpcError>` with `pub struct PersonProfile { pub email: Option<Option<String>>, pub email_verified: Option<bool>, pub first_name: Option<Option<String>>, pub last_name: Option<Option<String>>, pub attributes: Option<serde_json::Value> }` (outer `None` = unchanged, `Some(None)` = clear).

- [ ] **Step 1: Failing upgrade test**

```rust
#[test]
fn migration_173_gives_every_person_a_uuid_and_an_empty_profile() {
    let s = store_at_version(172);
    s.conn.execute_batch(
        "INSERT INTO people (name, is_personal_owner, created_at) VALUES ('ada', 0, 1);",
    ).unwrap();
    s.migrate().unwrap();
    let ada = s.get_person_by_name("ada").unwrap().unwrap();
    assert_eq!(ada.uuid.len(), 36, "a v4 uuid: {}", ada.uuid);
    assert_eq!(ada.email, None);
    assert!(!ada.email_verified);
    assert_eq!(ada.attributes, serde_json::json!({}));
    let owner = s.get_person(s.personal_owner_id().unwrap().unwrap()).unwrap().unwrap();
    assert_ne!(owner.uuid, ada.uuid);
    // Re-running keeps the uuids.
    s.conn.execute_batch("DELETE FROM schema_version WHERE version >= 173;").unwrap();
    s.migrate().unwrap();
    assert_eq!(s.get_person_by_name("ada").unwrap().unwrap().uuid, ada.uuid);
}
```

Run: `cargo fleet-test -- migration_173` → FAIL (no field `uuid`).

- [ ] **Step 2: Migration**

```sql
-- Identity sub-project 1 (spec docs/superpowers/specs/2026-10-11-identity-realm-model-design.md §1):
-- a person is a Keycloak user. `uuid` is the user's `id` in a realm export
-- and in tokens the hub will issue; the integer id stays internal. It is
-- filled by `Store::backfill_person_uuids` (a v4 uuid needs the uuid crate),
-- which runs on every open, so a row inserted by an older binary gets one too.
-- attributes  JSON object of string arrays, Keycloak's user attributes.
ALTER TABLE people ADD COLUMN uuid TEXT;
ALTER TABLE people ADD COLUMN email TEXT;
ALTER TABLE people ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
ALTER TABLE people ADD COLUMN first_name TEXT;
ALTER TABLE people ADD COLUMN last_name TEXT;
ALTER TABLE people ADD COLUMN attributes TEXT NOT NULL DEFAULT '{}';
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_uuid ON people(uuid) WHERE uuid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_live_email
  ON people(lower(email)) WHERE email IS NOT NULL AND disabled_at IS NULL;
INSERT OR IGNORE INTO schema_version (version) VALUES (173);
```

In `schema.rs`: `fn people_has_uuid(conn: &Connection) -> rusqlite::Result<bool>` modelled on `client_tokens_has_trusted_at` (:131) with `pragma_table_info('people') WHERE name = 'uuid'`; entry `Migration { version: 173, sql: include_str!("../../migrations/173_people_profile.sql"), already_applied: Some(people_has_uuid) }`.

- [ ] **Step 3: Backfill + row**

In `Store::migrate()` after `self.backfill_session_owner()?;`:

```rust
self.backfill_person_uuids()?;
```

```rust
/// Migration 173's backfill: every person without a uuid gets a v4 one.
fn backfill_person_uuids(&self) -> Result<usize> {
    let ids: Vec<i64> = self.conn
        .prepare("SELECT id FROM people WHERE uuid IS NULL")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_>>()?;
    for id in &ids {
        self.conn.execute(
            "UPDATE people SET uuid = ?1 WHERE id = ?2 AND uuid IS NULL",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), id],
        )?;
    }
    Ok(ids.len())
}
```

`PersonRow` gains the six fields (serde: `email`, `first_name`, `last_name` skip if `None`; `attributes` a `serde_json::Value`). `map_person` reads `attributes` as TEXT and parses it (`serde_json::from_str(&t).unwrap_or_else(|_| json!({}))`); `uuid` as `Option<String>` mapped with `unwrap_or_default()`. `create_person` and `mint_personal_owner` insert `uuid = Uuid::new_v4()`.

- [ ] **Step 4: `update_person_profile` (test first)**

```rust
#[test]
fn a_profile_update_validates_email_and_attributes() {
    let s = Store::open_in_memory().unwrap();
    let p = s.create_person("ada", None).unwrap();
    let r = s.update_person_profile(p.id, &PersonProfile {
        email: Some(Some("Ada@Example.com".into())), first_name: Some(Some("Ada".into())),
        attributes: Some(serde_json::json!({"team": ["core"]})), ..Default::default()
    }).unwrap();
    assert_eq!(r.email.as_deref(), Some("Ada@Example.com"));
    assert_eq!(r.attributes["team"][0], "core");
    let bad = |pp: PersonProfile| s.update_person_profile(p.id, &pp).unwrap_err().code;
    assert_eq!(bad(PersonProfile { email: Some(Some("no-at".into())), ..Default::default() }), codes::E_VALIDATE);
    assert_eq!(bad(PersonProfile { attributes: Some(serde_json::json!({"k": "not-an-array"})), ..Default::default() }), codes::E_VALIDATE);
    let other = s.create_person("bob", None).unwrap();
    assert_eq!(s.update_person_profile(other.id, &PersonProfile {
        email: Some(Some("ada@example.com".into())), ..Default::default() }).unwrap_err().code, codes::E_EXISTS);
}
```

Implement: email must contain exactly one `@` with non-empty sides, no `breaks_a_line`, ≤ 254 chars; names ≤ 128 chars, no `breaks_a_line`; attributes must be an object whose values are arrays of strings, ≤ 64 keys, each value ≤ 1024 chars; a live-email collision → `E_EXISTS`. `#[derive(Default)]` on `PersonProfile`.

- [ ] **Step 5: Run, verify, commit**

Run: `cargo fleet-test -- people` and `cargo fleet-test -- migration_173` → pass; `scripts/verify.sh`.

```bash
git -C <wt> add crates/fleet-core/migrations/173_people_profile.sql crates/fleet-core/src/store
git -C <wt> commit -m "feat(people): a person carries a Keycloak user profile and uuid"
```

---

### Task 3: Realm tables, seed and backfill (migration 174)

**Files:**
- Create: `crates/fleet-core/migrations/174_realm_model.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS `Migration::plain(174, …)`; extend `backfill_person_uuids` into `backfill_realm_uuids` covering `roles`, `groups`, `clients`)
- Test: `crates/fleet-core/src/store/schema/tests_upgrade.rs`

**Interfaces:**
- Produces tables (exact columns below) that Tasks 4–13 read.

- [ ] **Step 1: Failing upgrade test**

```rust
#[test]
fn migration_174_turns_org_members_into_managed_groups_and_roles() {
    let s = store_at_version(173);
    s.conn.execute_batch("
        INSERT INTO people (id, name, is_personal_owner, created_at) VALUES (10,'ada',0,1),(11,'bob',0,1),(12,'cy',0,1);
        INSERT INTO orgs (id, name, isolate_sessions, created_at) VALUES (1,'acme',0,1);
        INSERT INTO org_members (org_id, person_id, role, added_at, added_by, shares_since, removed_at) VALUES
          (1,10,'admin',5,NULL,5,NULL),(1,11,'viewer',6,10,NULL,NULL),(1,12,'member',7,NULL,7,9);
        INSERT INTO client_tokens (name, token_sha256, mode, created_at, trusted_at, person_id) VALUES ('ada-mac','x','full',1,2,10);
    ").unwrap();
    s.migrate().unwrap();
    let q = |sql: &str| -> Vec<String> {
        s.conn.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert_eq!(q("SELECT name FROM roles WHERE client_id IS NULL ORDER BY name"),
               ["assets-admin", "fleet-admin", "fleet-trusted"]);
    assert_eq!(q("SELECT r.name FROM roles r JOIN clients c ON c.id = r.client_id WHERE c.client_id='fleet' ORDER BY r.name"),
               ["org-admin", "org-member", "org-viewer"]);
    // Live members land in their subgroup; the removed one does not.
    assert_eq!(q("SELECT p.name || ':' || g.name FROM group_members m JOIN groups g ON g.id=m.group_id
                  JOIN people p ON p.id=m.person_id ORDER BY p.name"), ["ada:admins", "bob:viewers"]);
    assert_eq!(q("SELECT g.name FROM groups g JOIN groups o ON o.id=g.parent_id WHERE o.org_id=1 ORDER BY g.name"),
               ["admins", "members", "viewers"]);
    // The owner is fleet-admin; ada (trusted device) is fleet-trusted.
    let owner = s.personal_owner_id().unwrap().unwrap();
    assert_eq!(q(&format!("SELECT r.name FROM user_role_mappings u JOIN roles r ON r.id=u.role_id WHERE u.person_id={owner}")), ["fleet-admin"]);
    assert_eq!(q("SELECT r.name FROM user_role_mappings u JOIN roles r ON r.id=u.role_id WHERE u.person_id=10"), ["fleet-trusted"]);
    // Every new row has a uuid; re-running adds nothing.
    let no_uuid: i64 = s.conn.query_row("SELECT COUNT(*) FROM groups WHERE uuid IS NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(no_uuid, 0);
    let before = q("SELECT COUNT(*) || '/' || (SELECT COUNT(*) FROM group_members) FROM groups");
    s.conn.execute_batch("DELETE FROM schema_version WHERE version >= 174;").unwrap();
    s.migrate().unwrap();
    assert_eq!(q("SELECT COUNT(*) || '/' || (SELECT COUNT(*) FROM group_members) FROM groups"), before);
}
```

Run: `cargo fleet-test -- migration_174` → FAIL (no table `roles`).

- [ ] **Step 2: Migration SQL**

```sql
-- Identity sub-project 1 (spec docs/superpowers/specs/2026-10-11-identity-realm-model-design.md §1-2):
-- the realm model. Groups and role mappings are the source of truth; from
-- here on `org_members` is a projection of them, written only by
-- `Store::reproject_org_memberships` (store/realm.rs).
--
-- No foreign key to people or orgs (the migration-066 rationale): a row
-- pointing at a removed id fails closed. Keys between the new tables do
-- cascade, because removing a role or a group removes what it grants.
-- `uuid` columns are filled by `Store::backfill_realm_uuids`.

CREATE TABLE IF NOT EXISTS clients (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT UNIQUE,
  client_id  TEXT NOT NULL UNIQUE,
  name       TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS roles (
  id          INTEGER PRIMARY KEY,
  uuid        TEXT UNIQUE,
  client_id   INTEGER REFERENCES clients(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  description TEXT,
  seeded      INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_roles_name ON roles(ifnull(client_id, 0), name);

CREATE TABLE IF NOT EXISTS role_composites (
  parent_id INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  child_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (parent_id, child_id),
  CHECK (parent_id <> child_id)
);

-- org_id  set on an org's root group `/orgs/<org>` only.
-- managed 1 for `/orgs`, every org root and its three subgroups: their
--         structure follows the org; only membership is edited.
CREATE TABLE IF NOT EXISTS groups (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT UNIQUE,
  name       TEXT NOT NULL,
  parent_id  INTEGER REFERENCES groups(id) ON DELETE CASCADE,
  attributes TEXT NOT NULL DEFAULT '{}',
  managed    INTEGER NOT NULL DEFAULT 0,
  org_id     INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_name ON groups(ifnull(parent_id, 0), name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_org ON groups(org_id) WHERE org_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS group_members (
  group_id  INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL,
  added_at  INTEGER NOT NULL,
  added_by  INTEGER,
  PRIMARY KEY (group_id, person_id)
);
CREATE INDEX IF NOT EXISTS idx_group_members_person ON group_members(person_id);

CREATE TABLE IF NOT EXISTS user_role_mappings (
  person_id INTEGER NOT NULL,
  role_id   INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (person_id, role_id)
);

CREATE TABLE IF NOT EXISTS group_role_mappings (
  group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  role_id  INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, role_id)
);
```

Then the `auth_epoch` trigger trio (`AFTER INSERT` / `AFTER UPDATE` / `AFTER DELETE … BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;`) for each of `roles`, `role_composites`, `groups`, `group_members`, `user_role_mappings`, `group_role_mappings`, named `auth_epoch_<table>_<insert|update|delete>`.

Then the seed and backfill (all idempotent):

```sql
INSERT OR IGNORE INTO clients (client_id, name, created_at) VALUES ('fleet', 'claude-fleet', strftime('%s','now'));

INSERT OR IGNORE INTO roles (client_id, name, description, seeded, created_at) VALUES
  (NULL, 'fleet-admin',   'Administers the whole fleet', 1, strftime('%s','now')),
  (NULL, 'fleet-trusted', 'Their full devices may change settings and administer', 1, strftime('%s','now')),
  (NULL, 'assets-admin',  'Manages the personal asset catalog', 1, strftime('%s','now'));
INSERT OR IGNORE INTO roles (client_id, name, description, seeded, created_at)
  SELECT c.id, v.name, v.descr, 1, strftime('%s','now')
  FROM clients c, (SELECT 'org-admin' AS name, 'Administers an org' AS descr
                   UNION ALL SELECT 'org-member', 'Works in an org'
                   UNION ALL SELECT 'org-viewer', 'Reads an org') v
  WHERE c.client_id = 'fleet';
INSERT OR IGNORE INTO role_composites (parent_id, child_id)
  SELECT p.id, ch.id FROM roles p JOIN roles ch ON ch.client_id = p.client_id
  JOIN clients c ON c.id = p.client_id AND c.client_id = 'fleet'
  WHERE (p.name, ch.name) IN (VALUES ('org-admin','org-member'), ('org-member','org-viewer'));

INSERT OR IGNORE INTO groups (name, parent_id, managed, created_at) VALUES ('orgs', NULL, 1, strftime('%s','now'));
INSERT OR IGNORE INTO groups (name, parent_id, managed, org_id, created_at)
  SELECT o.name, (SELECT id FROM groups WHERE parent_id IS NULL AND name = 'orgs'), 1, o.id, strftime('%s','now')
  FROM orgs o;
INSERT OR IGNORE INTO groups (name, parent_id, managed, created_at)
  SELECT s.name, g.id, 1, strftime('%s','now')
  FROM groups g, (SELECT 'admins' AS name UNION ALL SELECT 'members' UNION ALL SELECT 'viewers') s
  WHERE g.org_id IS NOT NULL;
INSERT OR IGNORE INTO group_role_mappings (group_id, role_id)
  SELECT sg.id, r.id FROM groups sg JOIN groups root ON root.id = sg.parent_id AND root.org_id IS NOT NULL
  JOIN roles r ON r.name = CASE sg.name WHEN 'admins' THEN 'org-admin' WHEN 'members' THEN 'org-member' ELSE 'org-viewer' END
  JOIN clients c ON c.id = r.client_id AND c.client_id = 'fleet';

INSERT OR IGNORE INTO group_members (group_id, person_id, added_at, added_by)
  SELECT sg.id, m.person_id, m.added_at, m.added_by
  FROM org_members m JOIN groups root ON root.org_id = m.org_id
  JOIN groups sg ON sg.parent_id = root.id
   AND sg.name = CASE m.role WHEN 'admin' THEN 'admins' WHEN 'member' THEN 'members' ELSE 'viewers' END
  WHERE m.removed_at IS NULL;

INSERT OR IGNORE INTO user_role_mappings (person_id, role_id)
  SELECT p.id, r.id FROM people p, roles r
  WHERE p.is_personal_owner = 1 AND r.client_id IS NULL AND r.name = 'fleet-admin';
INSERT OR IGNORE INTO user_role_mappings (person_id, role_id)
  SELECT DISTINCT t.person_id, r.id FROM client_tokens t, roles r
  WHERE t.person_id IS NOT NULL AND t.revoked_at IS NULL AND t.trusted_at IS NOT NULL
    AND r.client_id IS NULL AND r.name = 'fleet-trusted';
INSERT OR IGNORE INTO user_role_mappings (person_id, role_id)
  SELECT DISTINCT t.person_id, r.id FROM client_tokens t, roles r
  WHERE t.person_id IS NOT NULL AND t.revoked_at IS NULL AND t.assets_admin_at IS NOT NULL
    AND r.client_id IS NULL AND r.name = 'assets-admin';

INSERT OR IGNORE INTO schema_version (version) VALUES (174);
```

Before writing the assets-admin backfill, read `Store::client_is_assets_admin` (`store/clients.rs:531`) and `store/catalog.rs:88`: if the authoritative flag is a grant row on the `personal` catalog rather than `assets_admin_at`, add a second `INSERT OR IGNORE … SELECT` from that grant table for live devices, so no current assets admin loses it. Add both cases to the Step 1 test.

- [ ] **Step 3: uuids, run, commit**

Rename `backfill_person_uuids` → `backfill_realm_uuids` and loop it over `["people", "roles", "groups", "clients"]` (table names are constants, not input). Run `cargo fleet-test -- migration_17` and `cargo fleet-test -- store::schema` → pass (including `migrations_are_contiguous_from_one` and `every_migration_records_its_own_version`). `scripts/verify.sh`.

```bash
git -C <wt> add crates/fleet-core/migrations/174_realm_model.sql crates/fleet-core/src/store
git -C <wt> commit -m "feat(realm): roles, groups and mappings, seeded from orgs and devices"
```

---

### Task 4: Realm store API and effective roles

**Files:**
- Create: `crates/fleet-core/src/store/realm.rs` (declare `mod realm;` in `store/mod.rs`, `pub use realm::*;`)
- Create: `crates/fleet-core/src/store/realm/tests.rs`

**Interfaces:**
- Produces (all on `impl Store`, all `Result<_, IpcError>`):
  - Rows: `pub struct RoleRow { pub id: i64, pub uuid: String, pub client: Option<String> /* client_id text */, pub name: String, pub description: Option<String>, pub seeded: bool }`, `pub struct GroupRow { pub id: i64, pub uuid: String, pub name: String, pub path: String, pub parent_id: Option<i64>, pub managed: bool, pub org_id: Option<i64>, pub attributes: serde_json::Value }`, `pub enum RoleRef<'a> { Realm(&'a str), Client(&'a str, &'a str) }`, `pub enum Principal { Person(i64), Group(i64) }`.
  - Roles: `list_roles(client: Option<&str>) -> Vec<RoleRow>`, `role(r: RoleRef) -> Option<RoleRow>`, `create_role(client: Option<&str>, name, description: Option<&str>) -> RoleRow`, `update_role(id, description: Option<&str>) -> RoleRow`, `delete_role(id) -> bool` (refuses `seeded`), `add_composite(parent: i64, child: i64)`, `remove_composite(parent, child) -> bool`.
  - Groups: `list_groups() -> Vec<GroupRow>`, `group(id) -> Option<GroupRow>`, `group_by_path(path) -> Option<GroupRow>`, `create_group(parent: Option<i64>, name, by: Option<i64>) -> GroupRow`, `rename_group(id, name) -> GroupRow`, `move_group(id, parent: Option<i64>) -> GroupRow`, `delete_group(id) -> bool`, `group_members(id) -> Vec<i64>`, `groups_of(person) -> Vec<GroupRow>`.
  - Membership/mapping raw writers used by Task 5's wrappers: `add_group_member_in_tx(conn, group, person, by)`, `remove_group_member_in_tx(conn, group, person) -> bool`, `map_role(p: Principal, role: i64)`, `unmap_role(p: Principal, role: i64) -> bool`.
  - `effective_roles(person: i64) -> Vec<EffectiveRole>` with `pub struct EffectiveRole { pub role: RoleRow, pub via: Vec<String> /* "direct", "group:/orgs/acme/admins", "composite:org-admin" */ }`.
  - `people_with_realm_role(name: &str) -> Vec<i64>` (live people only).
  - Constants: `pub const CLIENT_FLEET: &str = "fleet"; pub const ROLE_FLEET_ADMIN: &str = "fleet-admin"; pub const ROLE_FLEET_TRUSTED: &str = "fleet-trusted"; pub const ROLE_ASSETS_ADMIN: &str = "assets-admin"; pub const ORG_ROLES: [(&str, &str); 3] = [("admins", "org-admin"), ("members", "org-member"), ("viewers", "org-viewer")];`

- [ ] **Step 1: Failing tests**

```rust
use super::*;
use crate::store::Store;
use crate::ipc_error::codes;

fn s() -> Store { Store::open_in_memory().unwrap() }

#[test]
fn a_group_role_reaches_members_of_its_descendants_and_composites_expand() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let eng = s.create_group(None, "eng", None).unwrap();
    let core = s.create_group(Some(eng.id), "core", None).unwrap();
    assert_eq!(core.path, "/eng/core");
    let deploy = s.create_role(None, "deployer", None).unwrap();
    let read = s.create_role(None, "reader", None).unwrap();
    s.add_composite(deploy.id, read.id).unwrap();
    s.map_role(Principal::Group(eng.id), deploy.id).unwrap();
    s.with_tx(|c| s.add_group_member_in_tx(c, core.id, ada, None)).unwrap();
    let names: Vec<String> = s.effective_roles(ada).unwrap().into_iter().map(|e| e.role.name).collect();
    assert!(names.contains(&"deployer".into()) && names.contains(&"reader".into()), "{names:?}");
    assert_eq!(s.people_with_realm_role("reader").unwrap(), vec![ada]);
}

#[test]
fn a_composite_cycle_is_refused() {
    let s = s();
    let a = s.create_role(None, "a", None).unwrap();
    let b = s.create_role(None, "b", None).unwrap();
    s.add_composite(a.id, b.id).unwrap();
    assert_eq!(s.add_composite(b.id, a.id).unwrap_err().code, codes::E_VALIDATE);
}

#[test]
fn a_disabled_person_has_no_effective_roles() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let r = s.role(RoleRef::Realm(ROLE_FLEET_TRUSTED)).unwrap().unwrap();
    s.map_role(Principal::Person(ada), r.id).unwrap();
    s.disable_person(ada).unwrap();
    assert!(s.effective_roles(ada).unwrap().is_empty());
    assert!(s.people_with_realm_role(ROLE_FLEET_TRUSTED).unwrap().is_empty());
}

#[test]
fn seeded_roles_and_managed_groups_cannot_be_deleted_or_restructured() {
    let s = s();
    let admin = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert_eq!(s.delete_role(admin.id).unwrap_err().code, codes::E_FORBIDDEN);
    let orgs = s.group_by_path("/orgs").unwrap().unwrap();
    assert_eq!(s.delete_group(orgs.id).unwrap_err().code, codes::E_FORBIDDEN);
    assert_eq!(s.create_group(Some(orgs.id), "x", None).unwrap_err().code, codes::E_FORBIDDEN);
    assert_eq!(s.rename_group(orgs.id, "y").unwrap_err().code, codes::E_FORBIDDEN);
}

#[test]
fn org_roles_cannot_be_mapped_outside_an_orgs_managed_groups() {
    let s = s();
    let ada = s.create_person("ada", None).unwrap().id;
    let org_admin = s.role(RoleRef::Client(CLIENT_FLEET, "org-admin")).unwrap().unwrap();
    assert_eq!(s.map_role(Principal::Person(ada), org_admin.id).unwrap_err().code, codes::E_VALIDATE);
    let g = s.create_group(None, "eng", None).unwrap();
    assert_eq!(s.map_role(Principal::Group(g.id), org_admin.id).unwrap_err().code, codes::E_VALIDATE);
}

#[test]
fn a_group_cannot_move_under_itself() {
    let s = s();
    let a = s.create_group(None, "a", None).unwrap();
    let b = s.create_group(Some(a.id), "b", None).unwrap();
    assert_eq!(s.move_group(a.id, Some(b.id)).unwrap_err().code, codes::E_VALIDATE);
}

#[test]
fn every_realm_write_moves_the_auth_epoch() {
    let s = s();
    let e0 = s.auth_epoch().unwrap();
    s.create_group(None, "eng", None).unwrap();
    assert!(s.auth_epoch().unwrap() > e0);
}
```

Run: `cargo fleet-test -- store::realm` → FAIL (module missing).

- [ ] **Step 2: Implement**

Key queries:

```rust
const EFFECTIVE_ROLE_IDS: &str = "
WITH RECURSIVE
  g(id) AS (SELECT group_id FROM group_members WHERE person_id = ?1
            UNION SELECT gr.parent_id FROM groups gr JOIN g ON gr.id = g.id WHERE gr.parent_id IS NOT NULL),
  r(id) AS (SELECT role_id FROM user_role_mappings WHERE person_id = ?1
            UNION SELECT role_id FROM group_role_mappings WHERE group_id IN (SELECT id FROM g)
            UNION SELECT rc.child_id FROM role_composites rc JOIN r ON rc.parent_id = r.id)
SELECT id FROM r
WHERE EXISTS (SELECT 1 FROM people p WHERE p.id = ?1 AND p.disabled_at IS NULL)";

const PEOPLE_WITH_REALM_ROLE: &str = "
WITH RECURSIVE
  holders(role_id) AS (SELECT id FROM roles WHERE client_id IS NULL AND name = ?1
                       UNION SELECT rc.parent_id FROM role_composites rc JOIN holders h ON rc.child_id = h.role_id),
  grp(id) AS (SELECT group_id FROM group_role_mappings WHERE role_id IN (SELECT role_id FROM holders)
              UNION SELECT c.id FROM groups c JOIN grp ON c.parent_id = grp.id)
SELECT p.id FROM people p WHERE p.disabled_at IS NULL AND (
  EXISTS (SELECT 1 FROM user_role_mappings u WHERE u.person_id = p.id AND u.role_id IN (SELECT role_id FROM holders))
  OR EXISTS (SELECT 1 FROM group_members m WHERE m.person_id = p.id AND m.group_id IN (SELECT id FROM grp)))
ORDER BY p.id";
```

(`UNION`, not `UNION ALL`, so a cycle written by hand still terminates.)

- `effective_roles` runs `EFFECTIVE_ROLE_IDS`, loads the `RoleRow`s, and fills `via` with a second pass: `"direct"` for `user_role_mappings` rows, `"group:<path>"` for each group of the person (and ancestor) mapping it, `"composite:<parent name>"` for a role reached only through a composite.
- Paths: `fn group_path_in_tx(conn, id) -> rusqlite::Result<String>` walks `parent_id` (bounded at 64 levels → `E_VALIDATE` "group nesting too deep").
- Cycle check for `add_composite`: refuse when `child` already reaches `parent` (`WITH RECURSIVE d(id) AS (SELECT ?child UNION SELECT rc.child_id FROM role_composites rc JOIN d ON rc.parent_id = d.id) SELECT EXISTS(SELECT 1 FROM d WHERE id = ?parent)`).
- `move_group`: refuse when the new parent is the group or one of its descendants.
- Managed groups (`managed = 1`): `rename_group`, `move_group`, `delete_group`, `create_group(parent = managed)` → `E_FORBIDDEN` "managed by its org"; only Task 5's org wrappers create / rename / delete them (through `_in_tx` helpers that skip the check).
- `map_role`: when the role is a `fleet` client role named `org-*`, refuse (`E_VALIDATE`, "org roles are granted through the org's group `/orgs/<org>`") unless called by Task 5's seeding helper (`map_role_in_tx(conn, p, role, allow_org_roles: bool)`).
- Names: reuse the person-name validation rules (trim, non-empty, ≤ 64 chars, no `breaks_a_line`, no `/` for groups).
- `Store::with_tx<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<T, IpcError>) -> Result<T, IpcError>`: `let tx = self.immediate_transaction()?; let v = f(&tx)?; tx.commit()?; Ok(v)` — add to `store/mod.rs` if no equivalent exists (search first: `grep -n "fn with_tx\|fn in_tx" crates/fleet-core/src/store`).

- [ ] **Step 3: Run, verify, commit**

Run: `cargo fleet-test -- store::realm` → 7 passed. `scripts/verify.sh`.

```bash
git -C <wt> add crates/fleet-core/src/store
git -C <wt> commit -m "feat(realm): store API for roles, groups, mappings and effective roles"
```

---

### Task 5: `org_members` becomes a projection of the org groups

**Files:**
- Modify: `crates/fleet-core/src/store/org_members.rs` (`set_org_member` :166, `remove_org_member` :238)
- Modify: `crates/fleet-core/src/store/orgs.rs` (`add_org` :375, `update_org` :404, `remove_org` :610 incl. :650)
- Modify: `crates/fleet-core/src/store/people.rs` (`disable_person` :319)
- Modify: `crates/fleet-core/src/store/realm.rs` (projection)
- Test: `crates/fleet-core/src/store/realm/tests.rs`

**Interfaces:**
- Consumes: Task 4's API.
- Produces: `fn reproject_org_memberships_in_tx(conn: &Connection, person: i64, by: Option<i64>) -> Result<(), IpcError>`; `fn ensure_org_groups_in_tx(conn, org_id: i64, org_name: &str) -> Result<i64 /* root group id */, IpcError>`; `Store::org_roles_from_groups(person: i64) -> Vec<(i64 /*org*/, String /*role*/)>` (the direct computation the projection test compares against). Public signatures of `set_org_member`, `remove_org_member`, `add_org`, `update_org`, `remove_org`, `disable_person` are unchanged.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn set_org_member_moves_the_person_between_the_orgs_subgroups() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "viewer", None).unwrap();
    s.set_org_member(org.id, ada, "admin", None).unwrap();
    let paths: Vec<String> = s.groups_of(ada).unwrap().into_iter().map(|g| g.path).collect();
    assert_eq!(paths, ["/orgs/acme/admins"]);
    let m = s.org_member(org.id, ada).unwrap().unwrap();
    assert_eq!(m.role, "admin");
    assert!(m.shares_since.is_some(), "viewer → admin starts receiving shares");
}

#[test]
fn the_projection_never_widens_an_existing_grant() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    let since = s.org_member(org.id, ada).unwrap().unwrap().shares_since;
    s.set_org_member(org.id, ada, "admin", None).unwrap();
    assert_eq!(s.org_member(org.id, ada).unwrap().unwrap().shares_since, since, "member → admin keeps shares_since");
    s.remove_org_member(org.id, ada).unwrap();
    assert!(s.org_member(org.id, ada).unwrap().unwrap().removed_at.is_some());
    assert!(s.groups_of(ada).unwrap().is_empty());
}

#[test]
fn org_rename_and_removal_follow_into_the_groups() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    assert!(s.group_by_path("/orgs/acme/members").unwrap().is_some());
    s.update_org(org.id, Some("acme2"), None, None).unwrap();
    assert!(s.group_by_path("/orgs/acme2/members").unwrap().is_some());
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    s.remove_org(org.id).unwrap();
    assert!(s.group_by_path("/orgs/acme2").unwrap().is_none());
    assert!(s.org_member(org.id, ada).unwrap().unwrap().removed_at.is_some());
}

#[test]
fn disabling_a_person_ends_every_group_membership_and_mapping() {
    let s = s();
    let org = s.add_org("acme", None, false).unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.set_org_member(org.id, ada, "member", None).unwrap();
    let t = s.role(RoleRef::Realm(ROLE_FLEET_TRUSTED)).unwrap().unwrap();
    s.map_role(Principal::Person(ada), t.id).unwrap();
    s.disable_person(ada).unwrap();
    assert!(s.groups_of(ada).unwrap().is_empty());
    assert!(s.org_member(org.id, ada).unwrap().unwrap().removed_at.is_some());
    let mapped: i64 = s.conn.query_row("SELECT COUNT(*) FROM user_role_mappings WHERE person_id = ?1", [ada], |r| r.get(0)).unwrap();
    assert_eq!(mapped, 0);
}

/// Random membership writes: the projected `org_members` always equals the
/// direct computation from the groups.
#[test]
fn the_projection_equals_the_direct_computation() {
    use rand::{Rng, SeedableRng};
    let s = s();
    let orgs: Vec<i64> = (0..3).map(|i| s.add_org(&format!("o{i}"), None, false).unwrap().id).collect();
    let people: Vec<i64> = (0..4).map(|i| s.create_person(&format!("p{i}"), None).unwrap().id).collect();
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    for _ in 0..300 {
        let (o, p) = (orgs[rng.random_range(0..3)], people[rng.random_range(0..4)]);
        match rng.random_range(0..4) {
            0 => { s.remove_org_member(o, p).unwrap(); }
            n => { s.set_org_member(o, p, ["admin", "member", "viewer"][n - 1], None).unwrap(); }
        }
        for &p in &people {
            let mut direct = s.org_roles_from_groups(p).unwrap();
            let mut projected: Vec<(i64, String)> = s.memberships_of(p).unwrap().into_iter()
                .filter(|m| m.removed_at.is_none()).map(|m| (m.org_id, m.role)).collect();
            direct.sort(); projected.sort();
            assert_eq!(direct, projected);
        }
    }
}

/// Nothing but the projection writes org_members.
#[test]
fn only_the_projection_writes_org_members() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "rs") { out.push(p) }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let writes = ["INSERT INTO org_members", "UPDATE org_members", "DELETE FROM org_members"];
    let mut offenders = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "store/realm.rs" || rel.contains("tests") { continue; }
        let text = std::fs::read_to_string(&f).unwrap();
        // Production code only: drop everything from the first `#[cfg(test)]`.
        let prod = text.split("#[cfg(test)]").next().unwrap();
        if writes.iter().any(|w| prod.contains(w)) { offenders.push(rel); }
    }
    assert!(offenders.is_empty(), "org_members written outside the projection: {offenders:?}");
}
```

(The file is read at run time, not with `include_str!`, per CLAUDE.md.) Check `rand`'s API version in fleet-core (`rand = "0.10"`): use `random_range` if it exists there, else `gen_range`.

Run: `cargo fleet-test -- store::realm` → the new tests FAIL.

- [ ] **Step 2: Implement**

1. Move the bodies of today's `set_org_member` / `remove_org_member` into private `fn write_org_member_row_in_tx(conn, org, person, role, by)` / `fn end_org_member_row_in_tx(conn, org, person) -> bool` in `realm.rs` (they keep the `shares_since` / re-join / `bump_grant_generation` logic verbatim; `bump_grant_generation` runs after commit in the public wrappers, as today).
2. `org_roles_from_groups`:

```sql
SELECT root.org_id, r.name FROM group_members gm
JOIN groups sg ON sg.id = gm.group_id AND sg.managed = 1
JOIN groups root ON root.id = sg.parent_id AND root.org_id IS NOT NULL
JOIN group_role_mappings m ON m.group_id = sg.id
JOIN roles r ON r.id = m.role_id
WHERE gm.person_id = ?1
  AND EXISTS (SELECT 1 FROM people p WHERE p.id = ?1 AND p.disabled_at IS NULL)
```

then per org keep the highest of `org-admin > org-member > org-viewer`, mapped back to `admin | member | viewer`.
3. `reproject_org_memberships_in_tx(conn, person, by)`: desired = `org_roles_from_groups`; current = live `org_members` rows of the person; for each desired (org, role) whose current role differs or is absent → `write_org_member_row_in_tx`; for each current org not desired → `end_org_member_row_in_tx`.
4. `set_org_member(org, person, role, added_by)`: validate role; `with_tx`: `ensure_org_groups_in_tx`, remove the person from the org's three subgroups, add to the subgroup for `role`, `reproject_org_memberships_in_tx(person, added_by)`; return `org_member(org, person)`. `remove_org_member`: remove from the three subgroups, reproject; return whether a live row ended.
5. `add_org`: wrap the insert and `ensure_org_groups_in_tx(conn, id, &name)` in one transaction. `ensure_org_groups_in_tx` creates `/orgs` if absent, the root (`org_id = id`, `managed = 1`), the three subgroups and their `group_role_mappings` (via `map_role_in_tx(…, allow_org_roles = true)`), and is idempotent. `update_org` with a new name renames the root group in the same transaction. `remove_org`: inside its existing transaction, collect the org's members, delete the root group (cascades subgroups, memberships and mappings), keep its existing `UPDATE org_members SET removed_at …` (:650) — it is the projection of "no groups", so move that statement into `realm.rs` as `end_all_org_member_rows_in_tx(conn, org)` and call it from there.
6. `disable_person`: inside its transaction add `DELETE FROM group_members WHERE person_id = ?1`, `DELETE FROM user_role_mappings WHERE person_id = ?1`, then `reproject_org_memberships_in_tx(conn, id, None)`.

- [ ] **Step 3: Run the old and new org tests**

Run: `cargo fleet-test -- org_members` , `cargo fleet-test -- store::realm`, `cargo fleet-test -- org_admin`, `cargo fleet-test -- session_grants`, `cargo fleet-test -- orgs`
Expected: all pass — existing org / grant behaviour is unchanged.

- [ ] **Step 4: Full suite for the module, verify, commit**

Run: `scripts/verify.sh` (it runs the touched modules' tests).

```bash
git -C <wt> add crates/fleet-core/src/store
git -C <wt> commit -m "feat(realm): org membership is a projection of the org's groups"
```

PR 2 is Tasks 2–5. Before opening it: `scripts/verify.sh full`.

---

### Task 6: `fleet-admin` decides fleet-wide administration

**Files:**
- Modify: `crates/fleet-core/src/mcp/auth.rs` (`Caller` :134-190, `Caller::master` :198, new `apply_realm`)
- Modify: `crates/fleet-core/src/mcp/token_cache.rs` (`AuthRows` :55, the reload)
- Modify: `crates/fleet-core/src/store/read_pool.rs` (`auth_snapshot` :124)
- Modify: `crates/fleet-core/src/mcp/mod.rs` (`authorize` :236-300 and the fallback rows :270-276, the `/hook?token=` path :304)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`access_allows` :159, :178)
- Modify authority call sites (list below)
- Modify: `crates/fleet-core/src/store/realm.rs` (`unmap_role` / `remove_group_member` refuse removing `fleet-admin` from the personal owner or the last holder); `crates/fleet-core/src/store/people.rs` (`mint_personal_owner` :442 maps `fleet-admin`)
- Test: `crates/fleet-core/src/mcp/auth.rs` tests, `crates/fleet-core/src/mcp/tools/tests.rs`

**Interfaces:**
- Produces: `Caller.is_fleet_admin: bool`; `pub struct RealmSets { pub fleet_admins: Arc<BTreeSet<i64>>, pub trusted: Arc<BTreeSet<i64>>, pub assets_admins: Arc<BTreeSet<i64>> }` (in `token_cache.rs`, `Default`); `AuthRows.realm: RealmSets`; `pub fn apply_realm(caller: &mut Caller, realm: &RealmSets)`; `Store::realm_sets(&self) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>), IpcError>`; `Store::is_fleet_admin(person: i64) -> Result<bool, IpcError>`.

- [ ] **Step 1: Failing tests**

In `auth.rs` tests:

```rust
#[test]
fn a_second_persons_device_is_fleet_admin_only_with_the_role() {
    let mut c = resolve_token("tok-phone", "s3cret", &[], &[client_row("tok-phone", Some(22))], OWNER).unwrap();
    assert!(!c.is_fleet_admin);
    apply_realm(&mut c, &RealmSets { fleet_admins: Arc::new([22].into()), ..Default::default() });
    assert!(c.is_fleet_admin);
}

#[test]
fn the_master_is_always_fleet_admin() {
    let mut c = resolve_token("s3cret", "s3cret", &[], &[], OWNER).unwrap();
    apply_realm(&mut c, &RealmSets::default());
    assert!(c.is_fleet_admin);
}
```

(`client_row(token, person)` — use the test module's existing row builder; add the `person_id` parameter if it has none.)

In `mcp/tools/tests.rs`, next to `org_admin_reaches_a_persons_full_device_only` (:19409), a test that pairs a second person's trusted full device, grants that person `fleet-admin` with `map_role`, and asserts `org_admin { action: "list_orgs" }` now answers with `Authority::Fleet` reach (e.g. sees an org it is not a member of) and that a `Person`-access tool (`set_setting` read path) is visible to it; and that removing the role takes both away on the next call.

In `store/realm/tests.rs`:

```rust
#[test]
fn the_owner_and_the_last_fleet_admin_keep_the_role() {
    let s = s();
    let owner = s.personal_owner_id().unwrap().unwrap();
    let fa = s.role(RoleRef::Realm(ROLE_FLEET_ADMIN)).unwrap().unwrap();
    assert_eq!(s.unmap_role(Principal::Person(owner), fa.id).unwrap_err().code, codes::E_FORBIDDEN);
    assert!(s.is_fleet_admin(owner).unwrap());
}
```

Run: `cargo fleet-test -- mcp::auth` → FAIL (no field `is_fleet_admin`).

- [ ] **Step 2: Implement the plumbing**

1. `Caller` gains `pub is_fleet_admin: bool` (doc: "has the effective realm role `fleet-admin`, or is the master / an admin-scoped control token; the AUTHORITY half of what `is_personal_owner` meant before identity sub-project 1 — `is_personal_owner` keeps the identity half: whose sessions an unscoped caller acts as"). `Caller::master()` sets it `true`. Every `Caller { … }` literal (the compiler lists them) sets it to the same value it sets `is_personal_owner`, except host tokens (`false`).
2. `apply_realm`:

```rust
/// Decide the realm-role half of a resolved caller (identity sub-project 1).
/// Run on every resolved caller before any gate reads it.
pub fn apply_realm(caller: &mut Caller, realm: &RealmSets) {
    if let Some(p) = caller.client.as_ref().and_then(|c| c.person_id) {
        caller.is_fleet_admin = realm.fleet_admins.contains(&p);
    }
}
```

(Task 7 extends it with trust.)
3. `Store::realm_sets` runs `people_with_realm_role` for the three role names. The personal owner is added to `fleet_admins` unconditionally, so an owner can never be locked out by a missing mapping.
4. `auth_snapshot` returns the three vectors in the same read transaction as the token rows; `TokenCache` stores them in `AuthRows.realm` (they change only with the epoch — every realm table has the trigger trio). The writer fallback in `authorize` fills `realm` from `s.realm_sets().unwrap_or_default()` (empty = fail closed).
5. In `authorize`, right after `auth::check_request(…)` succeeds (and after the `/hook?token=` fallback resolves), call `auth::apply_realm(&mut caller, &rows.realm)`. Find every other resolver with `graft callers check_request` and `graft callers resolve_token` (events route, report route, agent router) and apply it there too.
6. `mint_personal_owner` maps `fleet-admin` to the new owner in its transaction.
7. `unmap_role` / `remove_group_member_in_tx` / `disable_person`: refuse (`E_FORBIDDEN`) when the change would leave the personal owner without `fleet-admin` or leave `people_with_realm_role("fleet-admin")` empty (owner's union excluded from that count).

- [ ] **Step 3: Switch the authority gates**

Replace the AUTHORITY uses of `is_personal_owner` / `personal_owner_id()`; keep the IDENTITY uses.

| Site | Today | After |
|---|---|---|
| `mcp/guard.rs:159` (`Access::Person`) | `caller.is_personal_owner && (…)` | `caller.is_fleet_admin && (…)` |
| `mcp/guard.rs:178` (`Access::PersonDevice`) | `caller.is_personal_owner && caller.is_person_device()` | `caller.is_fleet_admin && caller.is_person_device()` |
| `mcp/tools/fleet.rs:1124` (org_admin authority) | `caller.is_personal_owner && caller.is_person_device()` | `caller.is_fleet_admin && caller.is_person_device()` |
| `mcp/tools/updates.rs:239` (update_policy) | same | same change |
| `mcp/tools/fleet.rs:377` (quick_replies set) | `caller.is_personal_owner && caller.is_person_device()` | `caller.is_fleet_admin && …` |
| `mcp/tools/api_tokens.rs:63` (`token_minter`) | `… && is_personal_owner && …` | `… && is_fleet_admin && …` |
| `service/orgs.rs:1175` (`AdminView::for_caller`) | `is_person_device() && is_personal_owner` | `is_person_device() && is_fleet_admin` |
| `service/orgs.rs:1189` (owner on an org-bound device admins that org) | owner | `is_fleet_admin` |
| `service/org_admin.rs:361` (`authority_for`: owner = admin of any org) | `personal_owner_id() == p` | `s.is_fleet_admin(p)?` |
| `service/org_admin.rs:738` (`require_org_device`: owner's device off-limits to an org admin) | owner | any fleet admin's device |
| `service/org_admin.rs:810` (`require_other_member`) | owner | any fleet admin |
| `service/org_admin.rs:1286` (BindDevice: owner may bind anywhere) | owner | fleet admin |
| `mcp/tools/fleet.rs:1215` (pair_device refuses the owner as a member) | owner | fleet admin |

Read each of `service/org_admin.rs:507, 604, 892, 963`, `service/orgs.rs:1646`, `fleet-hub/src/person.rs:204, 259`: they are display flags (`owner: …`) or the unclaimed-sessions reach. Keep display flags as the owner; switch `unclaimed_reach` (:507) to fleet admin. The identity uses (`auth.rs:379` view scope, `owner_for`, `owner_name`, `hub_personal_owner` and its callers, routines, missions, resume, session_grants, org_members:374/405, schema backfill) stay on the personal owner — do not touch them.

- [ ] **Step 4: Run**

Run: `cargo fleet-test -- mcp::` , `cargo fleet-test -- org_admin`, `cargo fleet-test -- service::orgs`, `cargo fleet-test -- store::realm`
Expected: all pass, including the existing owner-gate tests (the owner holds `fleet-admin`).

- [ ] **Step 5: Verify, commit**

`scripts/verify.sh`.

```bash
git -C <wt> add -A crates/fleet-core crates/fleet-hub
git -C <wt> commit -m "feat(realm): the fleet-admin role, not the single owner, administers the fleet"
```

---

### Task 7: Trust and assets-admin belong to the person

**Files:**
- Modify: `crates/fleet-core/src/mcp/auth.rs` (`apply_realm`; `ClientRef.trusted` doc :85)
- Modify: `crates/fleet-core/src/store/clients.rs` (`set_client_trust` :247, `set_client_assets_admin` :481, `client_is_assets_admin` :531)
- Modify: every writer of `trusted_at` outside `set_client_trust` (find: `grep -rn "trusted_at" crates/fleet-core/src --include=*.rs | grep -v "tests\|//"`; expected: pairing `mcp/pairing.rs:620/656/668`, `fleet-hub/src/pair.rs:577`)
- Test: `crates/fleet-core/src/store/clients.rs` tests, `crates/fleet-core/src/mcp/auth.rs` tests

**Interfaces:**
- Consumes: `RealmSets.trusted`, `RealmSets.assets_admins`, `ROLE_FLEET_TRUSTED`, `ROLE_ASSETS_ADMIN`, `map_role` / `unmap_role`.
- Produces: unchanged public signatures of `set_client_trust(name, on) -> ClientTokenRow` and `set_client_assets_admin(name, on) -> ClientTokenRow`; new semantics.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn trusting_a_device_trusts_every_full_device_of_its_person() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    let mac = pair_test_device(&s, "ada-mac", "full", Some(ada));
    let phone = pair_test_device(&s, "ada-phone", "full", Some(ada));
    let ro = pair_test_device(&s, "ada-ro", "readonly", Some(ada));
    s.set_client_trust("ada-mac", true).unwrap();
    let realm = s.realm_sets().unwrap();
    assert!(realm.1.contains(&ada));
    for (tok, trusted) in [(mac, true), (phone, true), (ro, false)] {
        let mut c = caller_for(&s, &tok);
        crate::mcp::auth::apply_realm(&mut c, &realm_sets_of(&s));
        assert_eq!(c.client.unwrap().trusted, trusted, "{tok}");
    }
}

#[test]
fn a_device_with_no_person_is_never_trusted() {
    let s = Store::open_in_memory().unwrap();
    pair_test_device(&s, "orphan", "full", None);
    assert_eq!(s.set_client_trust("orphan", true).unwrap_err().code, codes::E_VALIDATE);
}

#[test]
fn assets_admin_follows_the_person_and_the_eligible_device() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    pair_test_device(&s, "ada-mac", "full", Some(ada));
    let ro = pair_test_device_id(&s, "ada-ro", "readonly", Some(ada));
    s.set_client_assets_admin("ada-mac", true).unwrap();
    assert!(s.people_with_realm_role(ROLE_ASSETS_ADMIN).unwrap().contains(&ada));
    assert!(!s.client_is_assets_admin(ro).unwrap(), "readonly is not eligible");
}
```

Use the helpers the clients tests already have for pairing a device and resolving a caller; add `pair_test_device(&Store, name, mode, person) -> String /* token */`, `pair_test_device_id`, `caller_for` and `realm_sets_of` (→ `RealmSets` from `s.realm_sets()`) to that test module if missing.

Run: `cargo fleet-test -- store::clients` → FAIL.

- [ ] **Step 2: Implement**

1. `apply_realm` adds:

```rust
if let Some(c) = caller.client.as_mut() {
    c.trusted = c.person_id.is_some_and(|p| realm.trusted.contains(&p)) && caller.mode == TokenMode::Full;
}
```

(Order: compute `mode` before borrowing `client` mutably.) Machine tokens (peer, updater) already have no person and stay untrusted, as today.
2. `set_client_trust(name, on)`: load the row; refuse peer / updater as today; `person_id` `None` → `E_VALIDATE` "the device belongs to no person"; `on` → `map_role(Person(p), fleet-trusted)`, `off` → `unmap_role`; also keep writing `trusted_at` (COALESCE / NULL) so the column mirrors the person for one release; return the reloaded row.
3. `set_client_assets_admin(name, on)`: same shape for `assets-admin`; keep the existing grant-row / mirror-column write for one release.
4. `client_is_assets_admin(id)`: keep the eligibility check (`LIVE_GRANT_ELIGIBLE`: full, not revoked, no org), then `person ∈ people_with_realm_role(assets-admin)`; other catalogs' grants (`set_client_catalog_grant`) are untouched.
5. Pairing a `trusted` device (`mcp/pairing.rs:620/656/668`, `fleet-hub/src/pair.rs:577`): after the row exists, call `set_client_trust(name, true)` instead of writing `trusted_at` directly.
6. The `no lock-out` rule in `service/org_admin.rs` ("the device in use cannot be untrusted") becomes "the caller cannot remove `fleet-trusted` from their own person": find the check next to `SetDeviceTrust` (:1242) and compare persons instead of devices.

- [ ] **Step 3: Run**

Run: `cargo fleet-test -- store::clients`, `cargo fleet-test -- mcp::`, `cargo fleet-test -- org_admin`, `cargo fleet-test -- assets`, `cargo fleet-test -- pairing`
Expected: pass. If an existing test asserts that trusting one device leaves the person's other full device untrusted, update it to the new rule and say so in the commit body.

- [ ] **Step 4: Verify, commit; PR 3**

`scripts/verify.sh`, then `scripts/verify.sh full` before PR 3.

```bash
git -C <wt> commit -am "feat(realm)!: trust and assets-admin are the person's, granted as realm roles

BREAKING CHANGE: trusting a device now trusts every full device of its person."
```

---

### Task 8: Audit log, re-enable, retention (migration 175)

**Files:**
- Create: `crates/fleet-core/migrations/175_admin_events.sql`
- Create: `crates/fleet-core/src/store/admin_events.rs`
- Modify: `crates/fleet-core/src/store/schema.rs`, `store/mod.rs`
- Modify: `crates/fleet-core/src/store/people.rs` (new `enable_person`)
- Modify: `crates/fleet-core/src/service/org_admin.rs` (`run` :1010 records each successful write; new action `enable_person`)
- Modify: `crates/fleet-core/src/mcp/oidc.rs` (`resolve_person` :567-637 records provision / link)
- Modify: `crates/fleet-hub/src/person.rs` (records; new `Enable { name }` subcommand)
- Modify: `crates/fleet-core/src/service/gc.rs` (`sweep_with` :421-470), `crates/fleet-core/src/service/settings.rs` (SPECS row)
- Test: `store/admin_events.rs` tests, `store/people.rs` tests, `service/gc.rs` tests

**Interfaces:**
- Produces: `pub struct AdminEvent<'a> { pub actor_person: Option<i64>, pub actor_device: Option<i64>, pub action: &'a str, pub resource_type: &'a str, pub resource_id: &'a str, pub representation: serde_json::Value }`; `Store::record_admin_event(&self, ev: &AdminEvent) -> Result<i64, IpcError>`; `pub(crate) fn record_admin_event_in_tx(conn, ev) -> Result<i64, IpcError>`; `Store::admin_events(&self, q: &AdminEventQuery) -> Result<Vec<AdminEventRow>, IpcError>` with `pub struct AdminEventQuery { pub since: Option<i64>, pub resource_type: Option<String>, pub resource_id: Option<String>, pub actor_person: Option<i64>, pub limit: u32 /* ≤ 500 */ }`; `Store::sweep_admin_events(&self, older_than: i64) -> Result<usize, IpcError>`; `Store::enable_person(&self, id: i64) -> Result<PersonRow, IpcError>`; `pub const AUDIT_RETENTION_DAYS: &str = "auth.audit_retention_days";`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn admin_events_are_append_only_except_for_the_sweep() {
    let s = Store::open_in_memory().unwrap();
    let id = s.record_admin_event(&AdminEvent { actor_person: None, actor_device: None, action: "create",
        resource_type: "group", resource_id: "/eng", representation: serde_json::json!({"name":"eng"}) }).unwrap();
    assert!(s.conn.execute("UPDATE admin_events SET action = 'x' WHERE id = ?1", [id]).is_err());
    assert!(s.conn.execute("DELETE FROM admin_events WHERE id = ?1", [id]).is_err());
    assert_eq!(s.sweep_admin_events(i64::MAX).unwrap(), 1);
}

#[test]
fn a_representation_never_carries_a_secret() {
    let s = Store::open_in_memory().unwrap();
    s.record_admin_event(&AdminEvent { actor_person: None, actor_device: None, action: "pair",
        resource_type: "device", resource_id: "ada-mac",
        representation: serde_json::json!({"name":"ada-mac","code":"ABCD1234","secret":"x","token":"y"}) }).unwrap();
    let rep = &s.admin_events(&AdminEventQuery { limit: 10, ..Default::default() }).unwrap()[0].representation;
    for k in ["code", "secret", "token"] { assert!(rep.get(k).is_none(), "{k} leaked: {rep}"); }
}

#[test]
fn enable_person_restores_sign_in_but_not_devices() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap().id;
    s.disable_person(ada).unwrap();
    let p = s.enable_person(ada).unwrap();
    assert!(p.disabled_at.is_none());
    assert!(s.groups_of(ada).unwrap().is_empty());
    let bob = s.create_person("bob", None).unwrap().id;
    s.disable_person(bob).unwrap();
    s.create_person("bob", None).unwrap();
    assert_eq!(s.enable_person(bob).unwrap_err().code, codes::E_EXISTS, "the name is taken by a live person");
}
```

And in `service/org_admin.rs` tests: after `set_member` through `run`, `admin_events` holds one row with `resource_type = "org_member"`, `actor_person` = the caller.

Run: `cargo fleet-test -- admin_events` → FAIL.

- [ ] **Step 2: Migration 175**

```sql
-- Identity sub-project 1 (spec §3): Keycloak-style admin events, and the
-- auth-epoch triggers person_identities never had.
--
-- admin_events is append-only: UPDATE always aborts; DELETE aborts unless
-- the retention sweep has opened `admin_events_gc` inside its own
-- transaction (Store::sweep_admin_events), so nothing else can rewrite
-- history through the store.
CREATE TABLE IF NOT EXISTS admin_events (
  id              INTEGER PRIMARY KEY,
  ts              INTEGER NOT NULL,
  actor_person_id INTEGER,
  actor_device_id INTEGER,
  action          TEXT NOT NULL,
  resource_type   TEXT NOT NULL,
  resource_id     TEXT NOT NULL,
  representation  TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_admin_events_ts ON admin_events(ts);
CREATE INDEX IF NOT EXISTS idx_admin_events_resource ON admin_events(resource_type, resource_id);

CREATE TABLE IF NOT EXISTS admin_events_gc (id INTEGER PRIMARY KEY CHECK (id = 1), open INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO admin_events_gc (id, open) VALUES (1, 0);

CREATE TRIGGER IF NOT EXISTS admin_events_no_update BEFORE UPDATE ON admin_events
BEGIN SELECT RAISE(ABORT, 'admin_events is append-only'); END;
CREATE TRIGGER IF NOT EXISTS admin_events_no_delete BEFORE DELETE ON admin_events
WHEN (SELECT open FROM admin_events_gc WHERE id = 1) = 0
BEGIN SELECT RAISE(ABORT, 'admin_events is append-only'); END;
```

plus the `auth_epoch_person_identities_{insert,update,delete}` trio and `INSERT OR IGNORE INTO schema_version (version) VALUES (175);`. Register `Migration::plain(175, …)`.

- [ ] **Step 3: Implement**

- `record_admin_event_in_tx` serialises `representation` after `scrub`: drop keys `code`, `secret`, `token`, `token_sha256`, `password`, `client_secret`, `api_key`, `value` recursively; truncate the JSON text to 4096 chars.
- `sweep_admin_events(older_than)`: `with_tx`: `UPDATE admin_events_gc SET open = 1`, `DELETE FROM admin_events WHERE ts < ?1`, `UPDATE admin_events_gc SET open = 0`, commit.
- `enable_person`: `UPDATE people SET disabled_at = NULL WHERE id = ?1 AND disabled_at IS NOT NULL`; unique-index violation → `E_EXISTS`; not found → `E_NOTFOUND`; already live → return the row.
- Record events (actor from `Me` / the caller; operator CLI = `None`):
  - `service::org_admin::run`: after every successful non-read action, one event: `action` = the action name, `resource_type` ∈ `org | org_rule | device | person | org_member | grant | project | hub`, `resource_id` = the name or id acted on, `representation` = `serde_json::to_value(args)` (scrubbed).
  - `mcp::oidc::resolve_person`: `provision` (new person) and `login` is NOT an admin event; record only `provision` and the identity link.
  - `fleet-hub person rename|disable|enable|link-sso|unlink-sso`.
  - Task 10's `realm_admin` records its own.
- Settings row in `SPECS`, next to `DECIDE_RETENTION_DAYS`:

```rust
Spec::new(
    AUDIT_RETENTION_DAYS,
    "365",
    Kind::Int { min: 0, max: 3650 },
    "Audit: keep admin events",
    "Days an administrative change (who changed which person, group, role or device) is kept.",
)
.unit(Unit::Days)
.zero("forever"),
```

- GC: in `sweep_with` after `decide::sweep_runs`, `let days = settings::get_string(s, AUDIT_RETENTION_DAYS).parse::<i64>().unwrap_or(365); if days > 0 { report.admin_events = s.sweep_admin_events(now - days * 86_400)?; }` (add `admin_events: usize` to `GcReport`; take the lock the way `decide::sweep_runs` does).
- `org_admin` gains `enable_person` (fleet authority only, like `disable_person`); add it to the `action` doc string list in `OrgAdminArgs`.
- `fleet-hub person enable <name>`: `PersonCmd::Enable { name: String }` → `store.enable_person(id)`.

- [ ] **Step 4: Run, regenerate, verify, commit; PR 4**

Run: `cargo fleet-test -- admin_events`, `cargo fleet-test -- people`, `cargo fleet-test -- org_admin`, `cargo fleet-test -- service::gc`, `REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current` (fails on purpose), again without the variable; `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` (the `org_admin` action list changed), again without. `scripts/verify.sh full`.

```bash
git -C <wt> add -A crates docs
git -C <wt> commit -m "feat(realm): an append-only admin audit log, and re-enabling a person"
```

---

### Task 9: Device tokens expire; SSO no longer auto-provisions

**Files:**
- Create: `crates/fleet-core/migrations/176_device_token_ttl.sql` (number: next free)
- Modify: `crates/fleet-core/src/store/schema.rs`
- Modify: `crates/fleet-core/src/service/settings.rs` (two SPECS rows, key consts)
- Modify: `crates/fleet-core/src/mcp/token_cache.rs` (`AuthRows.device_policy`), `store/read_pool.rs` (`auth_snapshot`), `mcp/mod.rs` (`authorize` and fallback)
- Modify: `crates/fleet-core/src/mcp/auth.rs` (new `device_token_expired`)
- Modify: `crates/fleet-core/src/mcp/oidc.rs` (:187-190 default), `docs/hub.md` (:1081 row), `deploy/hub/fleet-hub.env.example` (:16-17)
- Test: `mcp/auth.rs` tests, `mcp/oidc/tests.rs`, `store/schema/tests_upgrade.rs`

**Interfaces:**
- Produces: `pub struct DeviceTokenPolicy { pub ttl_secs: Option<i64>, pub max_age_secs: Option<i64>, pub since: i64 }` (`Default`: all off); `pub fn device_token_expired(row: &ClientTokenRow, p: &DeviceTokenPolicy, now: i64) -> bool`; `Store::device_token_policy(&self) -> Result<DeviceTokenPolicy, IpcError>`; consts `DEVICE_TOKEN_TTL_DAYS = "auth.device_token_ttl_days"`, `DEVICE_TOKEN_MAX_AGE_DAYS = "auth.device_token_max_age_days"`, `DEVICE_TOKEN_TTL_SINCE = "auth.device_token_ttl_since"`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn an_idle_device_token_expires_but_the_upgrade_signs_nobody_out() {
    let p = DeviceTokenPolicy { ttl_secs: Some(90 * 86_400), max_age_secs: None, since: 1_000_000 };
    let mut row = client_row_full("t", Some(1));
    row.created_at = 10; row.last_seen_at = Some(20);
    // Idle since long before the upgrade: the clock starts at `since`.
    assert!(!device_token_expired(&row, &p, 1_000_000 + 89 * 86_400));
    assert!(device_token_expired(&row, &p, 1_000_000 + 91 * 86_400));
    row.last_seen_at = Some(1_000_000 + 80 * 86_400);
    assert!(!device_token_expired(&row, &p, 1_000_000 + 91 * 86_400), "used recently");
}

#[test]
fn max_age_counts_from_creation_and_machine_tokens_never_expire() {
    let p = DeviceTokenPolicy { ttl_secs: None, max_age_secs: Some(100), since: 0 };
    let mut row = client_row_full("t", Some(1));
    row.created_at = 0;
    assert!(device_token_expired(&row, &p, 101));
    row.mode = "peer".into();
    assert!(!device_token_expired(&row, &p, 101));
    row.mode = "updater".into();
    assert!(!device_token_expired(&row, &p, 101));
}
```

In `oidc/tests.rs`: `config()` without `ENV_AUTO_PROVISION` has `auto_provision == false`; an unlinked account whose name is free is refused with `Refusal::Forbidden` (adapt the existing auto-provision test to set `ENV_AUTO_PROVISION=true` explicitly).

Upgrade test: after migrating 175 → 176, `settings` holds `auth.device_token_ttl_since` ≈ now; re-running does not change it.

Run: `cargo fleet-test -- device_token_expired` → FAIL.

- [ ] **Step 2: Implement**

Migration:

```sql
-- Identity sub-project 1 (spec §3): device tokens expire. The idle clock of
-- every existing token starts now, so the upgrade signs nobody out.
INSERT OR IGNORE INTO settings (key, value) VALUES ('auth.device_token_ttl_since', CAST(strftime('%s','now') AS TEXT));
-- The expiry policy is part of what a token resolves to: changing it moves
-- the auth epoch, so the cached rows are re-read with the new policy.
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_insert AFTER INSERT ON settings
WHEN NEW.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_update AFTER UPDATE ON settings
WHEN NEW.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_delete AFTER DELETE ON settings
WHEN OLD.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
INSERT OR IGNORE INTO schema_version (version) VALUES (176);
```

```rust
pub fn device_token_expired(row: &ClientTokenRow, p: &DeviceTokenPolicy, now: i64) -> bool {
    if matches!(row.mode.as_str(), "peer" | "updater") {
        return false;
    }
    if p.max_age_secs.is_some_and(|max| now - row.created_at > max) {
        return true;
    }
    let anchor = row.last_seen_at.unwrap_or(0).max(row.created_at).max(p.since);
    p.ttl_secs.is_some_and(|ttl| now - anchor > ttl)
}
```

(Use the mode constants the store already defines for peer / updater if they exist — `grep -n "\"updater\"" crates/fleet-core/src/store/clients.rs`.)
`device_token_policy`: days from the two settings (0 → `None`), `since` from `auth.device_token_ttl_since` (missing → 0).
In `authorize`, after `apply_realm`: if `caller.client` is `Some(c)` and the matching row in `rows.clients` (by `c.id`) is expired under `rows.device_policy`, log `tracing::info!(device = %c.name, "[mcp] device token expired; pair again")` and return `Err(StatusCode::UNAUTHORIZED)`. Apply the same in every other resolver found in Task 6.
Two SPECS rows (Int 0..3650, `Unit::Days`, `.zero("never")` / `.zero("off")`), labels "Devices: sign out after idle days" and "Devices: sign out after days, used or not".
OIDC: `None => false` at oidc.rs:187-190; update the field doc (:89-90), `docs/hub.md:1081` (default `false`, and how to invite: `fleet-hub person link-sso`), and the env example lines 16-17.

- [ ] **Step 3: Run, regenerate, verify, commit; PR 5**

Run: `cargo fleet-test -- mcp::auth`, `cargo fleet-test -- mcp::oidc`, `cargo fleet-test -- migration_176`, `cargo fleet-test -- token_cache`; `REGEN_SETTINGS_DOCS=1 …` then again; `scripts/verify.sh full`.

```bash
git -C <wt> add -A crates deploy docs
git -C <wt> commit -m "feat(auth)!: device tokens expire after idle days; SSO no longer auto-provisions

BREAKING CHANGE: FLEET_HUB_OIDC_AUTO_PROVISION defaults to false."
```

---

### Task 10: The `realm_admin` service and MCP tool

**Files:**
- Create: `crates/fleet-core/src/service/realm_admin.rs` (+ `service/mod.rs` `pub mod realm_admin;`)
- Create: `crates/fleet-core/src/service/realm_admin/tests.rs`
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs` (new `#[tool]` fn after `org_admin` :1148)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (TOOL_POLICIES row after `org_admin` :576-588)
- Modify: `skills/claude-fleet-control/SKILL.md` (tool index `device` row :100-104), `docs/control-api.md` (prose after `org_admin` :1167+)
- Regenerate: `docs/control-api-reference.md`
- Test: `service/realm_admin/tests.rs`, `mcp/tools/tests.rs`

**Interfaces:**
- Consumes: Tasks 4–8; `org_admin::{authority_for, Authority, Me}`; `org_admin_writer`.
- Produces:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RealmAdminParams")]
pub struct RealmAdminArgs {
    /// list_users|get_user|update_user|enable_user|disable_user|list_groups|get_group|create_group|rename_group|move_group|delete_group|add_member|remove_member|list_roles|create_role|update_role|delete_role|add_composite|remove_composite|map_role|unmap_role|effective_roles|audit
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub user: Option<String>,      // person name
    #[serde(default, skip_serializing_if = "Option::is_none")] pub group: Option<String>,     // group path, e.g. /orgs/acme/members
    #[serde(default, skip_serializing_if = "Option::is_none")] pub parent: Option<String>,    // group path; "" = top level
    #[serde(default, skip_serializing_if = "Option::is_none")] pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub role: Option<String>,      // "fleet-admin" or "fleet:org-admin"
    #[serde(default, skip_serializing_if = "Option::is_none")] pub child: Option<String>,     // composite child role
    #[serde(default, skip_serializing_if = "Option::is_none")] pub client: Option<String>,    // create_role: client_id
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub profile: Option<serde_json::Value>, // update_user: email, email_verified, first_name, last_name, attributes
    #[serde(default, skip_serializing_if = "Option::is_none")] pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub limit: Option<u32>,
}
pub fn run(args: &RealmAdminArgs, store: &Mutex<Store>, me: Me<'_>) -> Result<serde_json::Value, IpcError>;
```

- [ ] **Step 1: Failing authorization-table test**

```rust
/// Every action × every kind of caller → allowed, or the exact refusal.
#[test]
fn who_may_do_what() {
    let (store, acme, other) = fixture(); // two orgs, people: root(fleet-admin), ann(admin of acme), mo(member of acme), oz(admin of other)
    let fleet = Me { device: Some("root-mac"), person: Some(person(&store, "root")), authority: Authority::Fleet };
    let acme_admin = Me { device: Some("ann-mac"), person: Some(person(&store, "ann")), authority: Authority::Org { org: acme, hub: false } };
    let other_admin = Me { device: Some("oz-mac"), person: Some(person(&store, "oz")), authority: Authority::Org { org: other, hub: false } };
    let add = |g: &str| RealmAdminArgs { action: "add_member".into(), group: Some(g.into()), user: Some("mo".into()), ..Default::default() };
    assert!(run(&add("/orgs/acme/viewers"), &store, fleet).is_ok());
    assert!(run(&add("/orgs/acme/admins"), &store, acme_admin).is_ok());
    assert_eq!(code(run(&add("/orgs/acme/admins"), &store, other_admin)), codes::E_FORBIDDEN);
    let create = RealmAdminArgs { action: "create_group".into(), name: Some("eng".into()), ..Default::default() };
    assert!(run(&create, &store, fleet).is_ok());
    assert_eq!(code(run(&create, &store, acme_admin)), codes::E_FORBIDDEN);
    let map = RealmAdminArgs { action: "map_role".into(), user: Some("mo".into()), role: Some("fleet-admin".into()), ..Default::default() };
    assert_eq!(code(run(&map, &store, acme_admin)), codes::E_FORBIDDEN);
    assert!(run(&map, &store, fleet).is_ok());
    let audit = RealmAdminArgs { action: "audit".into(), ..Default::default() };
    assert!(run(&audit, &store, fleet).is_ok());
    assert_eq!(code(run(&audit, &store, acme_admin)), codes::E_FORBIDDEN);
    // An org admin reads only their own org's groups.
    let list = RealmAdminArgs { action: "list_groups".into(), ..Default::default() };
    let groups = run(&list, &store, acme_admin).unwrap();
    assert!(groups.to_string().contains("/orgs/acme") && !groups.to_string().contains("/orgs/other"));
}

#[test]
fn nobody_removes_their_own_fleet_admin() {
    let (store, _, _) = fixture();
    let root = person(&store, "root");
    let fleet = Me { device: Some("root-mac"), person: Some(root), authority: Authority::Fleet };
    let unmap = RealmAdminArgs { action: "unmap_role".into(), user: Some("root".into()), role: Some("fleet-admin".into()), ..Default::default() };
    assert_eq!(code(run(&unmap, &store, fleet)), codes::E_FORBIDDEN);
}

#[test]
fn every_write_is_audited() {
    let (store, _, _) = fixture();
    let fleet = Me { device: Some("root-mac"), person: Some(person(&store, "root")), authority: Authority::Fleet };
    run(&RealmAdminArgs { action: "create_group".into(), name: Some("eng".into()), ..Default::default() }, &store, fleet).unwrap();
    let s = store.lock().unwrap();
    let ev = &s.admin_events(&AdminEventQuery { limit: 5, ..Default::default() }).unwrap()[0];
    assert_eq!((ev.action.as_str(), ev.resource_type.as_str(), ev.resource_id.as_str()), ("create_group", "group", "/eng"));
}
```

`fixture()` builds a `Mutex<Store>` with the orgs, people, memberships (through `set_org_member`) and `fleet-admin` for `root`; `person(&store, name) -> i64`; `code(r) -> &'static str` returns the `IpcError` code.

Run: `cargo fleet-test -- service::realm_admin` → FAIL.

- [ ] **Step 2: Implement the service**

- `Action::parse` (closed enum, unknown → `E_VALIDATE` listing the actions), `is_read()` for `list_*`, `get_*`, `effective_roles`, `audit`.
- Authority: `Authority::Fleet` → every action. `Authority::Org { org, .. }` → only `add_member` / `remove_member` whose group is one of `/orgs/<that org>/{admins,members,viewers}`, and reads narrowed to that org: `list_groups` returns its subtree, `get_group` inside it, `list_users` / `get_user` / `effective_roles` for its live members. Everything else → `E_FORBIDDEN` "a fleet admin's".
- `add_member` / `remove_member` on a managed org subgroup go through `set_org_member` / `remove_org_member` (role from the subgroup name), so the projection and grant rules apply; on other groups through `with_tx` + `add_group_member_in_tx` / `remove_group_member_in_tx`. An org admin cannot remove themselves from `admins` of their org (no lock-out).
- Role references: `"fleet-admin"` = realm role; `"<client>:<name>"` = client role.
- `disable_user` / `enable_user` call `disable_person` / `enable_person`; disabling yourself → `E_FORBIDDEN`.
- `audit`: `admin_events(AdminEventQuery { since, limit: limit.unwrap_or(100).min(500), .. })`.
- Every successful write records one admin event in the same transaction where the store call takes one, otherwise right after it: `action` = the action name, `resource_type` ∈ `user | group | role | role_mapping | composite`, `resource_id` = the user name / group path / role ref, `actor_person` = `me.person`, `actor_device` = the device id (look up by `me.device`).
- Output: compact JSON rows; never the `client_tokens` hash.

- [ ] **Step 3: The tool**

In `mcp/tools/fleet.rs`, after `org_admin`:

```rust
#[tool(description = "Realm: users, groups (tree, members), realm and \
    client roles (composites), role mappings, effective roles, the audit \
    log. Fleet admin's device: all; an org admin's: their org's groups' \
    members. Changes need a trusted full device.")]
pub(super) async fn realm_admin(
    &self,
    Extension(caller): Extension<Caller>,
    Parameters(args): Parameters<crate::service::realm_admin::RealmAdminArgs>,
) -> Result<CallToolResult, McpError> {
    use crate::service::org_admin::{self as oa, Me};
    use crate::service::realm_admin as ra;
    audit("realm_admin", &args.audit_summary());
    let action = ra::Action::parse(&args.action).map_err(to_mcp_err)?;
    if !action.is_read() {
        org_admin_writer(&caller)?;
    }
    let authority = {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        oa::authority_for(
            &s,
            caller.is_fleet_admin && caller.is_person_device(),
            caller.person(),
            caller.client.as_ref().and_then(|c| c.org_id),
        )
        .map_err(to_mcp_err)?
    }
    .ok_or_else(|| mcp_err("E_FORBIDDEN", "realm_admin is for a fleet admin's device, or an org admin's for their org", None))?;
    let me = Me { device: caller.client.as_ref().map(|c| c.name.as_str()), person: caller.person(), authority };
    let out = ra::run(&args, &self.store, me).map_err(to_mcp_err)?;
    ok_json_compact(&out)
}
```

`RealmAdminArgs::audit_summary` mirrors `OrgAdminArgs::audit_summary` (action + names, never `profile`). Policy row:

```rust
    // Identity sub-project 1. Not served to the master (it has
    // `fleet-hub realm`). The tool decides the authority
    // (`org_admin::authority_for`); a change needs a trusted full device.
    ToolPolicy {
        name: "realm_admin",
        access: Access::Device,
        readonly: false,
        confirm: false,
        deadline: Deadline::Quick,
    },
```

Add `realm_admin` to the `device` row of the skill's tool index; add a `### realm_admin` section to `docs/control-api.md` after `org_admin` (actions, authority, role-reference syntax, the org-role rule).

- [ ] **Step 4: Tool-surface tests**

In `mcp/tools/tests.rs`, modelled on `org_admin_reaches_a_persons_full_device_only` (:19409): `realm_admin` is served to a person's full device, not to the master, a host token or a readonly device for writes. Then run:

`cargo fleet-test -- service::realm_admin`, `cargo fleet-test -- mcp::tools`, `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` (fails on purpose), again without. `the_served_definition_budget_stays_bounded` must pass — if not, shorten the description, never raise the budget.

- [ ] **Step 5: Verify, commit**

`scripts/verify.sh`.

```bash
git -C <wt> add -A crates skills docs
git -C <wt> commit -m "feat(realm): realm_admin — users, groups, roles, mappings and audit over MCP"
```

---

### Task 11: `fleet-hub realm` CLI

**Files:**
- Create: `crates/fleet-hub/src/realm.rs`
- Modify: `crates/fleet-hub/src/main.rs` (`Cmd::Realm` beside `Person` :254, dispatch beside :548, parse test beside :759)
- Modify: `docs/hub.md` (new subsection "Groups, roles and the audit log" after "Companies: members and roles" :1488-1541)

**Interfaces:**
- Consumes: `service::realm_admin::{run, RealmAdminArgs}`, `org_admin::Me::LOCAL`.
- Produces: `fleet-hub realm user list|show <name>|update <name> [--email …] [--first-name …] [--last-name …]|enable <name>|disable <name>`, `realm group list|show <path>|create <name> [--parent <path>]|rename <path> <name>|move <path> [--parent <path>]|delete <path>|add <path> <user>|remove <path> <user>`, `realm role list|create <name> [--client <id>] [--description …]|delete <ref>|compose <parent> <child>|decompose <parent> <child>|map <ref> (--user <n> | --group <path>)|unmap …`, `realm effective <user>`, `realm audit [--since <unix>] [--limit N]`; every subcommand takes `--json`.

- [ ] **Step 1: Failing parse test**

```rust
#[test]
fn realm_subcommands_parse() {
    for argv in [
        &["fleet-hub", "realm", "group", "create", "eng", "--parent", "/orgs"][..],
        &["fleet-hub", "realm", "role", "map", "fleet-admin", "--user", "ada"],
        &["fleet-hub", "realm", "audit", "--limit", "20", "--json"],
        &["fleet-hub", "realm", "effective", "ada"],
    ] {
        Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    }
}
```

Run: `cargo fleet-test -- realm_subcommands_parse` → FAIL.

- [ ] **Step 2: Implement**

Same shape as `person.rs::run` (sync, `crate::serve::existing_db`, `crate::serve::open_store`): each subcommand builds a `RealmAdminArgs` and calls `realm_admin::run(&args, &Mutex::new(store), Me::LOCAL)` — the operator on the hub machine is the fleet authority, actor `None` in the audit log. Table output through `crate::pair::table` / `fmt_time`; `--json` prints `serde_json::to_string_pretty`. Module doc: like `person.rs`'s, why it writes `state.db` directly.

- [ ] **Step 3: Run, docs, commit**

Run: `cargo fleet-test -- realm_subcommands_parse`; add a short "Groups, roles and the audit log" subsection to `docs/hub.md` with three examples (create a group, map `fleet-admin`, read the audit log). `scripts/verify.sh`.

```bash
git -C <wt> add crates/fleet-hub docs/hub.md
git -C <wt> commit -m "feat(hub): fleet-hub realm — groups, roles, mappings and audit from the hub machine"
```

---

### Task 12: Desktop — groups and roles, read-only

**Files:**
- Modify: `crates/fleet-core/src/service/org_admin.rs` (`list_people` output: add `groups: Vec<String>` and `roles: Vec<EffectiveRoleSummary { name: String, client: Option<String>, via: Vec<String> }>`, both `#[serde(default)]`)
- Create: `crates/fleet-core/pages/settings.groups.json`; modify `crates/fleet-core/pages/settings.people.json`
- Modify: `crates/fleet-core/src/pages/mod.rs` (`PAGE_FILES`), `crates/fleet-core/src/pages/resources.rs` (`GROUP_RESOURCE`, `PERSON_RESOURCE` fields)
- Modify: `src-tauri/src/commands/org_devices.rs` (new `list_groups` command, `routed::list_groups`), `src-tauri/src/lib.rs` (:556 registration), `src-tauri/src/backend/verdicts.rs` (row beside :615-629), `src-tauri/src/backend/tests_routing.rs` (read case beside :2390)
- Modify: `src/lib/settings_tree.ts` (:102-110, add Groups under Organisations), `src/lib/devices.ts` (`PersonSummary` :47-63 gains `groups`, `roles`)
- Regenerate: hub verdicts, control-api reference, page docs, hub contract golden
- Test: `src-tauri/src/backend/tests_routing.rs`, `crates/fleet-core/src/pages/tests.rs`, `src/lib/*.test.ts` touched

**Interfaces:**
- Consumes: `realm_admin` (`list_groups`), `Store::groups_of`, `Store::effective_roles`.
- Produces: desktop command `list_groups() -> Vec<GroupSummary { path: String, name: String, managed: bool, members: Vec<String>, roles: Vec<String> }>` routed to the hub tool `realm_admin { action: "list_groups" }`, local otherwise.

- [ ] **Step 1: Failing tests**

- `tests_routing.rs`: add `("list_groups", json!({}))` to the read cases next to `list_people`, routed to `realm_admin`.
- `pages/tests.rs`: `every_page_file_is_compiled_in` and `pages::validate` cover the new spec automatically; add an assertion that `settings.groups` has resource `group` and no `create` / `update` / `delete`.

Run: `cargo fleet-test -- backend::` (with `TMPDIR=/dev/shm/fleet-tests` on Linux) → FAIL (unknown command).

- [ ] **Step 2: Implement**

1. `list_people` (org_admin) fills `groups` (paths from `groups_of`) and `roles` (from `effective_roles`).
2. `GROUP_RESOURCE` in `resources.rs`, modelled on `PERSON_RESOURCE` (:1208): `list: "list_groups"`, `create/update/delete: None`, fields `path`, `managed`, `members` (`FieldKind::Items`), `roles` (`FieldKind::Items`).
3. `settings.groups.json`:

```json
{ "spec": "fleet.page/1", "id": "settings.groups", "title": "Groups", "parent": "settings",
  "layout": "master_detail", "resource": "group",
  "intro": "Groups and the roles they grant. An org's groups (/orgs/<org>/admins, members, viewers) follow the org; change their members on the org's page or with realm_admin.",
  "sections": [
    { "title": "Group", "items": [ {"type":"field","key":"path"}, {"type":"field","key":"managed"} ] },
    { "title": "Grants", "items": [ {"type":"field","key":"roles"}, {"type":"field","key":"members"} ] } ] }
```

4. `settings.people.json` gains a section `{ "title": "Roles", "items": [ {"type":"field","key":"groups"}, {"type":"field","key":"roles"} ] }` and `PERSON_RESOURCE` the two fields (`roles` rendered as `name (via …)` — use the `Items` field's label template the resource system supports; if none, have `list_people` also emit `roles_display: Vec<String>` and show that).
5. Desktop command `list_groups` in `org_devices.rs` next to `list_people` (:281-287), `routed::list_groups` → `hub.route("list_groups", &args)` / local `realm_admin::run(…, Me::LOCAL)`; register in `lib.rs`; verdict `("list_groups", Verdict::Routed { tool: "realm_admin" })`.
6. Frontend: `settings_tree.ts` adds `settings.groups` after `settings.people`; `PersonSummary` gains `groups: string[]` and `roles: { name: string; client?: string; via: string[] }[]`.

- [ ] **Step 3: Regenerate and run**

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
cargo fleet-test -- verdict_gen
REGEN_DOCS=1 cargo fleet-test -- reference_is_current
cargo fleet-test -- reference_is_current
REGEN_PAGE_DOCS=1 cargo fleet-test -- page_docs_are_current
cargo fleet-test -- page_docs_are_current
REGEN_HUB_CONTRACT=1 cargo fleet-test -- contract
cargo fleet-test -- contract
cargo fleet-test -- backend::
pnpm check
pnpm exec vitest related --run src/lib/settings_tree.ts src/lib/devices.ts
```

Each REGEN run fails on purpose; the plain re-run must pass. The new `list_people` fields have `#[serde(default)]`, so an older hub's answer still parses (no `CONTRACT_REVISION` bump).

- [ ] **Step 4: Verify, commit**

`scripts/verify.sh`.

```bash
git -C <wt> add -A crates src-tauri src docs
git -C <wt> commit -m "feat(desktop): read-only groups page and a person's groups and effective roles"
```

---

### Task 13: Docs, status and the full run

**Files:**
- Modify: `docs/hub.md` ("People and devices" :1417-1487: roles replace the single owner for administration; trust per person; token expiry; re-enable; audit), `docs/status.md` (:147-153: identity sub-project 1 landed; SSO signature verified; sub-projects 2–6 pending), `docs/architecture.md` (one paragraph: realm tables are the source of truth, `org_members` is a projection, `RealmSets` on `AuthRows`)

- [ ] **Step 1: Write the docs** (facts only, matching the code as merged in Tasks 1–12; no future tense about sub-projects beyond one line pointing to the spec's roadmap).

- [ ] **Step 2: Full validation**

```bash
scripts/verify.sh full
```

Expected: exit 0. A known flake (`service::add_project`, fleet-agent `conn::tests::report_frames_stay_under_the_frame_cap_and_carry_the_rest_over`) is re-run alone before blaming the change.

- [ ] **Step 3: Commit; PR 6**

```bash
git -C <wt> add docs
git -C <wt> commit -m "docs: the realm model, per-person trust, device-token expiry and the audit log"
```
