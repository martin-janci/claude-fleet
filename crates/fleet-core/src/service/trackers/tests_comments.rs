//! Tracker comments, per adapter, over [`FakeTransport`]: what each reads
//! (and leaves out), and the exact request each sends to post one. No test
//! here reaches a tracker.

use super::asana::Asana;
use super::github::GitHub;
use super::jira::JiraCloud;
use super::jira_common::text_to_adf;
use super::jira_dc::JiraDc;
use super::linear::Linear;
use super::*;
use crate::net::https::{FakeTransport, Method, Request, Response};
use crate::store::{TrackerConfig, TrackerCredential, TrackerSettings};
use serde_json::json;

/// A credential assembled at run time, never a literal token.
fn cred(kind: &str) -> TrackerCredential {
    TrackerCredential {
        auth_kind: kind.into(),
        username: (kind == "basic").then(|| "dev@example.com".into()),
        secret: crate::store::Secret::new(format!("tok{}", "0Ab9".repeat(6))),
    }
}

fn keys(prefixes: &[&str]) -> TrackerConfig {
    TrackerConfig {
        key_prefixes: prefixes.iter().map(|p| p.to_string()).collect(),
        ..Default::default()
    }
}

fn auth(r: &Request) -> &str {
    r.headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default()
}

fn comment(key: &str, external_id: Option<&str>) -> WriteOp {
    WriteOp::Comment {
        key: key.into(),
        external_id: external_id.map(str::to_string),
        body: "Shipped in 1.2\n\n— Ann via fleet".into(),
    }
}

// ── the shared pieces ───────────────────────────────────────────────────

#[test]
fn text_becomes_paragraphs_and_hard_breaks_with_no_markup_read() {
    let doc = text_to_adf("one\ntwo\n\n**three**\r\n\r\n\n");
    assert_eq!(
        doc,
        json!({ "type": "doc", "version": 1, "content": [
            { "type": "paragraph", "content": [
                { "type": "text", "text": "one" },
                { "type": "hardBreak" },
                { "type": "text", "text": "two" },
            ]},
            { "type": "paragraph", "content": [{ "type": "text", "text": "**three**" }] },
        ]})
    );
}

#[test]
fn a_comment_is_capped_and_an_unusable_one_dropped() {
    let long = "x".repeat(TRACKER_COMMENT_MAX_CHARS + 10);
    let c = TrackerComment::from_parts(
        "1",
        &"A".repeat(500),
        &long,
        "2026-10-01T10:00:00Z",
        Some("https://evil.example/x"),
        "https://acme.atlassian.net",
    )
    .unwrap();
    assert_eq!(c.body.chars().count(), TRACKER_COMMENT_MAX_CHARS);
    assert_eq!(c.author.chars().count(), TRACKER_COMMENT_AUTHOR_MAX_CHARS);
    assert_eq!(c.url, None, "a URL off the tracker's own site is not kept");
    for bad_id in ["", "a b", "x/../y", &"9".repeat(201)] {
        assert!(
            TrackerComment::from_parts(bad_id, "a", "b", "2026-10-01T10:00:00Z", None, "")
                .is_none()
        );
    }
    assert!(TrackerComment::from_parts("1", "a", "b", "yesterday", None, "").is_none());
}

#[test]
fn the_newest_are_kept_oldest_first() {
    let at = |id: &str, t: &str| {
        TrackerComment::from_parts(id, "a", "b", &format!("2026-10-0{t}T10:00:00Z"), None, "")
            .unwrap()
    };
    let got = newest_comments(vec![at("3", "3"), at("1", "1"), at("2", "2")], 2);
    assert_eq!(
        got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["2", "3"]
    );
}

// ── Jira Cloud ──────────────────────────────────────────────────────────

