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
//! * [`LoginConflict::Copied`], across hosts: one `.credentials.json`
//!   copied onto another host (or into another profile), by a dotfiles
//!   sync, `rsync` of `~/.claude`, a copied profile dir. Both copies hold
//!   the same refresh token, so the same race runs between hosts. The
//!   health probe prints a fingerprint of each login's refresh token
//!   (`loginfp=`, the first 12 hex digits of its SHA-256, computed on the
//!   host), the store keeps each login's last few
//!   ([`HostRow::login_fingerprints`]), and [`copied`] matches them across
//!   the fleet: one login's newest fingerprint among another's means one
//!   is a copy of the other. The older ones keep the match after the
//!   first copy refreshes, until the other is signed in on its own.
//!
//! Fleet only reports these; which copy goes is the person's call.

use serde::{Deserialize, Serialize};

use crate::service::add_account::is_api_key_account;
use crate::store::{HostProfileRow, HostRow, LoginFingerprint};

/// One login in the fleet: a host and its own login (`profile` `None`) or
/// one of its profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginAt {
    pub host: String,
    #[serde(default)]
    pub profile: Option<String>,
}

impl LoginAt {
    /// "the host login on mac", "profile work on trn".
    fn name(&self) -> String {
        match &self.profile {
            None => format!("the host login on {}", self.host),
            Some(p) => format!("profile {p} on {}", self.host),
        }
    }
}

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
    /// The host's login `profile` (`None`: its own) holds the same refresh
    /// token as each of `copies`, logins elsewhere in the fleet (or on the
    /// same host, under another profile).
    Copied {
        #[serde(default)]
        profile: Option<String>,
        copies: Vec<LoginAt>,
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
                 hitting \"Login expired\". Run fleet's sessions there under a login profile, \
                 then delete ~/.claude/.credentials.json: the mark stays while both copies exist."
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
            LoginConflict::Copied { profile, copies } => {
                let this = match profile {
                    None => "the host login".to_string(),
                    Some(p) => format!("profile {p}"),
                };
                let others: Vec<String> = copies.iter().map(LoginAt::name).collect();
                format!(
                    "{alias}: {this} is a copy of one Claude login that is also {}: they share \
                     one refresh token, so each refresh revokes the others' copy and they take \
                     turns hitting \"Login expired\". Sign each host in on its own (Sign in \
                     again), and stop copying .credentials.json between hosts (a dotfiles or \
                     ~/.claude sync, a copied profile dir).",
                    others.join(" and ")
                )
            }
        }
    }
}

/// PURE: whether two logins' fingerprints say one is a copy of the other:
/// either's newest is among the other's.
fn same_token(a: &LoginFingerprint, b: &LoginFingerprint) -> bool {
    let has = |x: &LoginFingerprint, f: Option<&String>| f.is_some_and(|f| x.seen.contains(f));
    has(b, a.seen.first()) || has(a, b.seen.first())
}

/// PURE: the fleet's copied logins, each group two or more logins holding
/// one refresh token, in host order. A login counts once, in the first
/// group it matches.
pub fn copied(hosts: &[HostRow]) -> Vec<Vec<LoginAt>> {
    let logins: Vec<(LoginAt, &LoginFingerprint)> = hosts
        .iter()
        .flat_map(|h| {
            h.login_fingerprints.iter().map(move |f| {
                (
                    LoginAt {
                        host: h.alias.clone(),
                        profile: f.profile.clone(),
                    },
                    f,
                )
            })
        })
        .collect();
    let mut taken = vec![false; logins.len()];
    let mut out = Vec::new();
    for i in 0..logins.len() {
        if taken[i] {
            continue;
        }
        // Grow the group until nothing more matches any member.
        let mut group = vec![i];
        let mut k = 0;
        while k < group.len() {
            let m = group[k];
            for j in 0..logins.len() {
                if !taken[j] && !group.contains(&j) && same_token(logins[m].1, logins[j].1) {
                    group.push(j);
                }
            }
            k += 1;
        }
        if group.len() > 1 {
            for &g in &group {
                taken[g] = true;
            }
            out.push(group.into_iter().map(|g| logins[g].0.clone()).collect());
        }
    }
    out
}

