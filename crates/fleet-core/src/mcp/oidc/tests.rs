use super::*;
use crate::net::https::{FakeTransport, Response as HttpResponse};

const ISS: &str = "https://sso.example.com/realms/acme";
const HUB: &str = "https://fleet.example.com";

fn config() -> OidcConfig {
    OidcConfig::from_lookup(|k| match k {
        ENV_ISSUER => Some(format!("{ISS}/")),
        ENV_CLIENT_ID => Some("fleet-hub".into()),
        ENV_CLIENT_SECRET => Some("shh".into()),
        _ => None,
    })
    .unwrap()
    .unwrap()
}

fn discovery() -> serde_json::Value {
    serde_json::json!({
        "issuer": ISS,
        "authorization_endpoint": format!("{ISS}/protocol/openid-connect/auth"),
        "token_endpoint": format!("{ISS}/protocol/openid-connect/token"),
    })
}

fn fake_with_discovery() -> FakeTransport {
    let fake = FakeTransport::new();
    fake.always(
        Method::Get,
        "/.well-known/openid-configuration",
        Ok(HttpResponse::json(200, &discovery())),
    );
    fake
}

fn jwt(claims: &serde_json::Value) -> String {
    let b = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "{}.{}.{}",
        b(br#"{"alg":"RS256","typ":"JWT"}"#),
        b(claims.to_string().as_bytes()),
        b(b"sig")
    )
}

fn query_param(url: &str, key: &str) -> String {
    let q = url.split_once('?').unwrap().1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .unwrap()
        .to_string()
}

fn id_claims(nonce: &str) -> serde_json::Value {
    serde_json::json!({
        "iss": ISS,
        "aud": "fleet-hub",
        "azp": "fleet-hub",
        "sub": "0b7d-uuid",
        "exp": unix_now() + 300,
        "iat": unix_now(),
        "nonce": nonce,
        "preferred_username": "ada",
        "name": "Ada Lovelace",
    })
}

/// A provider that answers the token request with `id` (and a Keycloak
/// access token carrying `realm_roles`).
fn token_answer(fake: &FakeTransport, id: &serde_json::Value, realm_roles: &[&str]) {
    let access = serde_json::json!({ "realm_access": { "roles": realm_roles } });
    fake.once(
        Method::Post,
        "/protocol/openid-connect/token",
        Ok(HttpResponse::json(
            200,
            &serde_json::json!({
                "id_token": jwt(id),
                "access_token": jwt(&access),
                "token_type": "Bearer",
            }),
        )),
    );
}

#[test]
fn no_issuer_means_sign_on_is_off_and_a_half_config_is_an_error() {
    assert_eq!(OidcConfig::from_lookup(|_| None).unwrap(), None);
    let err = OidcConfig::from_lookup(|k| (k == ENV_ISSUER).then(|| ISS.to_string())).unwrap_err();
    assert!(err.contains(ENV_CLIENT_ID), "{err}");
    let plain = OidcConfig::from_lookup(|k| match k {
        ENV_ISSUER => Some("http://sso.example.com/realms/acme".into()),
        ENV_CLIENT_ID => Some("c".into()),
        _ => None,
    })
    .unwrap_err();
    assert!(plain.contains("https://"), "{plain}");
}