fn cloud(f: &FakeTransport) -> JiraCloud {
    JiraCloud::new(
        "https://acme.atlassian.net",
        keys(&["ABC"]),
        Some(cred("basic")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn jira_cloud_reads_adf_comments_newest_fifty_oldest_first() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/rest/api/3/issue/ABC-1/comment?orderBy=-created&maxResults=50",
        Ok(Response::json(
            200,
            &json!({ "comments": [
                { "id": "10002", "author": { "displayName": "Bob" },
                  "body": { "type": "doc", "version": 1, "content": [
                      { "type": "paragraph", "content": [
                          { "type": "text", "text": "Ping " },
                          { "type": "mention", "attrs": { "text": "@Ann" } }]}]},
                  "created": "2026-10-02T09:00:00.000+0000" },
                { "id": "10001", "author": { "displayName": "Ann" },
                  "body": { "type": "doc", "version": 1, "content": [
                      { "type": "paragraph", "content": [{ "type": "text", "text": "First" }]}]},
                  "created": "2026-10-01T09:00:00.000+0200" },
                { "id": "../x", "author": {}, "body": "dropped", "created": "2026-10-01T09:00:00Z" },
            ]}),
        )),
    );
    let p = cloud(&f);
    assert!(p.caps().comments && p.caps().write);
    let got = p
        .comments(&ItemRef::Key("ABC-1".into()), TRACKER_COMMENTS_MAX)
        .await
        .unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].id.as_str(), got[0].body.as_str()),
        ("10001", "First")
    );
    assert_eq!(got[0].created_at, 1_790_838_000);
    assert_eq!(
        (got[1].author.as_str(), got[1].body.as_str()),
        ("Bob", "Ping @Ann")
    );
    assert_eq!(
        got[1].url.as_deref(),
        Some("https://acme.atlassian.net/browse/ABC-1?focusedCommentId=10002")
    );
    // A reference that is not a key is never put in a path.
    assert!(p
        .comments(&ItemRef::Key("../../myself".into()), 5)
        .await
        .is_err());
    assert_eq!(f.requests().len(), 1);
}

#[tokio::test]
async fn jira_cloud_posts_a_comment_as_adf_with_basic_auth() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/rest/api/3/issue/ABC-1/comment",
        Ok(Response::json(201, &json!({ "id": "10050", "self": "x" }))),
    );
    let out = cloud(&f)
        .write(&comment("ABC-1", Some("10001")))
        .await
        .unwrap();
    assert_eq!(out.remote_id.as_deref(), Some("10050"));
    assert_eq!(
        out.url.as_deref(),
        Some("https://acme.atlassian.net/browse/ABC-1?focusedCommentId=10050")
    );
    let sent = f.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::Post);
    assert!(auth(&sent[0]).starts_with("Basic "));
    assert_eq!(
        sent[0].json_body().unwrap(),
        json!({ "body": text_to_adf("Shipped in 1.2\n\n— Ann via fleet") })
    );
    let f = FakeTransport::new();
    assert!(cloud(&f).write(&comment("abc 1", None)).await.is_err());
    assert!(f.requests().is_empty());
}

// ── Jira Data Center ────────────────────────────────────────────────────

fn dc(f: &FakeTransport) -> JiraDc {
    JiraDc::new(
        "https://jira.corp.example/jira",
        keys(&["OPS"]),
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn jira_dc_reads_plain_comments_through_api_v2() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/jira/rest/api/2/issue/OPS-7/comment?orderBy=-created&maxResults=50",
        Ok(Response::json(
            200,
            &json!({ "comments": [
                { "id": "301", "author": { "displayName": "Dee" }, "body": "Looks *fine*",
                  "created": "2026-10-03T08:00:00.000+0000" },
            ]}),
        )),
    );
    let got = dc(&f)
        .comments(&ItemRef::Key("OPS-7".into()), TRACKER_COMMENTS_MAX)
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].body, "Looks *fine*");
    assert_eq!(
        got[0].url.as_deref(),
        Some("https://jira.corp.example/jira/browse/OPS-7?focusedCommentId=301")
    );
}

#[tokio::test]
async fn jira_dc_posts_a_string_body_with_its_pat() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/jira/rest/api/2/issue/OPS-7/comment",
        Ok(Response::json(201, &json!({ "id": "302" }))),
    );
    let out = dc(&f).write(&comment("OPS-7", None)).await.unwrap();
    assert_eq!(out.remote_id.as_deref(), Some("302"));
    let sent = f.requests();
    assert!(auth(&sent[0]).starts_with("Bearer "));
    assert_eq!(
        sent[0].json_body().unwrap(),
        json!({ "body": "Shipped in 1.2\n\n— Ann via fleet" })
    );
}

// ── GitHub ──────────────────────────────────────────────────────────────

fn github(f: &FakeTransport) -> GitHub {
    GitHub::new(
        "https://github.com/acme",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn github_reads_an_issues_last_comments_by_node_id() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "node": { "comments": { "nodes": [
                { "id": "IC_kwDOA1", "body": "On it", "url": "https://github.com/acme/api/issues/42#issuecomment-1",
                  "createdAt": "2026-10-01T10:00:00Z", "author": { "login": "dana" } },
                { "id": "IC_kwDOA2", "body": "Ghosted", "url": "https://evil.example/x",
                  "createdAt": "2026-10-02T10:00:00Z", "author": null },
            ]}}}}),
        )),
    );
    let got = github(&f)
        .comments(&ItemRef::Id("I_kwDOAcme0001".into()), TRACKER_COMMENTS_MAX)
        .await
        .unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].author, "dana");
    assert!(got[0]
        .url
        .as_deref()
        .unwrap()
        .starts_with("https://github.com/acme/"));
    assert_eq!(
        (got[1].author.as_str(), got[1].url.as_deref()),
        ("ghost", None)
    );
    let req = f.requests()[0].json_body().unwrap();
    assert!(req["query"]
        .as_str()
        .unwrap()
        .contains("comments(last: 50)"));
    assert_eq!(req["variables"]["id"], "I_kwDOAcme0001");
}

