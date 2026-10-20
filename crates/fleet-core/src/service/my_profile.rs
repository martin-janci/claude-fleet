//! The caller's OWN profile on this fleet — `my_profile` (multi-hub Phase 3b,
//! `docs/superpowers/specs/2026-10-17-desktop-multi-hub-design.md`).
//!
//! One human can be a different person on each hub they use (a work and a
//! private identity, each with its own GitHub token, e-mail and sign-in).
//! A desktop that shows several hubs asks each one "who am I here?" and
//! shows the answer on that fleet's chip, its Settings row and New session.
//!
//! Per-caller by construction, like `my_grants`: there is no argument to name
//! anybody else, the one input is the connection's own person
//! (`mcp::tools::fleet::owner_for`), and a caller that proves no person
//! gets an answer with `person_id: None` and nothing else. Secrets never
//! appear: an environment entry is listed by NAME (with whether it is a
//! secret or a reference); the only values served are the git identity's
//! name and e-mail, and only when they are stored as plain values — the same
//! values `env_vars list` already shows this caller.

use crate::ipc_error::IpcError;
use crate::store::Store;
use serde::{Deserialize, Serialize};

/// One linked single-sign-on account, by its issuer. The subject is the
/// provider's opaque id and is not served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MySso {
    pub issuer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<i64>,
}

/// One of the caller's own person-scope environment variables, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyEnvName {
    pub name: String,
    pub secret: bool,
    pub is_ref: bool,
}

/// The git identity the caller's sessions commit as, from their own
/// person-scope `GIT_AUTHOR_NAME` / `GIT_AUTHOR_EMAIL` (plain values only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyGit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// `my_profile`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyProfile {
    /// Who the caller is here, as a `people` row id; `None` for a caller
    /// that proves no person (everything below is then empty).
    pub person_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// This hub's own owner.
    #[serde(default)]
    pub is_owner: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sso: Vec<MySso>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<MyEnvName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<MyGit>,
}

/// `person`'s own profile. `None` answers the empty profile, never anyone's.
pub fn my_profile(s: &Store, person: Option<i64>) -> Result<MyProfile, IpcError> {
    let Some(id) = person else {
        return Ok(MyProfile::default());
    };
    let Some(p) = s.get_person(id)? else {
        return Ok(MyProfile {
            person_id: Some(id),
            ..MyProfile::default()
        });
    };
    let sso = s
        .list_identities(id)?
        .into_iter()
        .map(|i| MySso {
            issuer: i.issuer,
            last_login_at: i.last_login_at,
        })
        .collect();
    let key = id.to_string();
    let mut env = Vec::new();
    let mut git = MyGit::default();
    for r in s.list_env_vars()? {
        if r.scope != "person" || r.scope_key != key {
            continue;
        }
        match r.name.as_str() {
            "GIT_AUTHOR_NAME" if !r.secret && !r.is_ref => git.name = r.value.clone(),
            "GIT_AUTHOR_EMAIL" if !r.secret && !r.is_ref => git.email = r.value.clone(),
            _ => {}
        }
        env.push(MyEnvName {
            name: r.name,
            secret: r.secret,
            is_ref: r.is_ref,
        });
    }
    let git = (git.name.is_some() || git.email.is_some()).then_some(git);
    Ok(MyProfile {
        person_id: Some(id),
        name: Some(p.name),
        display_name: p.display_name,
        email: p.email,
        is_owner: p.is_personal_owner,
        sso,
        env,
        git,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EnvValue, Secret};

    fn person(s: &Store, name: &str) -> i64 {
        s.create_person(name, None).unwrap().id
    }

    #[test]
    fn nobody_gets_the_empty_profile() {
        let s = Store::open_in_memory().unwrap();
        person(&s, "ada");
        assert_eq!(my_profile(&s, None).unwrap(), MyProfile::default());
    }

    #[test]
    fn a_person_gets_their_own_names_sso_and_variable_names_and_no_secret() {
        let s = Store::open_in_memory().unwrap();
        let ada = person(&s, "ada");
        let bob = person(&s, "bob");
        let me = ada.to_string();
        s.set_env_var(
            "person",
            &me,
            "GH_TOKEN",
            &EnvValue::Secret(Secret::new("ghp_secret")),
        )
        .unwrap();
        s.set_env_var(
            "person",
            &me,
            "GIT_AUTHOR_NAME",
            &EnvValue::Plain("Ada L".into()),
        )
        .unwrap();
        s.set_env_var(
            "person",
            &me,
            "GIT_AUTHOR_EMAIL",
            &EnvValue::Plain("ada@work.example".into()),
        )
        .unwrap();
        s.set_env_var(
            "person",
            &bob.to_string(),
            "JIRA_API_TOKEN",
            &EnvValue::Secret(Secret::new("bob-tok")),
        )
        .unwrap();
        s.set_env_var("global", "", "SHARED", &EnvValue::Plain("x".into()))
            .unwrap();
        s.link_identity("https://sso.example/realms/work", "sub-1", ada)
            .unwrap();

        let p = my_profile(&s, Some(ada)).unwrap();
        assert_eq!(p.person_id, Some(ada));
        assert_eq!(p.name.as_deref(), Some("ada"));
        assert_eq!(p.sso.len(), 1);
        assert_eq!(p.sso[0].issuer, "https://sso.example/realms/work");
        let names: Vec<&str> = p.env.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["GH_TOKEN", "GIT_AUTHOR_EMAIL", "GIT_AUTHOR_NAME"]);
        assert!(p.env[0].secret);
        assert_eq!(
            p.git,
            Some(MyGit {
                name: Some("Ada L".into()),
                email: Some("ada@work.example".into())
            })
        );
        let wire = serde_json::to_string(&p).unwrap();
        assert!(
            !wire.contains("ghp_secret"),
            "a secret never leaves: {wire}"
        );
        assert!(
            !wire.contains("bob-tok") && !wire.contains("JIRA_API_TOKEN"),
            "nobody else's: {wire}"
        );
        assert!(
            !wire.contains("SHARED"),
            "the shared scopes are not a profile: {wire}"
        );
        assert!(!wire.contains("sub-1"), "the subject is not served: {wire}");
    }
}
