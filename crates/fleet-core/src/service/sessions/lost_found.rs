//! Lost and found with proposals (redesign step 4.12): the project an
//! entry goes into, prefilled.
//!
//! [`lost_target`] answers what Host detail's Adopt and Restore forms are
//! prefilled with. The rule goes first: a working directory inside a fleet
//! project (`find_project_id_for_path`, the same match reconcile and
//! discover use) is that project. Only when no rule places it is Jev asked
//! ([`crate::service::decide::lost_target`], `decide.jev.adopt_target` /
//! `decide.jev.restore_target`, both off by default). Nothing here adopts,
//! restores or moves anything: a person presses the button and confirms.
//!
//! [`place_transcript`] is what Restore into a project needs when the
//! conversation ran somewhere else: Claude Code keys transcripts by the
//! working directory, so `claude --resume` in the project's root finds the
//! conversation only once a copy sits under that root's directory in
//! `~/.claude/projects`. It copies, never moves, and never overwrites a
//! transcript already there (as `move_session` does for its target).

use super::*;
use crate::ipc_error::{codes, lock};
use crate::service::decide::lost_target::{
    self as decide_lost, pane_subject, transcript_subject, LostInput, LostKind, LostTarget,
    LostTicket, MAX_CANDIDATES,
};
use crate::service::decide::start_project::{fenced, Candidate};
use crate::service::decide::DecideCtx;
use crate::service::decide::Feature;
use paths::{find_project_id_for_path, HostPaths};
use reconcile::HostShell;

/// Wall clock for the one small script either call runs on the host.
const SCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct LostTargetArgs {
    /// A live tmux session fleet did not start (Adopt): its fleet id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// Or a conversation `discover_lost_sessions` found (Restore): its host,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// its transcript id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// its working directory, as the listing gave it,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// and its git branch, when the listing gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PlaceTranscriptArgs {
    /// The host the conversation was found on.
    pub host_alias: String,
    /// The conversation's transcript id, as `discover_lost_sessions` listed it.
    pub claude_session_id: String,
    /// The project to restore it into; its root on the host must exist.
    pub project_id: i64,
}

/// Where a placed conversation can be resumed: `new_session { host_alias,
/// project_id, name: tmux_name, resume_claude_session_id }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedTranscript {
    pub project_id: i64,
    pub tmux_name: String,
    /// `false` when a transcript with that id was already under the root
    /// (it was left as it was).
    pub copied: bool,
}

/// The candidates a question carries: the person's projects (not fleet's
/// own), most recently used first, at most [`MAX_CANDIDATES`].
fn candidates(projects: &[ProjectRow]) -> Vec<Candidate> {
    let mut rows: Vec<&ProjectRow> = projects.iter().filter(|p| !p.system).collect();
    rows.sort_by_key(|p| std::cmp::Reverse(p.last_session_at.unwrap_or(0)));
    rows.into_iter()
        .take(MAX_CANDIDATES)
        .map(|p| Candidate {
            project_id: p.id,
            owner: p.owner.clone(),
            repo: p.repo.clone(),
        })
        .collect()
}

/// PURE: a pane's working directory from `tmux display-message`'s output.
fn parse_pane_cwd(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    line.starts_with('/').then(|| line.to_string())
}

/// What the Adopt (`session_id`) or Restore (`host_alias`,
/// `claude_session_id`, `cwd`) form is prefilled with. See the module
/// header. `E_INVALID` for neither or a malformed entry, `E_NOTFOUND` /
/// `E_INVALID_STATE` for a pane `adopt_session` would refuse. A pane whose
/// directory cannot be read answers an empty target, never an error: the
/// form only loses its prefill.
pub(crate) async fn lost_target(
    args: LostTargetArgs,
    store: &Arc<Mutex<Store>>,
    shell: &dyn HostShell,
    ctx: &DecideCtx,
) -> Result<LostTarget, IpcError> {
    let (input, ticket) = match args.session_id {
        Some(id) => (pane_input(id, store, shell).await?, None),
        None => {
            let input = transcript_input(&args, store)?;
            (input, transcript_ticket(&args, store)?)
        }
    };
    let mut target = match input {
        Ok(input) => decide_lost::propose(ctx, input).await,
        Err(known) => known,
    };
    target.ticket = ticket;
    Ok(target)
}