/// PURE: [`LoginConflict::Copied`] for each of `alias`'s logins in a
/// [`copied`] group.
pub fn copied_on(alias: &str, groups: &[Vec<LoginAt>]) -> Vec<LoginConflict> {
    let mut out = Vec::new();
    for g in groups {
        for l in g.iter().filter(|l| l.host == alias) {
            out.push(LoginConflict::Copied {
                profile: l.profile.clone(),
                copies: g.iter().filter(|o| *o != l).cloned().collect(),
            });
        }
    }
    out
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
    // An API key has no refresh token to revoke, so one key under two
    // profiles is not a conflict.
    let oauth = |u: &&str| !u.is_empty() && !is_api_key_account(u);
    if let Some(uuid) = host_account.filter(oauth) {
        add(uuid, None, None);
    }
    // Nor does a setup token: it is a token of its own, not a second
    // `/login` sharing one refresh token.
    for p in profiles.iter().filter(|p| p.credential.is_none()) {
        if let Some(uuid) = p.account_uuid.as_deref().filter(oauth) {
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
            credential: None,
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
    fn one_api_key_under_two_profiles_is_not_a_conflict() {
        let profiles = [
            profile("one", Some("apikey-0123456789abcdef"), None),
            profile("two", Some("apikey-0123456789abcdef"), None),
        ];
        assert!(find(None, Some("apikey-0123456789abcdef"), &profiles).is_empty());
    }

    #[test]
    fn a_setup_token_on_the_hosts_own_account_is_not_a_conflict() {
        let tok = HostProfileRow {
            credential: Some("setup_token".into()),
            ..profile("tok", Some("A"), None)
        };
        assert!(find(None, Some("A"), &[tok]).is_empty());
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
            k.contains("Keychain") && k.contains("delete ~/.claude/.credentials.json"),
            "{k}"
        );
    }

    fn host_with(alias: &str, fps: &[(Option<&str>, &[&str])]) -> HostRow {
        let s = crate::store::Store::open_in_memory().unwrap();
        s.upsert_host(alias).unwrap();
        let mut h = s.get_host_row(alias).unwrap().unwrap();
        h.login_fingerprints = fps
            .iter()
            .map(|(p, seen)| LoginFingerprint {
                profile: p.map(str::to_string),
                seen: seen.iter().map(|s| s.to_string()).collect(),
            })
            .collect();
        h
    }

    fn at(host: &str, profile: Option<&str>) -> LoginAt {
        LoginAt {
            host: host.into(),
            profile: profile.map(str::to_string),
        }
    }

    #[test]
    fn one_token_on_two_hosts_is_a_copied_login() {
        // mac's ~/.claude synced onto mefistos: one token, two hosts.
        let hosts = [
            host_with(
                "mac",
                &[(None, &["aaaaaaaaaaaa"]), (Some("work"), &["bbbbbbbbbbbb"])],
            ),
            host_with("mefistos", &[(None, &["aaaaaaaaaaaa"])]),
            host_with("trn", &[(None, &["cccccccccccc"])]),
        ];
        let groups = copied(&hosts);
        assert_eq!(groups, vec![vec![at("mac", None), at("mefistos", None)]]);
        assert_eq!(
            copied_on("mefistos", &groups),
            vec![LoginConflict::Copied {
                profile: None,
                copies: vec![at("mac", None)],
            }]
        );
        assert!(copied_on("trn", &groups).is_empty());
    }

    #[test]
    fn a_copy_still_matches_after_the_original_refreshed() {
        // mac refreshed (new token first, the copied one kept); mefistos
        // still holds the old one and is about to be signed out.
        let hosts = [
            host_with("mac", &[(None, &["dddddddddddd", "aaaaaaaaaaaa"])]),
            host_with("mefistos", &[(None, &["aaaaaaaaaaaa"])]),
        ];
        assert_eq!(copied(&hosts).len(), 1);
        // Once mefistos is signed in on its own, its newest is not mac's,
        // nor mac's newest its: the mark goes.
        let hosts = [
            host_with("mac", &[(None, &["dddddddddddd", "aaaaaaaaaaaa"])]),
            host_with("mefistos", &[(None, &["eeeeeeeeeeee", "aaaaaaaaaaaa"])]),
        ];
        assert!(copied(&hosts).is_empty());
    }

    #[test]
    fn a_profile_dir_copied_on_one_host_is_a_copied_login_too() {
        let hosts = [host_with(
            "trn",
            &[
                (None, &["aaaaaaaaaaaa"]),
                (Some("spare"), &["aaaaaaaaaaaa"]),
            ],
        )];
        let groups = copied(&hosts);
        assert_eq!(
            groups,
            vec![vec![at("trn", None), at("trn", Some("spare"))]]
        );
        assert_eq!(copied_on("trn", &groups).len(), 2);
        let s = copied_on("trn", &groups)[0].describe("trn");
        assert!(
            s.starts_with("trn: the host login is a copy of one Claude login that is also profile spare on trn"),
            "{s}"
        );
    }

    #[test]
    fn three_copies_are_one_group() {
        let hosts = [
            host_with("a", &[(None, &["111111111111"])]),
            host_with("b", &[(None, &["222222222222", "111111111111"])]),
            host_with("c", &[(None, &["222222222222"])]),
        ];
        assert_eq!(
            copied(&hosts),
            vec![vec![at("a", None), at("b", None), at("c", None)]]
        );
    }

    #[test]
    fn a_conflict_crosses_the_wire_tagged_by_kind() {
        assert_eq!(
            serde_json::to_value(LoginConflict::Copied {
                profile: None,
                copies: vec![at("mac", Some("p"))],
            })
            .unwrap(),
            serde_json::json!({"kind": "copied", "profile": null, "copies": [{"host": "mac", "profile": "p"}]})
        );
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
