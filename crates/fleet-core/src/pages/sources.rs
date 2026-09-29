//! Data sources: the named, read-only backend queries a page may show (design
//! §3, "Beyond settings"). A source declares its parameters and the shape of
//! what it returns, so the validator can check that a `chart` gets a series
//! and a `table` gets rows before anything runs. A page can never run a
//! query of its own: it names a source here, with literal parameters.
//!
//! Access: every source below reads the whole fleet (its usage, its work
//! graph's retention and usage counts), so it is for the process that owns the fleet (a
//! standalone desktop, or a master token on a hub), exactly like
//! `usage_report` and `work_admin`. An org-scoped variant is a new source,
//! not a parameter.

use crate::ipc_error::{codes, IpcError};
use crate::service::usage;
use crate::store::{Store, UsageTotals};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// How a column's value is formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColType {
    Text,
    Int,
    Tokens,
    /// Micro-USD, shown as dollars.
    UsdMicros,
    /// `YYYY-MM-DD`.
    Day,
    /// Unix seconds, shown as how long ago; `null` is "never".
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Column {
    pub id: &'static str,
    pub label: &'static str,
    pub ty: ColType,
}

const fn col(id: &'static str, label: &'static str, ty: ColType) -> Column {
    Column { id, label, ty }
}

/// What a source returns. `Record` is one JSON object with `fields`; `Rows`
/// is an array of such objects; `Series` is an array of points with an `x`
/// and every `y`; `Scalar` is one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    Scalar { ty: ColType },
    Record { fields: &'static [Column] },
    Rows { columns: &'static [Column] },
    Series { x: Column, y: &'static [Column] },
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Scalar { .. } => "scalar",
            Shape::Record { .. } => "record",
            Shape::Rows { .. } => "rows",
            Shape::Series { .. } => "series",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamType {
    /// A whole number of days in `min..=max`.
    Days { min: u64, max: u64 },
    /// A registered host's alias (checked for syntax here, for existence
    /// when the source runs).
    HostAlias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ParamSpec {
    pub name: &'static str,
    pub ty: ParamType,
    /// Used when the page names none; `None` means "not set".
    pub default: Option<u64>,
    pub help: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SourceSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    #[serde(flatten)]
    pub shape: Shape,
    pub params: &'static [ParamSpec],
}

const TOKEN_COLUMNS: &[Column] = &[
    col("input_tokens", "Input", ColType::Tokens),
    col("output_tokens", "Output", ColType::Tokens),
    col("cache_write_tokens", "Cache write", ColType::Tokens),
    col("cache_read_tokens", "Cache read", ColType::Tokens),
    col("cost_micros", "Est. cost", ColType::UsdMicros),
];

const HOST_PARAM: ParamSpec = ParamSpec {
    name: "host",
    ty: ParamType::HostAlias,
    default: None,
    help: "Only this host; every host when unset.",
};

/// Every data source. Order is the order `describe_sources` lists them in.
pub const SOURCES: &[SourceSpec] = &[
    SourceSpec {
        id: "usage.total",
        label: "Token usage",
        help: "Tokens and estimated cost over every session row that still exists, each over its whole lifetime.",
        shape: Shape::Record {
            fields: TOKEN_COLUMNS,
        },
        params: &[HOST_PARAM],
    },
    SourceSpec {
        id: "usage.by_day",
        label: "Usage by day",
        help: "Tokens and estimated cost per UTC day from the durable roll-up, killed sessions included.",
        shape: Shape::Series {
            x: col("day", "Day", ColType::Day),
            y: &[
                col("cost_micros", "Est. cost", ColType::UsdMicros),
                col("input_tokens", "Input", ColType::Tokens),
                col("output_tokens", "Output", ColType::Tokens),
            ],
        },
        params: &[
            ParamSpec {
                name: "days",
                ty: ParamType::Days { min: 1, max: 365 },
                default: Some(14),
                help: "How many UTC days back, today included.",
            },
            HOST_PARAM,
        ],
    },
    SourceSpec {
        id: "usage.by_host",
        label: "Usage by host",
        help: "Tokens and estimated cost per host over the session rows that still exist.",
        shape: Shape::Rows {
            columns: &[
                col("host", "Host", ColType::Text),
                col("input_tokens", "Input", ColType::Tokens),
                col("output_tokens", "Output", ColType::Tokens),
                col("cost_micros", "Est. cost", ColType::UsdMicros),
            ],
        },
        params: &[],
    },
    SourceSpec {
        id: "usage.by_model",
        label: "Usage by model",
        help: "Tokens and estimated cost per model over the session rows that still exist, by each session's latest model.",
        shape: Shape::Rows {
            columns: &[
                col("model", "Model", ColType::Text),
                col("sessions", "Sessions", ColType::Int),
                col("input_tokens", "Input", ColType::Tokens),
                col("output_tokens", "Output", ColType::Tokens),
                col("cost_micros", "Est. cost", ColType::UsdMicros),
            ],
        },
        params: &[HOST_PARAM],
    },
    SourceSpec {
        id: "work.retention",
        label: "Work retention",
        help: "Per swept table: its window, its rows, and what a sweep would delete now (a dry run).",
        shape: Shape::Rows {
            columns: &[
                col("kept", "Kept", ColType::Text),
                col("days", "Window (days, 0 = forever)", ColType::Int),
                col("rows", "Rows", ColType::Int),
                col("would_delete", "Would delete now", ColType::Int),
            ],
        },
        params: &[],
    },
    SourceSpec {
        id: "work.usage",
        label: "Work graph usage",
        help: "How the work graph was used over the window: links, detection, handovers, resumes, the journal, tidy-up and each tracker's sync, as counts (`fleet-hub work usage`).",
        shape: Shape::Rows {
            columns: &[
                col("group", "What", ColType::Text),
                col("counted", "Counted", ColType::Text),
            ],
        },
        params: &[ParamSpec {
            name: "days",
            ty: ParamType::Days {
                min: 1,
                max: crate::service::work::usage::MAX_DAYS as u64,
            },
            default: Some(crate::service::work::usage::DEFAULT_DAYS as u64),
            help: "How many days back.",
        }],
    },
    SourceSpec {
        id: "work.retention_last",
        label: "Last retention sweep",
        help: "When the last sweep ran and what it deleted.",
        shape: Shape::Record {
            fields: &[
                col("at", "Last sweep", ColType::Time),
                col("journal", "Journal entries", ColType::Int),
                col("tracker_items", "Done tickets", ColType::Int),
                col("timeline_work_events", "Timeline events", ColType::Int),
                col("tracker_writes", "PR links", ColType::Int),
                col("describe_cache", "Full descriptions", ColType::Int),
            ],
        },
        params: &[],
    },
];

pub fn source(id: &str) -> Option<&'static SourceSpec> {
    SOURCES.iter().find(|s| s.id == id)
}

/// Resolve `given` against `spec`'s parameters: every name known, every
/// value of its type, defaults filled in. The same check the validator runs
/// on a page's literal params and `fetch` runs on a caller's.
pub fn resolve_params(
    spec: &SourceSpec,
    given: &Map<String, Value>,
) -> Result<BTreeMap<&'static str, Value>, IpcError> {
    let bad = |msg: String| IpcError::new(codes::E_INVALID, format!("{}: {msg}", spec.id));
    for name in given.keys() {
        if !spec.params.iter().any(|p| p.name == name) {
            return Err(bad(format!("unknown parameter `{name}`")));
        }
    }
    let mut out = BTreeMap::new();
    for p in spec.params {
        let Some(v) = given.get(p.name).filter(|v| !v.is_null()) else {
            if let Some(d) = p.default {
                out.insert(p.name, json!(d));
            }
            continue;
        };
        match p.ty {
            ParamType::Days { min, max } => match v.as_u64() {
                Some(n) if (min..=max).contains(&n) => {
                    out.insert(p.name, json!(n));
                }
                _ => {
                    return Err(bad(format!(
                        "`{}` must be a whole number of days, {min}–{max}",
                        p.name
                    )))
                }
            },
            ParamType::HostAlias => {
                let alias = v
                    .as_str()
                    .ok_or_else(|| bad(format!("`{}` must be a host alias", p.name)))?;
                crate::validate::host_alias_syntax(alias)
                    .map_err(|e| bad(format!("`{}`: {}", p.name, e.message)))?;
                out.insert(p.name, json!(alias));
            }
        }
    }
    Ok(out)
}

/// A swept table as a person names it.
fn retention_label(table: &str) -> &str {
    match table {
        "work_journal" => "Work journal",
        "work_items" => "Done tickets",
        "session_events" => "Work timeline",
        "work_item_descriptions" => "Full descriptions",
        // The write-back outbox: a write still waiting to be sent is never
        // swept, so its "would delete" counts settled (sent or given-up)
        // rows only.
        "tracker_writes" => "PR link outbox (sent or given up)",
        other => other,
    }
}

fn totals_json(t: &UsageTotals) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("input_tokens".into(), json!(t.input_tokens));
    m.insert("output_tokens".into(), json!(t.output_tokens));
    m.insert("cache_write_tokens".into(), json!(t.cache_write_tokens));
    m.insert("cache_read_tokens".into(), json!(t.cache_read_tokens));
    m.insert("cost_micros".into(), json!(t.cost_micros));
    m
}

/// Run source `id` with `params` at `now` (unix seconds). The result has the
/// source's declared shape (`sources_tests` holds every source to it).
pub fn fetch(
    s: &Store,
    id: &str,
    params: &Map<String, Value>,
    now: i64,
) -> Result<Value, IpcError> {
    let spec = source(id)
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("unknown data source {id}")))?;
    let p = resolve_params(spec, params)?;
    let host = p.get("host").and_then(Value::as_str);
    match spec.id {
        "usage.total" => {
            let r = usage::report(s, host, None, now)?;
            Ok(Value::Object(totals_json(&r.total)))
        }
        "usage.by_day" => {
            let days = p.get("days").and_then(Value::as_u64).unwrap_or(14) as i64;
            Ok(Value::Array(
                usage::recent_days(s, now, days, host)
                    .into_iter()
                    .map(|d| {
                        json!({
                            "day": d.day,
                            "cost_micros": d.totals.cost_micros,
                            "input_tokens": d.totals.input_tokens,
                            "output_tokens": d.totals.output_tokens,
                        })
                    })
                    .collect(),
            ))
        }
        "usage.by_host" => {
            let r = usage::report(s, None, None, now)?;
            Ok(Value::Array(
                r.by_host
                    .iter()
                    .map(|(h, t)| {
                        json!({
                            "host": h,
                            "input_tokens": t.input_tokens,
                            "output_tokens": t.output_tokens,
                            "cost_micros": t.cost_micros,
                        })
                    })
                    .collect(),
            ))
        }
        "usage.by_model" => {
            let mut by: BTreeMap<String, (i64, UsageTotals)> = BTreeMap::new();
            for row in s.list_all_sessions()? {
                let t = row.usage.totals();
                if t.is_zero() || host.is_some_and(|h| row.host_alias != h) {
                    continue;
                }
                let model = row.usage.usage_model.unwrap_or_else(|| "unknown".into());
                let e = by.entry(model).or_default();
                e.0 += 1;
                e.1.add(&t);
            }
            let mut rows: Vec<(String, (i64, UsageTotals))> = by.into_iter().collect();
            rows.sort_by(|a, b| {
                b.1 .1
                    .cost_micros
                    .cmp(&a.1 .1.cost_micros)
                    .then(a.0.cmp(&b.0))
            });
            Ok(Value::Array(
                rows.into_iter()
                    .map(|(model, (sessions, t))| {
                        json!({
                            "model": model,
                            "sessions": sessions,
                            "input_tokens": t.input_tokens,
                            "output_tokens": t.output_tokens,
                            "cost_micros": t.cost_micros,
                        })
                    })
                    .collect(),
            ))
        }
        "work.retention" => {
            let st = crate::service::work::retention::status_locked(s, now)?;
            Ok(Value::Array(
                st.tables
                    .iter()
                    .map(|t| {
                        json!({
                            "kept": retention_label(&t.table),
                            "days": t.days,
                            "rows": t.rows,
                            "would_delete": t.would_delete,
                        })
                    })
                    .collect(),
            ))
        }
        "work.usage" => {
            let days = p
                .get("days")
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(crate::service::work::usage::DEFAULT_DAYS));
            let u = crate::service::work::usage::usage(
                s,
                days as u32,
                now,
                &crate::service::trackers::sync::metrics_for,
            )?;
            Ok(Value::Array(
                u.rows()
                    .into_iter()
                    .map(|(group, counted)| json!({ "group": group, "counted": counted }))
                    .collect(),
            ))
        }
        "work.retention_last" => {
            let st = crate::service::work::retention::status_locked(s, now)?;
            let at = st.last_sweep.as_ref().map(|l| l.at);
            let l = st.last_sweep.unwrap_or_default();
            Ok(json!({
                "at": at,
                "journal": l.journal,
                "tracker_items": l.tracker_items,
                "timeline_work_events": l.timeline_work_events,
                "tracker_writes": l.tracker_writes,
                "describe_cache": l.describe_cache,
            }))
        }
        other => Err(IpcError::new(
            codes::E_INTERNAL,
            format!("data source {other} is declared but has no reader"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table the retention sweep keeps has a name a person reads,
    /// never the raw SQL table.
    #[test]
    fn every_swept_table_has_a_label() {
        for t in crate::store::RetentionTable::ALL {
            assert_ne!(retention_label(t.table()), t.table(), "{}", t.table());
        }
    }

    /// Every value `fetch` returns has exactly the columns its shape
    /// declares, so a renderer can trust the declaration.
    fn assert_matches_shape(spec: &SourceSpec, v: &Value) {
        let keys_of = |o: &Value| -> Vec<String> {
            let mut k: Vec<String> = o.as_object().expect("object").keys().cloned().collect();
            k.sort();
            k
        };
        let ids = |cols: &[Column]| -> Vec<String> {
            let mut k: Vec<String> = cols.iter().map(|c| c.id.to_string()).collect();
            k.sort();
            k
        };
        match spec.shape {
            Shape::Scalar { .. } => assert!(v.is_number(), "{}: a scalar is a number", spec.id),
            Shape::Record { fields } => assert_eq!(keys_of(v), ids(fields), "{}", spec.id),
            Shape::Rows { columns } => {
                for row in v.as_array().expect("rows are an array") {
                    assert_eq!(keys_of(row), ids(columns), "{}", spec.id);
                }
            }
            Shape::Series { x, y } => {
                let mut cols = vec![x];
                cols.extend_from_slice(y);
                for point in v.as_array().expect("a series is an array") {
                    assert_eq!(keys_of(point), ids(&cols), "{}", spec.id);
                }
            }
        }
    }

    /// Two hosts, three sessions with usage, on two days; `now` is the
    /// second day.
    fn seeded() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        let now = 20_707 * usage::SECS_PER_DAY + 3_600;
        for (name, host, cost, at, model) in [
            ("a", "alpha", 100, now - 10, "claude-opus-5"),
            (
                "b",
                "beta",
                300,
                now - 2 * usage::SECS_PER_DAY,
                "claude-sonnet-5",
            ),
            ("c", "alpha", 200, now - 20, "claude-opus-5"),
        ] {
            s.upsert_host(host).unwrap();
            let id = s
                .upsert_session(name, host, None, None, 1, 1, "running", None)
                .unwrap();
            s.apply_usage(
                id,
                host,
                &crate::store::UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        input_tokens: 10,
                        cost_micros: cost,
                        ..Default::default()
                    },
                    model: Some(model.into()),
                    offset: 1,
                    source: "x.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now: at,
                    by_day: Vec::new(),
                    backfill_until: None,
                },
            )
            .unwrap();
        }
        (s, now)
    }

    #[test]
    fn every_source_has_a_reader_that_returns_its_shape() {
        let (s, now) = seeded();
        for spec in SOURCES {
            let v = fetch(&s, spec.id, &Map::new(), now)
                .unwrap_or_else(|e| panic!("{}: {}", spec.id, e.message));
            assert_matches_shape(spec, &v);
            if let Some(rows) = v.as_array() {
                assert!(!rows.is_empty(), "{}: the seed shows up", spec.id);
            }
        }
    }

    #[test]
    fn usage_by_model_groups_sessions_and_sorts_by_cost_then_name() {
        let (s, now) = seeded();
        let v = fetch(&s, "usage.by_model", &Map::new(), now).unwrap();
        assert_eq!(
            v,
            json!([
                {"model": "claude-opus-5", "sessions": 2, "input_tokens": 20, "output_tokens": 0, "cost_micros": 300},
                {"model": "claude-sonnet-5", "sessions": 1, "input_tokens": 10, "output_tokens": 0, "cost_micros": 300},
            ])
        );
        let mut alpha = Map::new();
        alpha.insert("host".into(), json!("alpha"));
        let v = fetch(&s, "usage.by_model", &alpha, now).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert!(fetch(&s, "no.such_source", &Map::new(), now).is_err());
    }

    #[test]
    fn ids_are_unique_and_help_is_a_sentence() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in SOURCES {
            assert!(seen.insert(spec.id), "duplicate source {}", spec.id);
            assert!(spec.help.ends_with('.'), "{}: help is a sentence", spec.id);
            for p in spec.params {
                assert!(
                    p.help.ends_with('.'),
                    "{}.{}: help is a sentence",
                    spec.id,
                    p.name
                );
            }
        }
    }

    #[test]
    fn params_are_checked_and_defaulted() {
        let spec = source("usage.by_day").unwrap();
        let ok = resolve_params(spec, &Map::new()).unwrap();
        assert_eq!(ok.get("days"), Some(&json!(14)));
        assert!(!ok.contains_key("host"));
        let mut m = Map::new();
        m.insert("days".into(), json!(0));
        assert!(resolve_params(spec, &m).is_err(), "below the minimum");
        let mut m = Map::new();
        m.insert("limit".into(), json!(5));
        assert!(resolve_params(spec, &m).is_err(), "unknown parameter");
        let mut m = Map::new();
        m.insert("host".into(), json!("bad host; rm"));
        assert!(resolve_params(spec, &m).is_err(), "not a host alias");
    }
}