/// J10's "or ticket" half: the ticket a found conversation's git branch
/// names (`pd-2412-receipt-totals` → `PD-2412`), recognised the way a live
/// session's branch is (`work::recognize::first_key` over every tracker's
/// key prefixes), with its title when a tracker's cache holds it. A rule,
/// like the project's: Jev is not asked, nothing is sent anywhere, and
/// nothing is linked until a person confirms Restore with it ticked.
fn transcript_ticket(
    args: &LostTargetArgs,
    store: &Arc<Mutex<Store>>,
) -> Result<Option<LostTicket>, IpcError> {
    let Some(branch) = args
        .git_branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    else {
        return Ok(None);
    };
    let s = lock(store)?;
    let trackers = s.list_trackers()?;
    let mut prefixes: Vec<String> = Vec::new();
    for t in &trackers {
        for p in &t.config.key_prefixes {
            let p = p.to_ascii_uppercase();
            if !prefixes.contains(&p) {
                prefixes.push(p);
            }
        }
    }
    let ctx = crate::service::work::recognize::RecognizeCtx {
        prefixes,
        trackers: crate::service::work::detect::tracker_hosts(&trackers),
        repo: None,
    };
    let Some(key) = crate::service::work::recognize::first_key(branch, &ctx) else {
        return Ok(None);
    };
    let title = s.tracker_item_for_key(&key)?.map(|item| item.title);
    Ok(Some(LostTicket {
        key,
        title,
        source: "rule".into(),
        reason: format!("its branch {branch} names it"),
    }))
}

/// The pane's question, or (as `Err`) the answer without one: the rule's
/// project (the row's own, or its directory's), or nothing when the
/// directory cannot be read.
async fn pane_input(
    session_id: i64,
    store: &Arc<Mutex<Store>>,
    shell: &dyn HostShell,
) -> Result<Result<LostInput, LostTarget>, IpcError> {
    let row = {
        let s = lock(store)?;
        s.get_session_by_id(session_id)?.ok_or_else(|| {
            IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
        })?
    };
    if !is_outside_fleet(&row) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("session {session_id} is not a pane fleet did not start"),
        ));
    }
    if let Some(pid) = row.project_id {
        return Ok(Err(LostTarget::rule(pid)));
    }
    crate::validate::host_alias(&row.host_alias)?;
    crate::validate::tmux_name_addressable(&row.tmux_name)?;
    let script = format!(
        "tmux display-message -p -t {} '#{{pane_current_path}}'",
        quote(&crate::tmux::exact_pane(&row.tmux_name))
    );
    let cwd = match tokio::time::timeout(SCRIPT_TIMEOUT, shell.run_script(&row.host_alias, &script))
        .await
    {
        Ok(Ok(out)) => parse_pane_cwd(&out),
        Ok(Err(e)) => {
            tracing::warn!(
                "[lost_found] pane {session_id}'s directory not read: {}",
                e.message
            );
            None
        }
        Err(_) => None,
    };
    let Some(cwd) = cwd else {
        return Ok(Err(LostTarget::default()));
    };
    let s = lock(store)?;
    let projects = s.list_projects()?;
    let paths = HostPaths::for_host(&s, &row.host_alias);
    if let Some(pid) = find_project_id_for_path(
        &projects,
        &row.host_alias,
        std::path::Path::new(&cwd),
        &paths,
    ) {
        return Ok(Err(LostTarget::rule(pid)));
    }
    Ok(Ok(LostInput {
        kind: LostKind::Pane,
        subject: pane_subject(row.id),
        org_id: row.org_id,
        cwd,
        git_branch: None,
        name: Some(row.tmux_name.clone()),
        candidates: fenced(&s, Feature::AdoptTarget, row.org_id, &candidates(&projects)),
    }))
}