#[tokio::test]
async fn github_posts_add_comment_on_the_issue_node() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "addComment": { "commentEdge": { "node": {
                "id": "IC_kwDOA9", "url": "https://github.com/acme/api/issues/42#issuecomment-9" }}}}}),
        )),
    );
    let out = github(&f)
        .write(&comment("acme/api#42", Some("I_kwDOAcme0001")))
        .await
        .unwrap();
    assert_eq!(out.remote_id.as_deref(), Some("IC_kwDOA9"));
    assert!(out.url.unwrap().ends_with("#issuecomment-9"));
    let sent = f.requests();
    assert_eq!(sent.len(), 1);
    let req = sent[0].json_body().unwrap();
    assert!(req["query"].as_str().unwrap().starts_with("mutation"));
    assert!(req["query"].as_str().unwrap().contains("addComment"));
    assert_eq!(req["variables"]["id"], "I_kwDOAcme0001");
    assert_eq!(
        req["variables"]["body"],
        "Shipped in 1.2\n\n— Ann via fleet"
    );
    assert_eq!(auth(&sent[0]), "", "gh's own login, never a token");
    // GitHub takes no PR remote link.
    let op = WriteOp::PrRemoteLink {
        key: "acme/api#42".into(),
        url: "https://github.com/acme/api/pull/1".into(),
        title: "PR".into(),
    };
    assert!(matches!(
        github(&f).write(&op).await,
        Err(TrackerError::Refused(_))
    ));
}

// ── Linear ──────────────────────────────────────────────────────────────

fn linear(f: &FakeTransport) -> Linear {
    Linear::new(
        "https://linear.app/acme",
        keys(&["ENG"]),
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn linear_reads_an_issues_comments_by_id() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "issue": { "comments": { "nodes": [
                { "id": "7f1c2b9e-0000-4000-8000-000000000002", "body": "Second",
                  "url": "https://linear.app/acme/issue/ENG-12#comment-2",
                  "createdAt": "2026-10-02T10:00:00.000Z", "user": { "name": "Lin" } },
                { "id": "7f1c2b9e-0000-4000-8000-000000000001", "body": "First",
                  "url": "https://linear.app/acme/issue/ENG-12#comment-1",
                  "createdAt": "2026-10-01T10:00:00.000Z", "user": null },
            ]}}}}),
        )),
    );
    let got = linear(&f)
        .comments(
            &ItemRef::Id("9b2d0000-0000-4000-8000-00000000000c".into()),
            50,
        )
        .await
        .unwrap();
    assert_eq!(
        got.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(got[0].author, "Linear");
    let sent = f.requests();
    assert!(
        !auth(&sent[0]).starts_with("Bearer"),
        "Linear's key goes as it is"
    );
    let req = sent[0].json_body().unwrap();
    assert!(req["query"]
        .as_str()
        .unwrap()
        .contains("comments(last: 50)"));
}

#[tokio::test]
async fn linear_posts_comment_create_on_the_issue_id() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "commentCreate": { "success": true, "comment": {
                "id": "7f1c2b9e-0000-4000-8000-000000000009",
                "url": "https://linear.app/acme/issue/ENG-12#comment-9" }}}}),
        )),
    );
    let out = linear(&f)
        .write(&comment(
            "ENG-12",
            Some("9b2d0000-0000-4000-8000-00000000000c"),
        ))
        .await
        .unwrap();
    assert_eq!(
        out.remote_id.as_deref(),
        Some("7f1c2b9e-0000-4000-8000-000000000009")
    );
    let req = f.requests()[0].json_body().unwrap();
    assert!(req["query"].as_str().unwrap().contains("commentCreate"));
    assert_eq!(
        req["variables"]["id"],
        "9b2d0000-0000-4000-8000-00000000000c"
    );
    assert_eq!(
        req["variables"]["body"],
        "Shipped in 1.2\n\n— Ann via fleet"
    );
    // Linear said no: an error, never a "done".
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/graphql",
        Ok(Response::json(
            200,
            &json!({ "data": { "commentCreate": { "success": false, "comment": null }}}),
        )),
    );
    assert!(linear(&f).write(&comment("ENG-12", None)).await.is_err());
}

// ── Asana ───────────────────────────────────────────────────────────────

