//! Login conflicts: two copies of one Claude login on one host, which log
//! each other out (docs/accounts.md, *Two copies of one login*).
//!
//! **Why two copies break.** Every OAuth refresh hands out a new refresh
//! token and revokes the old one. Two copies of the same login refresh on
//! their own schedules, so the first to refresh leaves the other holding a
//! revoked token, and its next refresh fails: "Login expired · Please run
//! /login". Signing in again only starts the next round.
//!
//! **The two ways a host gets there.**
//! * [`LoginConflict::KeychainAndFile`]: on macOS Claude Code keeps the
//!   default login in the login Keychain, but a `claude` started over SSH
//!   (fleet's tmux sessions) finds the Keychain locked and falls back to
//!   `~/.claude/.credentials.json`. The desktop app or a local terminal
//!   then refreshes one copy and fleet's sessions the other. Read by the
//!   health probe (`credsplit=`, [`crate::tmux::HOST_HEALTH_SCRIPT`]) into
//!   [`HostRow::login_split`].
//! * [`LoginConflict::SharedAccount`]: the host's login and a login profile
//!   (or two profiles) signed into the same account, each in its own config
//!   dir. Derived from what the host row already holds.
//!
//! Fleet only reports these; which copy goes is the person's call.

use serde::{Deserialize, Serialize};

use crate::store::{HostProfileRow, HostRow};

/// One conflict on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoginConflict {
    /// macOS: the default login is in the Keychain and in
    /// `~/.claude/.credentials.json`.
    KeychainAndFile,
    /// Two or more logins on the host are one account. `logins` names
    /// them, `None` for the host's own login, then profiles by name.
    SharedAccount {
        logins: Vec<Option<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
    },
}

impl LoginConflict {
    /// One sentence for a person: what is wrong on `alias` and what to do.
    pub fn describe(&self, alias: &str) -> String {
        match self {
            LoginConflict::KeychainAndFile => format!(
                "{alias}: the default Claude login is kept twice, in the macOS Keychain (the \
                 desktop app, a local terminal) and in ~/.claude/.credentials.json (sessions \
                 started over SSH). Each refresh revokes the other copy, so one side keeps \
                 hitting \"Login expired\". Run fleet's sessions there under a login profile on \
                 another account, or sign the desktop app into another account."
            ),
            LoginConflict::SharedAccount { logins, email } => {
                let names: Vec<String> = logins
                    .iter()
                    .map(|l| match l {
                        None => "the host login".to_string(),
                        Some(p) => format!("profile {p}"),
                    })
                    .collect();
                let account = email.as_deref().unwrap_or("one account");
                let all = if names.len() == 2 { "both" } else { "all" };
                format!(
                    "{alias}: {} are {all} signed into {account}. Each refresh revokes the \
                     others' copy, so they keep hitting \"Login expired\". Keep one login per \
                     account on a host: sign the others into other accounts.",
                    names.join(" and ")
                )
            }
        }
    }
}

/// PURE: the conflicts on one host.
pub fn for_host(host: &HostRow) -> Vec<LoginConflict> {
    find(
        host.login_split,
        host.account_uuid.as_deref(),
        host.claude_profiles.as_deref().unwrap_or_default(),
    )
}

/// PURE: [`for_host`] over the parts it reads. `host_account` is the
/// account of the host's own login.
pub fn find(
    login_split: Option<bool>,
    host_account: Option<&str>,
    profiles: &[HostProfileRow],
) -> Vec<LoginConflict> {
    let mut out = Vec::new();
    if login_split == Some(true) {
        out.push(LoginConflict::KeychainAndFile);
    }
    // Accounts in first-seen order, the host's own login first.
    let mut groups: Vec<(String, Vec<Option<String>>, Option<String>)> = Vec::new();
    let mut add = |uuid: &str, login: Option<String>, email: Option<&String>| match groups
        .iter_mut()
        .find(|(u, _, _)| *u == uuid)
    {
        Some((_, logins, known)) => {
            logins.push(login);
            if known.is_none() {
                *known = email.cloned();
            }
        }
        None => groups.push((uuid.to_string(), vec![login], email.cloned())),
    };
    if let Some(uuid) = host_account.filter(|u| !u.is_empty()) {
        add(uuid, None, None);
    }
    for p in profiles {
        if let Some(uuid) = p.account_uuid.as_deref().filter(|u| !u.is_empty()) {
            add(uuid, Some(p.name.clone()), p.email.as_ref());
        }
    }
    out.extend(
        groups
            .into_iter()
            .filter(|(_, logins, _)| logins.len() > 1)
            .map(|(_, logins, email)| LoginConflict::SharedAccount { logins, email }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, uuid: Option<&str>, email: Option<&str>) -> HostProfileRow {
        HostProfileRow {
            name: name.to_string(),
            account_uuid: uuid.map(str::to_string),
            email: email.map(str::to_string),
        }
    }

    #[test]
    fn a_host_with_one_login_per_account_has_no_conflict() {
        let profiles = [
            profile("work", Some("b"), Some("b@x")),
            profile("new", None, None),
        ];
        assert!(find(Some(false), Some("a"), &profiles).is_empty());
        assert!(find(None, None, &[]).is_empty());
    }

    #[test]
    fn the_keychain_and_a_file_is_a_conflict() {
        assert_eq!(
            find(Some(true), Some("a"), &[]),
            vec![LoginConflict::KeychainAndFile]
        );
    }

    #[test]
    fn a_profile_on_the_hosts_own_account_is_a_conflict() {
        // The Mac in the field: the host login and profile Primary were both
        // mj@…, and so was the Keychain copy.
        let profiles = [
            profile("Primary", Some("a"), Some("mj@x")),
            profile("work", Some("b"), Some("b@x")),
        ];
        assert_eq!(
            find(Some(true), Some("a"), &profiles),
            vec![
                LoginConflict::KeychainAndFile,
                LoginConflict::SharedAccount {
                    logins: vec![None, Some("Primary".into())],
                    email: Some("mj@x".into()),
                },
            ]
        );
    }

    #[test]
    fn two_profiles_on_one_account_are_a_conflict_without_the_host_login() {
        let profiles = [
            profile("one", Some("b"), None),
            profile("two", Some("b"), Some("b@x")),
        ];
        assert_eq!(
            find(None, Some("a"), &profiles),
            vec![LoginConflict::SharedAccount {
                logins: vec![Some("one".into()), Some("two".into())],
                email: Some("b@x".into()),
            }]
        );
    }

    #[test]
    fn the_sentences_name_the_logins_and_the_account() {
        let c = LoginConflict::SharedAccount {
            logins: vec![None, Some("Primary".into())],
            email: Some("mj@x".into()),
        };
        let s = c.describe("mac");
        assert!(
            s.starts_with("mac: the host login and profile Primary are both signed into mj@x."),
            "{s}"
        );
        let k = LoginConflict::KeychainAndFile.describe("mac");
        assert!(
            k.contains("Keychain") && k.contains(".credentials.json"),
            "{k}"
        );
    }

    #[test]
    fn a_conflict_crosses_the_wire_tagged_by_kind() {
        let v = serde_json::to_value(LoginConflict::SharedAccount {
            logins: vec![None, Some("p".into())],
            email: None,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "shared_account", "logins": [null, "p"]})
        );
        assert_eq!(
            serde_json::to_value(LoginConflict::KeychainAndFile).unwrap(),
            serde_json::json!({"kind": "keychain_and_file"})
        );
    }
}