/// The transcript's question, or (as `Err`) the rule's answer.
fn transcript_input(
    args: &LostTargetArgs,
    store: &Arc<Mutex<Store>>,
) -> Result<Result<LostInput, LostTarget>, IpcError> {
    let (Some(host), Some(id), Some(cwd)) = (
        args.host_alias.as_deref(),
        args.claude_session_id.as_deref(),
        args.cwd.as_deref(),
    ) else {
        return Err(IpcError::new(
            codes::E_INVALID,
            "pass session_id (a pane) or host_alias, claude_session_id and cwd (a found conversation)",
        ));
    };
    crate::validate::host_alias(host)?;
    crate::validate::claude_session_id(id)?;
    if !cwd.starts_with('/') {
        return Err(IpcError::new(
            codes::E_INVALID,
            "cwd must be an absolute path",
        ));
    }
    let s = lock(store)?;
    let projects = s.list_projects()?;
    let paths = HostPaths::for_host(&s, host);
    if let Some(pid) = find_project_id_for_path(&projects, host, std::path::Path::new(cwd), &paths)
    {
        return Ok(Err(LostTarget::rule(pid)));
    }
    let fp_key = s.decision_fp_key()?;
    // The org a session started there would have (its path and host), so
    // the question asks THAT org's consent, not `decide.jev.unassigned`.
    let org_id = crate::store::org_of_session(
        &crate::store::SessionOrgFacts {
            host_alias: host,
            owner: None,
            repo: None,
            path: Some(cwd),
        },
        &s.list_org_rules()?,
        s.host_org(host)?,
    );
    Ok(Ok(LostInput {
        kind: LostKind::Transcript,
        subject: transcript_subject(&fp_key, id),
        org_id,
        cwd: cwd.to_string(),
        git_branch: args.git_branch.clone().filter(|b| !b.trim().is_empty()),
        name: None,
        candidates: fenced(&s, Feature::RestoreTarget, org_id, &candidates(&projects)),
    }))
}

/// Marker the placement script prints before its answer.
const PLACED: &str = "cf-place:";