fn asana(f: &FakeTransport) -> Asana {
    Asana::new(
        "https://app.asana.com",
        TrackerConfig::default(),
        TrackerSettings::default(),
        Some(cred("bearer")),
        Arc::new(f.clone()),
    )
}

#[tokio::test]
async fn asana_keeps_only_the_comment_stories_across_pages() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/tasks/1200000000002001/stories?limit=100&opt_fields=resource_subtype,text,created_by.name,created_at",
        Ok(Response::json(
            200,
            &json!({ "data": [
                { "gid": "1200000000009001", "resource_subtype": "assigned",
                  "text": "Ann assigned to you", "created_at": "2026-10-01T09:00:00.000Z",
                  "created_by": { "name": "Ann" } },
                { "gid": "1200000000009002", "resource_subtype": "comment_added",
                  "text": "Can we ship Friday?", "created_at": "2026-10-01T10:00:00.000Z",
                  "created_by": { "name": "Ann" } },
            ], "next_page": { "offset": "eyJ0eXAi" } }),
        )),
    );
    f.once(
        Method::Get,
        "&offset=eyJ0eXAi",
        Ok(Response::json(
            200,
            &json!({ "data": [
                { "gid": "1200000000009003", "resource_subtype": "section_changed",
                  "text": "moved", "created_at": "2026-10-02T09:00:00.000Z" },
                { "gid": "1200000000009004", "resource_subtype": "comment_added",
                  "text": "Yes", "created_at": "2026-10-02T10:00:00.000Z",
                  "created_by": { "name": "Bo" } },
            ], "next_page": null }),
        )),
    );
    let got = asana(&f)
        .comments(&ItemRef::Id("1200000000002001".into()), 50)
        .await
        .unwrap();
    assert_eq!(
        got.iter()
            .map(|c| (c.id.as_str(), c.body.as_str()))
            .collect::<Vec<_>>(),
        [
            ("1200000000009002", "Can we ship Friday?"),
            ("1200000000009004", "Yes")
        ]
    );
    assert_eq!(f.requests().len(), 2);
}

#[tokio::test]
async fn asana_posts_a_story_with_its_pat() {
    let f = FakeTransport::new();
    f.once(
        Method::Post,
        "/api/1.0/tasks/1200000000002001/stories",
        Ok(Response::json(
            201,
            &json!({ "data": { "gid": "1200000000009100", "resource_subtype": "comment_added" } }),
        )),
    );
    let out = asana(&f)
        .write(&comment("asana:1200000000002001", Some("1200000000002001")))
        .await
        .unwrap();
    assert_eq!(out.remote_id.as_deref(), Some("1200000000009100"));
    let sent = f.requests();
    assert!(auth(&sent[0]).starts_with("Bearer "));
    assert_eq!(
        sent[0].json_body().unwrap(),
        json!({ "data": { "text": "Shipped in 1.2\n\n— Ann via fleet" } })
    );
    // A gid that is not one is never put in a path.
    let f = FakeTransport::new();
    assert!(asana(&f)
        .write(&comment("asana:../me", Some("../me")))
        .await
        .is_err());
    assert!(f.requests().is_empty());
}

#[tokio::test]
async fn asana_lists_a_tasks_attachments_with_their_links() {
    let f = FakeTransport::new();
    f.once(
        Method::Get,
        "/attachments?parent=1200000000002001&limit=100&opt_fields=name,permanent_url,view_url,size,created_at",
        Ok(Response::json(
            200,
            &json!({ "data": [
                { "gid": "1200000000008002", "name": "spec\nv2.pdf",
                  "permanent_url": "https://app.asana.com/app/asana/-/get_asset?asset_id=1200000000008002",
                  "size": 2048, "created_at": "2026-10-02T09:00:00.000Z" },
                { "gid": "1200000000008001", "name": "screenshot.png",
                  "permanent_url": "",
                  "view_url": "https://drive.google.com/file/d/abc/view",
                  "created_at": "2026-10-01T09:00:00.000Z" },
                { "gid": "1200000000008003", "name": "notes",
                  "permanent_url": "javascript:alert(1)" },
                { "gid": "../me", "name": "bad id" },
            ] }),
        )),
    );
    let got = asana(&f)
        .attachments(&ItemRef::Id("1200000000002001".into()), 50)
        .await
        .unwrap();
    assert_eq!(
        got.iter()
            .map(|a| (a.name.as_str(), a.url.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("notes", None),
            (
                "screenshot.png",
                Some("https://drive.google.com/file/d/abc/view")
            ),
            (
                "spec v2.pdf",
                Some("https://app.asana.com/app/asana/-/get_asset?asset_id=1200000000008002")
            ),
        ]
    );
    assert_eq!(got[2].size, Some(2048));
}
