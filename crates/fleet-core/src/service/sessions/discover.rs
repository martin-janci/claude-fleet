//! Host-reboot recovery, discovery half (Task 7 of the host-reboot recovery
//! plan): parse and rank the Claude transcripts a host actually has on disk,
//! independent of anything fleet's own DB knows about. Pure/read-only — no
//! ssh, no store, no writes. Task 8 wires this to a live host (running
//! [`crate::tmux::discover_transcripts_script`] over ssh/local exec) and
//! fills in `derived_tmux_name`/`project_id`/`worktree_id`/
//! `existing_session_id`, which this module deliberately leaves `None`.

use serde::Serialize;

/// One transcript found by [`crate::tmux::discover_transcripts_script`]:
/// its Claude session id (the `.jsonl` file's basename, already validated
/// via [`crate::validate::claude_session_id`]), its mtime, and whatever
/// `cwd`/`gitBranch` could be recovered from its last `"cwd"`-bearing line.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptProbe {
    pub claude_session_id: String,
    pub mtime: i64,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

/// Find the JSON string literal value of `"<key>":"<value>"` in `text`,
/// starting the search at `key` (which must include the trailing colon,
/// e.g. `"cwd":`). Skips whitespace after the key, requires the next
/// non-whitespace character to be `"`, then scans for the matching
/// unescaped closing quote (byte-wise: multi-byte UTF-8 continuation bytes
/// never collide with the ASCII `\` / `"` we scan for) and decodes that
/// slice with `serde_json::from_str::<String>` so backslash escapes resolve
/// correctly. Absent key, no opening quote, or no matching closing quote
/// (e.g. the transcript line was truncated mid-string by `cut -c1-8192`)
/// all yield `None`.
fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    let idx = text.find(key)?;
    let after = text[idx + key.len()..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let bytes = after.as_bytes();
    let mut i = 1;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if escaped {
            escaped = false;
        } else if c == b'\\' {
            escaped = true;
        } else if c == b'"' {
            return serde_json::from_str::<String>(&after[..=i]).ok();
        }
        i += 1;
    }
    None
}

/// Parse macOS's `sysctl -n kern.boottime` output, e.g.
/// `{ sec = 1726000000, usec = 0 } Thu Sep 11 00:00:00 2025`: the integer
/// run of digits right after `sec = `. Empty (sysctl failed, redirected to
/// `/dev/null` by the script) or malformed input yields `None`.
fn parse_bootraw(value: &str) -> Option<i64> {
    let after = value.split_once("sec = ")?.1;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok()
}

/// Parse [`crate::tmux::discover_transcripts_script`]'s stdout into a boot
/// epoch (`bootsec=`/`bootraw=` line — see [`parse_bootraw`] for the latter)
/// and the list of transcripts it found. Every transcript is announced by an
/// `@@F\t<mtime>\t<claude session id>` line, immediately followed by an
/// `@@L\t<line>` line carrying the transcript's last `"cwd"`-bearing line
/// (verbatim, possibly truncated); `cwd`/`gitBranch` are pulled out of it via
/// [`extract_json_string_field`]. A probe is dropped entirely when its id
/// fails [`crate::validate::claude_session_id`] or its mtime does not parse
/// as `i64` — in that case the following `@@L` line (if any) is also
/// ignored, since it belongs to the dropped probe. Any other line (login
/// banners, stray shell noise) is ignored.
pub fn parse_discover_output(stdout: &str) -> (Option<i64>, Vec<TranscriptProbe>) {
    let mut boot: Option<i64> = None;
    let mut probes: Vec<TranscriptProbe> = Vec::new();
    let mut current: Option<TranscriptProbe> = None;

    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("bootsec=") {
            boot = v.trim().parse::<i64>().ok();
        } else if let Some(v) = line.strip_prefix("bootraw=") {
            boot = parse_bootraw(v);
        } else if let Some(rest) = line.strip_prefix("@@F\t") {
            if let Some(p) = current.take() {
                probes.push(p);
            }
            let mut parts = rest.splitn(2, '\t');
            let mtime = parts.next().and_then(|s| s.trim().parse::<i64>().ok());
            let id = parts.next().unwrap_or("");
            current = match mtime {
                Some(mtime) if crate::validate::claude_session_id(id).is_ok() => {
                    Some(TranscriptProbe {
                        claude_session_id: id.to_string(),
                        mtime,
                        cwd: None,
                        git_branch: None,
                    })
                }
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("@@L\t") {
            if let Some(p) = current.as_mut() {
                p.cwd = extract_json_string_field(rest, "\"cwd\":");
                p.git_branch = extract_json_string_field(rest, "\"gitBranch\":");
            }
        }
        // Anything else (login banners, stray shell noise) is ignored.
    }
    if let Some(p) = current.take() {
        probes.push(p);
    }
    (boot, probes)
}

/// One ranked candidate for lost-session restore: a transcript, deduplicated
/// by `cwd`, with a hint for how it relates to the host's last boot. Task 7
/// leaves `derived_tmux_name`/`project_id`/`worktree_id`/
/// `existing_session_id` as `None` — Task 8 (which has the store and the
/// fleet naming scheme) fills them in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LostCandidate {
    pub cwd: String,
    pub git_branch: Option<String>,
    pub claude_session_id: String,
    pub transcript_mtime: i64,
    pub derived_tmux_name: Option<String>,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub existing_session_id: Option<i64>,
    pub rank_hint: String,
}