/// The script that copies transcript `id` under the directory Claude Code
/// keys `root` by. Prints `cf-place:copied` or `cf-place:kept`; exits 3 when
/// the root is not on the host, 4 when no transcript has that id.
pub fn place_script(root: &str, id: &str) -> String {
    format!(
        r#"set +e
root={root}
id={id}
[ -n "$HOME" ] || {{ echo 'HOME is not set' >&2; exit 5; }}
case "$root" in "~") root=$HOME ;; '~/'*) root="$HOME/${{root#\~/}}" ;; esac
p=$(cd -- "$root" 2>/dev/null && pwd -P) || {{ echo "the project is not on this host: $root" >&2; exit 3; }}
src=
for f in "$HOME"/.claude/projects/*/"$id.jsonl"; do [ -f "$f" ] && {{ src=$f; break; }}; done
[ -n "$src" ] || {{ echo "no transcript $id on this host" >&2; exit 4; }}
enc=$(printf '%s' "$p" | sed 's/[^A-Za-z0-9]/-/g')
d="$HOME/.claude/projects/$enc"
mkdir -p -- "$d" || exit 7
if [ -e "$d/$id.jsonl" ]; then echo '{PLACED}kept'; exit 0; fi
cp -p -- "$src" "$d/$id.jsonl" || exit 7
echo '{PLACED}copied'
"#,
        root = quote(root),
        id = quote(id),
    )
}

/// Copy a found conversation's transcript where `claude --resume` in
/// `project_id`'s root finds it, for Restore into that project. Returns the
/// tmux name to resume it under. Records the person's choice against an
/// assist proposal ([`decide_lost::record_choice`]); the caller passes
/// `by_person` false for an agent's call, which marks nothing.
pub(crate) async fn place_transcript(
    args: PlaceTranscriptArgs,
    store: &Arc<Mutex<Store>>,
    shell: &dyn HostShell,
    by_person: bool,
) -> Result<PlacedTranscript, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::claude_session_id(&args.claude_session_id)?;
    let (root, owner, repo) = {
        let s = lock(store)?;
        let project = s.get_project(args.project_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("project {} not found", args.project_id),
            )
        })?;
        let (root, _) = paths::work_dirs(&s, &args.host_alias, args.project_id, None)?;
        (root, project.owner, project.repo)
    };
    let script = place_script(&root, &args.claude_session_id);
    let out = tokio::time::timeout(SCRIPT_TIMEOUT, shell.run_script(&args.host_alias, &script))
        .await
        .map_err(|_| IpcError::new(codes::E_TIMEOUT, "placing the transcript timed out"))??;
    let copied = match out.lines().find_map(|l| l.trim().strip_prefix(PLACED)) {
        Some("copied") => true,
        Some("kept") => false,
        _ => {
            return Err(IpcError::new(
                codes::E_SHELL,
                format!("unexpected answer placing the transcript: {}", out.trim()),
            ))
        }
    };
    if by_person {
        let s = lock(store)?;
        let fp_key = s.decision_fp_key()?;
        let subject = transcript_subject(&fp_key, &args.claude_session_id);
        if let Err(e) = decide_lost::record_choice(
            &s,
            LostKind::Transcript,
            &subject,
            Some(args.project_id),
            now_unix(),
        ) {
            tracing::warn!(
                "[decide] restore_target follow-up not recorded: {}",
                e.message
            );
        }
    }
    // The name `new_session` will take: the project's own name, unless a
    // row on the host already holds it (then a fresh pair, as
    // `fill_session_name` mints).
    let tmux_name = {
        let s = lock(store)?;
        let own = discover::derive_tmux_name(&owner, &repo, "main");
        let on_host = s.list_sessions_for_host(&args.host_alias)?;
        if on_host.iter().any(|r| r.tmux_name == own) {
            let taken = lifecycle::project_taken_slugs(&s, args.project_id, &owner, &repo)?;
            let pair = crate::service::names::generate_name_default(&taken);
            crate::service::names::tmux_safe(&format!("{own}--{pair}"))
        } else {
            own
        }
    };
    Ok(PlacedTranscript {
        project_id: args.project_id,
        tmux_name,
        copied,
    })
}

/// [`lost_target`] with the real shell.
pub async fn lost_target_over_ssh(
    args: LostTargetArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    ctx: &DecideCtx,
) -> Result<LostTarget, IpcError> {
    let shell = reconcile::RealHostShell {
        ssh: Arc::clone(ssh),
        timeout: SCRIPT_TIMEOUT,
    };
    lost_target(args, store, &shell, ctx).await
}

/// [`place_transcript`] with the real shell.
pub async fn place_transcript_over_ssh(
    args: PlaceTranscriptArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    by_person: bool,
) -> Result<PlacedTranscript, IpcError> {
    let shell = reconcile::RealHostShell {
        ssh: Arc::clone(ssh),
        timeout: SCRIPT_TIMEOUT,
    };
    place_transcript(args, store, &shell, by_person).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    const ID: &str = "44366faf-ae97-426a-91cd-beaf3c74f1d7";

    /// Answers every script with `stdout` and keeps what it was asked.
    struct Shell {
        stdout: String,
        seen: StdMutex<Vec<(String, String)>>,
    }

    impl Shell {
        fn answering(stdout: &str) -> Shell {
            Shell {
                stdout: stdout.into(),
                seen: StdMutex::new(Vec::new()),
            }
        }
        fn scripts(&self) -> Vec<(String, String)> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HostShell for Shell {
        async fn run_script(&self, host: &str, script: &str) -> Result<String, IpcError> {
            self.seen
                .lock()
                .unwrap()
                .push((host.to_string(), script.to_string()));
            Ok(self.stdout.clone())
        }
    }

    struct World {
        store: Arc<Mutex<Store>>,
        project: i64,
        pane: i64,
    }

    fn world() -> World {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let project = s
            .upsert_project("acme", "papaya-pos", "/p/acme/papaya-pos")
            .unwrap();
        let pane = s
            .upsert_session("scratch", "local", None, None, 1, 1, "running", None)
            .unwrap();
        World {
            store: Arc::new(Mutex::new(s)),
            project,
            pane,
        }
    }

    /// Review r15: a found conversation on an org's host is that org's,
    /// so its question needs the org's consent, not `unassigned`.
    #[test]
    fn a_found_conversation_on_an_orgs_host_carries_that_org() {
        let w = world();
        let org = {
            let s = w.store.lock().unwrap();
            let org = s.add_org("Acme", None, false).unwrap().id;
            s.set_host_org("local", Some(org)).unwrap();
            org
        };
        let args = LostTargetArgs {
            host_alias: Some("local".into()),
            claude_session_id: Some(ID.into()),
            cwd: Some("/home/me/elsewhere".into()),
            ..Default::default()
        };
        let input = transcript_input(&args, &w.store).unwrap().unwrap();
        assert_eq!(input.org_id, Some(org));
    }

    impl World {
        fn ctx(&self) -> DecideCtx {
            DecideCtx::jev(Arc::clone(&self.store))
        }
        async fn target(
            &self,
            args: LostTargetArgs,
            shell: &Shell,
        ) -> Result<LostTarget, IpcError> {
            lost_target(args, &self.store, shell, &self.ctx()).await
        }
    }

    fn pane_args(id: i64) -> LostTargetArgs {
        LostTargetArgs {
            session_id: Some(id),
            ..Default::default()
        }
    }

    fn transcript_args(cwd: &str) -> LostTargetArgs {
        LostTargetArgs {
            host_alias: Some("local".into()),
            claude_session_id: Some(ID.into()),
            cwd: Some(cwd.into()),
            git_branch: Some("pd-2412-receipt-totals".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_pane_reconcile_placed_is_the_rules_and_reads_nothing() {
        let w = world();
        lock(&w.store)
            .unwrap()
            .set_session_project(w.pane, w.project)
            .unwrap();
        let shell = Shell::answering("/elsewhere\n");
        let t = w.target(pane_args(w.pane), &shell).await.unwrap();
        assert_eq!(t, LostTarget::rule(w.project));
        assert!(shell.scripts().is_empty(), "no SSH for a known project");
    }

    #[tokio::test]
    async fn a_pane_in_a_project_directory_is_the_rules() {
        let w = world();
        let shell = Shell::answering("/p/acme/papaya-pos/src\n");
        let t = w.target(pane_args(w.pane), &shell).await.unwrap();
        assert_eq!(t, LostTarget::rule(w.project));
        let scripts = shell.scripts();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].0, "local");
        assert!(
            scripts[0].1.contains("#{pane_current_path}"),
            "{}",
            scripts[0].1
        );
        assert!(
            scripts[0].1.contains("'=scratch:'"),
            "the exact pane: {}",
            scripts[0].1
        );
    }

    #[tokio::test]
    async fn a_pane_no_rule_places_is_blank_while_jev_is_off() {
        let w = world();
        let shell = Shell::answering("/home/ada/scratch\n");
        let t = w.target(pane_args(w.pane), &shell).await.unwrap();
        assert_eq!(
            t,
            LostTarget::default(),
            "nothing prefilled, nothing unsure"
        );
        // An unreadable directory is a blank form too, never an error.
        let shell = Shell::answering("no server running\n");
        assert_eq!(
            w.target(pane_args(w.pane), &shell).await.unwrap(),
            LostTarget::default()
        );
    }

    #[tokio::test]
    async fn only_a_pane_fleet_did_not_start_is_asked_about() {
        let w = world();
        lock(&w.store).unwrap().set_started_at(w.pane, 5).unwrap();
        let shell = Shell::answering("/p/acme/papaya-pos\n");
        let err = w.target(pane_args(w.pane), &shell).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        let err = w.target(pane_args(999), &shell).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    #[tokio::test]
    async fn a_found_conversation_is_placed_by_its_directory_first() {
        let w = world();
        let shell = Shell::answering("");
        let t = w
            .target(
                transcript_args("/p/acme/papaya-pos/.worktrees/pd-2412"),
                &shell,
            )
            .await
            .unwrap();
        // The branch also names a ticket (J10's other half, tested below).
        let project_only = |t: LostTarget| LostTarget { ticket: None, ..t };
        assert_eq!(project_only(t), LostTarget::rule(w.project));
        let t = w
            .target(transcript_args("/home/ada/tmp"), &shell)
            .await
            .unwrap();
        assert_eq!(project_only(t), LostTarget::default());
        assert!(
            shell.scripts().is_empty(),
            "a transcript's form reads nothing"
        );
    }

    /// J10's ticket half: the branch's key is proposed beside the project,
    /// by the rule, with the cached title; a branch naming none, and a
    /// pane, carry no ticket.
    #[tokio::test]
    async fn a_found_conversation_gets_the_ticket_its_branch_names() {
        let w = world();
        {
            let s = w.store.lock().unwrap();
            let t = s
                .add_tracker("jira", "Acme", "https://acme.atlassian.net")
                .unwrap()
                .id;
            s.set_tracker_probe(
                t,
                Some("cloud"),
                &crate::store::TrackerConfig {
                    key_prefixes: vec!["PD".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let shell = Shell::answering("");
        // The project's rule and the ticket's together.
        let t = w
            .target(
                transcript_args("/p/acme/papaya-pos/.worktrees/pd-2412"),
                &shell,
            )
            .await
            .unwrap();
        assert_eq!(t.project_id, Some(w.project));
        let ticket = t.ticket.expect("the branch names PD-2412");
        assert_eq!(ticket.key, "PD-2412");
        assert_eq!(ticket.source, "rule");
        assert!(
            ticket.reason.contains("pd-2412-receipt-totals"),
            "{ticket:?}"
        );
        // Outside every project: still the ticket, and no project.
        let t = w
            .target(transcript_args("/home/ada/tmp"), &shell)
            .await
            .unwrap();
        assert_eq!(t.project_id, None);
        assert_eq!(t.ticket.map(|t| t.key).as_deref(), Some("PD-2412"));
        // A branch that names no ticket proposes none.
        let mut plain = transcript_args("/home/ada/tmp");
        plain.git_branch = Some("main".into());
        assert_eq!(w.target(plain, &shell).await.unwrap().ticket, None);
        // A pane carries none: Adopt keeps its project question only.
        let pane = Shell::answering("/home/ada/scratch\n");
        assert_eq!(
            w.target(pane_args(w.pane), &pane).await.unwrap().ticket,
            None
        );
    }

    #[tokio::test]
    async fn a_malformed_entry_is_refused() {
        let w = world();
        let shell = Shell::answering("");
        let none = LostTargetArgs::default();
        assert_eq!(
            w.target(none, &shell).await.unwrap_err().code,
            codes::E_INVALID
        );
        let mut bad = transcript_args("/x");
        bad.claude_session_id = Some("not-a-uuid".into());
        assert!(w.target(bad, &shell).await.is_err());
        let relative = transcript_args("tmp/x");
        assert_eq!(
            w.target(relative, &shell).await.unwrap_err().code,
            codes::E_INVALID
        );
    }

    #[test]
    fn the_place_script_copies_never_overwrites_and_resolves_the_root_physically() {
        let s = place_script("~/projects/github.com/acme/papaya-pos", ID);
        assert!(s.contains("pwd -P"), "{s}");
        assert!(
            s.contains("[ -e \"$d/$id.jsonl\" ]"),
            "never overwrites: {s}"
        );
        assert!(s.contains("cp -p --"), "copies: {s}");
        assert!(!s.contains("mv "), "never moves: {s}");
        assert!(s.contains("'~/'*) root=\"$HOME/"), "expands ~: {s}");
        assert!(s.contains(&format!("id='{ID}'")) || s.contains(&format!("id={ID}")));
    }

    #[tokio::test]
    async fn place_transcript_names_where_to_resume() {
        let w = world();
        let shell = Shell::answering("cf-place:copied\n");
        let placed = place_transcript(
            PlaceTranscriptArgs {
                host_alias: "local".into(),
                claude_session_id: ID.into(),
                project_id: w.project,
            },
            &w.store,
            &shell,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            placed,
            PlacedTranscript {
                project_id: w.project,
                tmux_name: "dev-acme-papaya-pos".into(),
                copied: true,
            }
        );
        assert!(
            shell.scripts()[0].1.contains("root='/p/acme/papaya-pos'"),
            "the project's root on the host: {}",
            shell.scripts()[0].1
        );

        let kept = place_transcript(
            PlaceTranscriptArgs {
                host_alias: "local".into(),
                claude_session_id: ID.into(),
                project_id: w.project,
            },
            &w.store,
            &Shell::answering("cf-place:kept\n"),
            true,
        )
        .await
        .unwrap();
        assert!(!kept.copied);

        // The project's own name is taken on the host: a free one instead.
        w.store
            .lock()
            .unwrap()
            .upsert_session(
                "dev-acme-papaya-pos",
                "local",
                Some(w.project),
                None,
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let again = place_transcript(
            PlaceTranscriptArgs {
                host_alias: "local".into(),
                claude_session_id: ID.into(),
                project_id: w.project,
            },
            &w.store,
            &Shell::answering("cf-place:kept\n"),
            true,
        )
        .await
        .unwrap();
        assert!(
            again.tmux_name.starts_with("dev-acme-papaya-pos--"),
            "{}",
            again.tmux_name
        );
        assert!(w
            .store
            .lock()
            .unwrap()
            .get_session(&again.tmux_name, "local")
            .unwrap()
            .is_none());

        let err = place_transcript(
            PlaceTranscriptArgs {
                host_alias: "local".into(),
                claude_session_id: ID.into(),
                project_id: 999,
            },
            &w.store,
            &Shell::answering("cf-place:copied\n"),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    #[tokio::test]
    async fn adopting_into_a_project_puts_the_row_there() {
        let w = world();
        let row = adopt_session(
            AdoptSessionArgs {
                session_id: w.pane,
                project_id: Some(w.project),
                owner_person_id: None,
                decider: Default::default(),
            },
            &w.store,
        )
        .unwrap();
        assert_eq!(row.project_id, Some(w.project));
        assert!(row.started_at.is_some());

        let w = world();
        let err = adopt_session(
            AdoptSessionArgs {
                session_id: w.pane,
                project_id: Some(999),
                owner_person_id: None,
                decider: Default::default(),
            },
            &w.store,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let row = lock(&w.store)
            .unwrap()
            .get_session_by_id(w.pane)
            .unwrap()
            .unwrap();
        assert!(row.started_at.is_none(), "a refused adopt changes nothing");
    }
}