#[test]
fn the_config_defaults_and_never_prints_its_secret() {
    let c = config();
    assert_eq!(c.issuer, ISS, "the trailing slash is dropped");
    assert!(c.auto_provision);
    assert_eq!(c.mode, "full");
    assert_eq!(c.username_claim, "preferred_username");
    assert_eq!(c.scopes, "openid profile email");
    assert!(!format!("{c:?}").contains("shh"));
    let custom = OidcConfig::from_lookup(|k| match k {
        ENV_ISSUER => Some(ISS.into()),
        ENV_CLIENT_ID => Some("c".into()),
        ENV_ALLOWED_ROLES => Some(" fleet-users , /ops ,".into()),
        ENV_AUTO_PROVISION => Some("false".into()),
        ENV_MODE => Some("readonly".into()),
        ENV_SCOPES => Some("profile".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    assert_eq!(custom.allowed_roles, vec!["fleet-users", "/ops"]);
    assert!(!custom.auto_provision);
    assert_eq!(custom.mode, "readonly");
    assert_eq!(custom.scopes, "openid profile");
    assert!(OidcConfig::from_lookup(|k| match k {
        ENV_ISSUER => Some(ISS.into()),
        ENV_CLIENT_ID => Some("c".into()),
        ENV_MODE => Some("peer".into()),
        _ => None,
    })
    .is_err());
}

#[tokio::test]
async fn begin_redirects_with_pkce_and_finish_checks_the_id_token() {
    let fake = fake_with_discovery();
    let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
    let redirect = format!("{HUB}/auth/oidc/callback");
    let (url, state) = oidc
        .begin(&redirect, Some("ada-phone".into()))
        .await
        .unwrap();
    assert!(url.starts_with(&format!("{ISS}/protocol/openid-connect/auth?")));
    assert_eq!(query_param(&url, "client_id"), "fleet-hub");
    assert_eq!(query_param(&url, "state"), state);
    assert_eq!(query_param(&url, "code_challenge_method"), "S256");
    assert_eq!(
        query_param(&url, "redirect_uri"),
        "https%3A%2F%2Ffleet.example.com%2Fauth%2Foidc%2Fcallback"
    );
    let nonce = query_param(&url, "nonce");

    token_answer(&fake, &id_claims(&nonce), &["fleet-users"]);
    let who = oidc.finish(&redirect, "the-code", &state).await.unwrap();
    assert_eq!(who.subject, "0b7d-uuid");
    assert_eq!(who.username.as_deref(), Some("ada"));
    assert_eq!(who.display_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(who.roles, vec!["fleet-users"]);
    assert_eq!(who.device_name.as_deref(), Some("ada-phone"));

    // The exchange carried the verifier whose S256 is the challenge, and
    // the confidential client's secret.
    let sent = fake
        .requests()
        .into_iter()
        .find(|r| r.method == Method::Post)
        .unwrap();
    let body = String::from_utf8(sent.body.unwrap()).unwrap();
    let verifier = body
        .split('&')
        .find_map(|kv| kv.strip_prefix("code_verifier="))
        .unwrap();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(<sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes()));
    assert_eq!(query_param(&url, "code_challenge"), challenge);
    assert!(body.contains("client_secret=shh"));
    assert!(body.contains("grant_type=authorization_code"));
    // Discovery was fetched once and cached.
    assert_eq!(fake.count("/.well-known/openid-configuration"), 1);

    // A state works once.
    let again = oidc
        .finish(&redirect, "the-code", &state)
        .await
        .unwrap_err();
    assert!(matches!(again, Refusal::Invalid(_)));
}

type Tamper = Box<dyn Fn(&mut serde_json::Value)>;

#[tokio::test]
async fn an_id_token_that_does_not_check_out_is_refused() {
    let redirect = format!("{HUB}/auth/oidc/callback");
    let cases: Vec<(&str, Tamper)> = vec![
        (
            "another issuer",
            Box::new(|c| c["iss"] = "https://evil.example.com".into()),
        ),
        (
            "another audience",
            Box::new(|c| c["aud"] = "someone-else".into()),
        ),
        (
            "another azp",
            Box::new(|c| c["azp"] = "someone-else".into()),
        ),
        (
            "expired",
            Box::new(|c| c["exp"] = (unix_now() - 3600).into()),
        ),
        ("a wrong nonce", Box::new(|c| c["nonce"] = "nope".into())),
        (
            "no nonce",
            Box::new(|c| {
                c.as_object_mut().unwrap().remove("nonce");
            }),
        ),
    ];
    for (what, tamper) in cases {
        let fake = fake_with_discovery();
        let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
        let (url, state) = oidc.begin(&redirect, None).await.unwrap();
        let mut claims = id_claims(&query_param(&url, "nonce"));
        tamper(&mut claims);
        token_answer(&fake, &claims, &[]);
        let err = oidc.finish(&redirect, "c", &state).await.unwrap_err();
        assert!(matches!(err, Refusal::Invalid(_)), "{what}: {err:?}");
    }
}

#[tokio::test]
async fn an_audience_list_naming_this_client_is_accepted() {
    let fake = fake_with_discovery();
    let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
    let redirect = format!("{HUB}/auth/oidc/callback");
    let (url, state) = oidc.begin(&redirect, None).await.unwrap();
    let mut claims = id_claims(&query_param(&url, "nonce"));
    claims["aud"] = serde_json::json!(["account", "fleet-hub"]);
    token_answer(&fake, &claims, &[]);
    oidc.finish(&redirect, "c", &state).await.unwrap();
}

#[tokio::test]
async fn the_providers_refusal_is_relayed_and_an_unknown_state_is_refused() {
    let fake = fake_with_discovery();
    let oidc = OidcProvider::with_transport(config(), Arc::new(fake.clone()));
    let redirect = format!("{HUB}/auth/oidc/callback");
    assert!(matches!(
        oidc.finish(&redirect, "c", "made-up").await.unwrap_err(),
        Refusal::Invalid(_)
    ));
    let (_, state) = oidc.begin(&redirect, None).await.unwrap();
    fake.once(
        Method::Post,
        "/protocol/openid-connect/token",
        Ok(HttpResponse::json(
            400,
            &serde_json::json!({ "error": "invalid_grant", "error_description": "Code not valid" }),
        )),
    );
    let err = oidc.finish(&redirect, "c", &state).await.unwrap_err();
    assert!(err.message().contains("Code not valid"), "{err:?}");
}

#[tokio::test]
async fn discovery_must_name_this_issuer_and_stay_on_its_host() {
    let redirect = format!("{HUB}/auth/oidc/callback");
    let mut wrong_issuer = discovery();
    wrong_issuer["issuer"] = "https://sso.example.com/realms/other".into();
    let mut foreign_token = discovery();
    foreign_token["token_endpoint"] = "https://collector.example.net/token".into();
    for doc in [wrong_issuer, foreign_token] {
        let fake = FakeTransport::new();
        fake.always(
            Method::Get,
            "/.well-known/openid-configuration",
            Ok(HttpResponse::json(200, &doc)),
        );
        let oidc = OidcProvider::with_transport(config(), Arc::new(fake));
        let err = oidc.begin(&redirect, None).await.unwrap_err();
        assert!(matches!(err, Refusal::Provider(_)), "{err:?}");
    }
}

fn signed_in(username: Option<&str>, roles: &[&str]) -> SignedIn {
    SignedIn {
        subject: "0b7d-uuid".into(),
        username: username.map(str::to_string),
        display_name: Some("Ada Lovelace".into()),
        roles: roles.iter().map(|r| r.to_string()).collect(),
        device_name: None,
    }
}

#[test]
fn a_first_sign_in_provisions_a_person_and_the_next_finds_them() {
    let s = Store::open_in_memory().unwrap();
    let oidc = OidcProvider::with_transport(config(), Arc::new(FakeTransport::new()));
    let p = oidc
        .resolve_person(&s, &signed_in(Some("ada"), &[]))
        .unwrap();
    assert_eq!(p.name, "ada");
    assert_eq!(p.display_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(
        s.get_identity(ISS, "0b7d-uuid").unwrap().unwrap().person_id,
        p.id
    );
    // A renamed account (username changed at the provider) is still them.
    let again = oidc
        .resolve_person(&s, &signed_in(Some("ada.l"), &[]))
        .unwrap();
    assert_eq!(again.id, p.id);
}

#[test]
fn an_existing_person_is_never_taken_over_by_name() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap();
    let oidc = OidcProvider::with_transport(config(), Arc::new(FakeTransport::new()));
    let err = oidc
        .resolve_person(&s, &signed_in(Some("ada"), &[]))
        .unwrap_err();
    assert!(matches!(err, Refusal::Forbidden(_)));
    assert!(
        err.message().contains("link-sso ada --subject 0b7d-uuid"),
        "{err:?}"
    );
    assert!(s.get_identity(ISS, "0b7d-uuid").unwrap().is_none());
    // Once the operator links it, the sign-in is that person.
    s.link_identity(ISS, "0b7d-uuid", ada.id).unwrap();
    assert_eq!(
        oidc.resolve_person(&s, &signed_in(Some("ada"), &[]))
            .unwrap()
            .id,
        ada.id
    );
}

#[test]
fn a_disabled_person_and_a_missing_role_are_refused() {
    let s = Store::open_in_memory().unwrap();
    let ada = s.create_person("ada", None).unwrap();
    s.link_identity(ISS, "0b7d-uuid", ada.id).unwrap();
    s.disable_person(ada.id).unwrap();
    let oidc = OidcProvider::with_transport(config(), Arc::new(FakeTransport::new()));
    assert!(matches!(
        oidc.resolve_person(&s, &signed_in(Some("ada"), &[]))
            .unwrap_err(),
        Refusal::Forbidden(_)
    ));

    let mut cfg = config();
    cfg.allowed_roles = vec!["/fleet".into()];
    let gated = OidcProvider::with_transport(cfg, Arc::new(FakeTransport::new()));
    let fresh = Store::open_in_memory().unwrap();
    assert!(matches!(
        gated
            .resolve_person(&fresh, &signed_in(Some("bob"), &["other"]))
            .unwrap_err(),
        Refusal::Forbidden(_)
    ));
    // A Keycloak group path and a bare name match each other.
    gated
        .resolve_person(&fresh, &signed_in(Some("bob"), &["fleet"]))
        .unwrap();
}

#[test]
fn without_auto_provision_only_a_linked_account_gets_in() {
    let s = Store::open_in_memory().unwrap();
    let mut cfg = config();
    cfg.auto_provision = false;
    let oidc = OidcProvider::with_transport(cfg, Arc::new(FakeTransport::new()));
    let err = oidc
        .resolve_person(&s, &signed_in(Some("ada"), &[]))
        .unwrap_err();
    assert!(err.message().contains("link-sso"), "{err:?}");
    assert!(s.get_person_by_name("ada").unwrap().is_none());
}

#[test]
fn a_generated_device_name_skips_the_taken_ones() {
    assert_eq!(device_name_for("ada", &[]), "ada-sso");
    let taken = vec!["ada-sso".to_string(), "ada-sso-2".to_string()];
    assert_eq!(device_name_for("ada", &taken), "ada-sso-3");
    assert_eq!(device_name_for("Ada L", &[]), "Ada-L-sso");
    let long = "x".repeat(200);
    assert!(device_name_for(&long, &[]).chars().count() <= crate::store::MAX_CLIENT_NAME_LEN);
}

#[test]
fn pct_and_esc_cover_what_they_must() {
    assert_eq!(pct("a b/c?d=e&f~"), "a%20b%2Fc%3Fd%3De%26f~");
    assert_eq!(
        esc(r#"<a href="x">&'"#),
        "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
    );
    assert!(jwt_claims("not-a-jwt").is_none());
    assert!(jwt_claims("a.b").is_none());
}

// --- the routes, end to end over the real handlers ----------------------------

fn app(
    oidc: Option<Arc<OidcProvider>>,
) -> (
    axum::Router,
    Arc<Mutex<Store>>,
    Arc<super::super::pairing::PendingPairings>,
) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let pairings = Arc::new(super::super::pairing::PendingPairings::new());
    let mut state = PairState::new(
        Arc::clone(&store),
        Arc::clone(&pairings),
        Arc::new(super::super::guard::RateLimiter::new()),
        HUB.to_string(),
    )
    .with_oidc(oidc);
    state.attempt_interval = Duration::ZERO;
    let router = axum::Router::new()
        .route("/auth/oidc/start", axum::routing::get(handle_start))
        .route("/auth/oidc/callback", axum::routing::get(handle_callback))
        .with_state(state);
    (router, store, pairings)
}

async fn get(
    app: &axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    use tower_service::Service as _;
    let mut req = axum::http::Request::builder().uri(uri);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let resp = app
        .clone()
        .call(req.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn without_a_provider_both_routes_are_404() {
    let (app, _, _) = app(None);
    assert_eq!(
        get(&app, "/auth/oidc/start", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/auth/oidc/callback?code=a&state=b", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_sign_in_ends_in_a_pairing_code_for_that_person() {
    let fake = fake_with_discovery();
    let oidc = Arc::new(OidcProvider::with_transport(
        config(),
        Arc::new(fake.clone()),
    ));
    let (app, store, pairings) = app(Some(oidc));

    let (status, headers, _) = get(&app, "/auth/oidc/start?name=ada-phone", None).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap().to_string();
    let state = query_param(&location, "state");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("Secure"),
        "{cookie}"
    );
    let cookie_pair = cookie.split(';').next().unwrap().to_string();

    // Another browser (no cookie) cannot finish it.
    let (status, _, body) = get(
        &app,
        &format!("/auth/oidc/callback?code=c&state={state}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("another browser"), "{body}");

    token_answer(&fake, &id_claims(&query_param(&location, "nonce")), &[]);
    let (status, headers, body) = get(
        &app,
        &format!("/auth/oidc/callback?code=c&state={state}"),
        Some(&cookie_pair),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(body.contains("Signed in as <strong>ada</strong>"), "{body}");
    assert!(
        body.contains("claudefleet:https://fleet.example.com/pair#"),
        "{body}"
    );
    assert_eq!(pairings.len(), 1);

    // The code on the page is the one `/pair` redeems, for ada's device.
    let code = body
        .split("claudefleet:https://fleet.example.com/pair#")
        .nth(1)
        .unwrap()
        .chars()
        .take(8)
        .collect::<String>();
    let req = pairings.consume(&code).unwrap();
    assert_eq!(req.name, "ada-phone");
    assert_eq!(req.mode, "full");
    assert!(!req.trusted);
    assert_eq!(req.person.as_deref(), Some("ada"));
    assert!(store
        .lock()
        .unwrap()
        .get_person_by_name("ada")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn the_providers_error_and_a_bad_device_name_end_on_a_page() {
    let fake = fake_with_discovery();
    let oidc = Arc::new(OidcProvider::with_transport(config(), Arc::new(fake)));
    let (app, _, _) = app(Some(oidc));
    let (status, _, body) = get(
        &app,
        "/auth/oidc/callback?error=access_denied&error_description=%3Cb%3Eno%3C%2Fb%3E",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("&lt;b&gt;no&lt;/b&gt;"), "escaped: {body}");
    let (status, _, _) = get(&app, "/auth/oidc/start?name=a%0Ab", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