/// `rank_hint` for one transcript's mtime against the host's boot epoch:
/// `"unknown"` when the boot epoch could not be read at all, `"after_boot"`
/// when the transcript was touched at or after boot, `"before_boot"` when it
/// was touched at most 24h before boot (a session that was very likely still
/// live right up to the reboot), else `"stale"`.
fn rank_hint_for(boot: Option<i64>, mtime: i64) -> &'static str {
    match boot {
        None => "unknown",
        Some(boot) => {
            if mtime >= boot {
                "after_boot"
            } else if boot - mtime <= 86_400 {
                "before_boot"
            } else {
                "stale"
            }
        }
    }
}

fn hint_order(hint: &str) -> u8 {
    match hint {
        "before_boot" => 0,
        "after_boot" => 1,
        "stale" => 2,
        _ => 3, // "unknown"
    }
}

/// Turn discovered transcripts into ranked, deduplicated candidates: probes
/// with no `cwd` are dropped outright (nothing to restore into), the rest
/// are grouped by `cwd` keeping only the one with the max `mtime` (a tie —
/// same `cwd`, same `mtime` — keeps the lexicographically larger session
/// id), and the result is ordered `before_boot`, then `after_boot`, then
/// `stale`, then `unknown`, newest (`mtime` desc) first within each group.
/// A `BTreeMap` keyed by `cwd` is used (not a `HashMap`) so the grouping
/// pass, and therefore any tie in the final sort, is deterministic.
pub fn rank_candidates(boot: Option<i64>, probes: Vec<TranscriptProbe>) -> Vec<LostCandidate> {
    let mut best: std::collections::BTreeMap<String, TranscriptProbe> =
        std::collections::BTreeMap::new();
    for p in probes {
        let Some(cwd) = p.cwd.clone() else {
            continue;
        };
        let keep = match best.get(&cwd) {
            None => true,
            Some(existing) => {
                (p.mtime, p.claude_session_id.as_str())
                    > (existing.mtime, existing.claude_session_id.as_str())
            }
        };
        if keep {
            best.insert(cwd, p);
        }
    }

    let mut candidates: Vec<LostCandidate> = best
        .into_iter()
        .map(|(cwd, p)| LostCandidate {
            cwd,
            git_branch: p.git_branch,
            claude_session_id: p.claude_session_id,
            transcript_mtime: p.mtime,
            derived_tmux_name: None,
            project_id: None,
            worktree_id: None,
            existing_session_id: None,
            rank_hint: rank_hint_for(boot, p.mtime).to_string(),
        })
        .collect();

    candidates.sort_by(|a, b| {
        hint_order(&a.rank_hint)
            .cmp(&hint_order(&b.rank_hint))
            .then(b.transcript_mtime.cmp(&a.transcript_mtime))
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(id: &str, mtime: i64, cwd: &str) -> TranscriptProbe {
        TranscriptProbe {
            claude_session_id: id.to_string(),
            mtime,
            cwd: Some(cwd.to_string()),
            git_branch: None,
        }
    }

    // ---- parse_discover_output -------------------------------------------------

    #[test]
    fn truncated_message_still_yields_cwd_and_branch() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let last_line = r#"{"parentUuid":"x","cwd":"/a/b \"q\"","sessionId":"abc","gitBranch":"feat/x","message":{"content":"abc"#;
        let stdout = format!("bootsec=1000\n@@F\t123\t{id}\n@@L\t{last_line}\n");

        let (boot, probes) = parse_discover_output(&stdout);

        assert_eq!(boot, Some(1000));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].claude_session_id, id);
        assert_eq!(probes[0].mtime, 123);
        assert_eq!(probes[0].cwd.as_deref(), Some("/a/b \"q\""));
        assert_eq!(probes[0].git_branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn bootraw_macos_format_is_parsed() {
        let (boot, probes) =
            parse_discover_output("bootraw={ sec = 1726000000, usec = 0 } Thu Sep 11 2025\n");
        assert_eq!(boot, Some(1_726_000_000));
        assert!(probes.is_empty());
    }

    #[test]
    fn bootraw_empty_when_sysctl_failed_is_unknown() {
        let (boot, _probes) = parse_discover_output("bootraw=\n");
        assert_eq!(boot, None);
    }

    #[test]
    fn invalid_session_id_drops_the_probe() {
        let stdout = "bootsec=1\n@@F\t100\tnot-a-uuid\n@@L\t{\"cwd\":\"/x\"}\n";
        let (boot, probes) = parse_discover_output(stdout);
        assert_eq!(boot, Some(1));
        assert!(probes.is_empty(), "{probes:?}");
    }

    #[test]
    fn unparseable_mtime_drops_the_probe() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout = format!("@@F\tNaN\t{id}\n@@L\t{{\"cwd\":\"/z\"}}\n");
        let (boot, probes) = parse_discover_output(&stdout);
        assert_eq!(boot, None);
        assert!(probes.is_empty(), "{probes:?}");
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout =
            format!("some login banner\nbootsec=42\nnoise\n@@F\t7\t{id}\n@@L\t{{\"cwd\":\"/z\"}}\nmore noise\n");
        let (boot, probes) = parse_discover_output(&stdout);
        assert_eq!(boot, Some(42));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cwd.as_deref(), Some("/z"));
    }

    #[test]
    fn missing_cwd_or_branch_line_is_none() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout = format!("@@F\t5\t{id}\n@@L\t\n");
        let (_boot, probes) = parse_discover_output(&stdout);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cwd, None);
        assert_eq!(probes[0].git_branch, None);
    }

    // ---- rank_candidates ---------------------------------------------------

    #[test]
    fn duplicate_cwd_keeps_the_newest() {
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", 100, "/a"),
            probe("22222222-2222-2222-2222-222222222222", 200, "/a"),
            probe("00000000-0000-0000-0000-000000000000", 150, "/a"),
        ];
        let out = rank_candidates(Some(1_000_000), probes);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].claude_session_id,
            "22222222-2222-2222-2222-222222222222"
        );
        assert_eq!(out[0].transcript_mtime, 200);
    }

    #[test]
    fn duplicate_cwd_and_mtime_tie_breaks_on_larger_id() {
        let probes = vec![
            probe("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 100, "/a"),
            probe("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", 100, "/a"),
        ];
        let out = rank_candidates(Some(1_000_000), probes);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].claude_session_id,
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        );
    }

    #[test]
    fn probes_without_cwd_are_dropped() {
        let mut p = probe("11111111-1111-1111-1111-111111111111", 100, "/a");
        p.cwd = None;
        let out = rank_candidates(Some(1_000_000), vec![p]);
        assert!(out.is_empty());
    }

    #[test]
    fn rank_hint_boundaries() {
        let boot = 1_000_000i64;
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", boot, "/after"),
            probe(
                "22222222-2222-2222-2222-222222222222",
                boot - 86_400,
                "/before-edge",
            ),
            probe(
                "33333333-3333-3333-3333-333333333333",
                boot - 86_401,
                "/stale-edge",
            ),
        ];
        let out = rank_candidates(Some(boot), probes);
        let hint_for = |cwd: &str| {
            out.iter()
                .find(|c| c.cwd == cwd)
                .unwrap_or_else(|| panic!("no candidate for {cwd}"))
                .rank_hint
                .clone()
        };
        assert_eq!(hint_for("/after"), "after_boot");
        assert_eq!(hint_for("/before-edge"), "before_boot");
        assert_eq!(hint_for("/stale-edge"), "stale");
    }

    #[test]
    fn ordering_across_groups() {
        let boot = 1_000_000i64;
        let probes = vec![
            probe(
                "11111111-1111-1111-1111-111111111111",
                boot - 90_000,
                "/stale-old",
            ),
            probe(
                "22222222-2222-2222-2222-222222222222",
                boot + 500,
                "/after-new",
            ),
            probe(
                "33333333-3333-3333-3333-333333333333",
                boot - 100,
                "/before-new",
            ),
            probe(
                "44444444-4444-4444-4444-444444444444",
                boot + 100,
                "/after-old",
            ),
            probe(
                "55555555-5555-5555-5555-555555555555",
                boot - 87_000,
                "/stale-new",
            ),
        ];
        let out = rank_candidates(Some(boot), probes);
        let order: Vec<&str> = out.iter().map(|c| c.cwd.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "/before-new",
                "/after-new",
                "/after-old",
                "/stale-new",
                "/stale-old",
            ]
        );
    }

    #[test]
    fn none_boot_marks_everything_unknown_ordered_by_mtime() {
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", 100, "/a"),
            probe("22222222-2222-2222-2222-222222222222", 300, "/b"),
            probe("33333333-3333-3333-3333-333333333333", 200, "/c"),
        ];
        let out = rank_candidates(None, probes);
        assert!(out.iter().all(|c| c.rank_hint == "unknown"), "{out:?}");
        let order: Vec<&str> = out.iter().map(|c| c.cwd.as_str()).collect();
        assert_eq!(order, vec!["/b", "/c", "/a"]);
    }
}
