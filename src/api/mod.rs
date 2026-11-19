//! Local demo server: the simulation over REST and server-sent events, and
//! the UI, embedded in the binary.
//!
//! Every endpoint takes the scenario in its query string (`seed`, `hostile`,
//! `sybil`). Scenarios are computed once and cached; they're deterministic.
//!
//! - `GET /api/scenario`  what's in the world, and the final scoreboard
//! - `GET /api/stream`    the 14 days of PRs, replayed as SSE (`speed` PRs/s)
//! - `GET /api/graph`     trust from `root` as known just after PR `upto`
//! - `GET /api/verdict`   evidence card and summary for PR `i`

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;

use crate::graph::{BadCall, TrustEngine};
use crate::ledger::{Kind, Settings};
use crate::sim::bench::{Run, Score, System};
use crate::sim::world::{GHOSTTY, HOSTILE, Truth};
use crate::sim::{self, World};
use crate::triage::groq::{self, Explanation};
use crate::triage::{GroqConfig, Tier, explain};

#[derive(RustEmbed)]
#[folder = "ui/"]
struct Ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScenarioKey {
    seed: u64,
    hostile: bool,
    sybil: bool,
}

/// One query shape for every endpoint. (`serde(flatten)` would be tidier,
/// but serde_urlencoded can't parse numbers or booleans through it.)
#[derive(Debug, Deserialize)]
struct Params {
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default)]
    hostile: bool,
    #[serde(default)]
    sybil: bool,
    /// Stream: PRs per second.
    #[serde(default = "default_speed")]
    speed: u64,
    /// Stream: index to start from.
    #[serde(default)]
    from: usize,
    /// Graph: whose point of view.
    root: Option<String>,
    /// Graph: use only what was known right after this PR.
    upto: Option<usize>,
    /// Verdict: which PR.
    i: Option<usize>,
}

impl Params {
    fn key(&self) -> ScenarioKey {
        ScenarioKey {
            seed: self.seed,
            hostile: self.hostile,
            sybil: self.sybil,
        }
    }
}

fn default_seed() -> u64 {
    1
}

fn default_speed() -> u64 {
    40
}

struct Scenario {
    world: World,
    run: Run,
}

struct AppState {
    scenarios: Mutex<HashMap<ScenarioKey, Arc<Scenario>>>,
    summaries: Mutex<HashMap<(ScenarioKey, usize), Explanation>>,
    llm: Option<GroqConfig>,
}

type Shared = Arc<AppState>;

impl AppState {
    async fn scenario(&self, key: ScenarioKey) -> Arc<Scenario> {
        if let Some(s) = self.scenarios.lock().expect("lock").get(&key) {
            return s.clone();
        }
        let built = tokio::task::spawn_blocking(move || {
            let world = World::scenario(key.seed, key.hostile, key.sybil);
            let run = sim::run(&world);
            Arc::new(Scenario { world, run })
        })
        .await
        .expect("simulation task doesn't panic");
        self.scenarios
            .lock()
            .expect("lock")
            .entry(key)
            .or_insert(built)
            .clone()
    }
}

pub fn router(llm: Option<GroqConfig>) -> Router {
    let state = Arc::new(AppState {
        scenarios: Mutex::new(HashMap::new()),
        summaries: Mutex::new(HashMap::new()),
        llm,
    });
    Router::new()
        .route("/api/scenario", get(scenario))
        .route("/api/stream", get(stream))
        .route("/api/graph", get(graph))
        .route("/api/verdict", get(verdict))
        .fallback(static_file)
        .with_state(state)
}

pub async fn serve(addr: SocketAddr, llm: Option<GroqConfig>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(llm)).await
}

// --- /api/scenario ----------------------------------------------------------

#[derive(Serialize)]
struct ProjectInfo {
    id: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct ScoreRow {
    system: &'static str,
    name: &'static str,
    spam_caught: f64,
    offenders_caught: f64,
    newcomers_blocked: f64,
    regulars_blocked: f64,
    honest_people_blocked: usize,
    smeared_blocked: usize,
    /// None when nothing was blocked.
    precision: Option<f64>,
}

#[derive(Serialize)]
struct ScenarioInfo {
    seed: u64,
    hostile: bool,
    sybil: bool,
    start: DateTime<Utc>,
    days: i64,
    prs: usize,
    by_truth: BTreeMap<&'static str, usize>,
    smeared: usize,
    projects: Vec<ProjectInfo>,
    scores: Vec<ScoreRow>,
    llm: Option<String>,
}

fn kind_of(world: &World, id: &str) -> &'static str {
    if id == HOSTILE {
        "hostile"
    } else if world.sybils.iter().any(|s| s == id) {
        "sybil"
    } else {
        "honest"
    }
}

fn truth_name(t: Truth) -> &'static str {
    match t {
        Truth::Regular => "regular",
        Truth::Newcomer => "newcomer",
        Truth::Spam => "campaign_spam",
        Truth::RepeatOffender => "repeat_offender",
    }
}

fn system_id(s: System) -> &'static str {
    match s {
        System::FlatRequireVouch => "require_vouch",
        System::FlatDenounceOnly => "denounce_only",
        System::FlatSlurp => "peers_lists",
        System::Beacon => "beacon",
    }
}

fn score_row(system: System, s: &Score) -> ScoreRow {
    ScoreRow {
        system: system_id(system),
        name: system.name(),
        spam_caught: s.spam_caught(),
        offenders_caught: s.offenders_caught(),
        newcomers_blocked: s.newcomers_blocked(),
        regulars_blocked: s.regulars_blocked(),
        honest_people_blocked: s.honest_people_blocked.len(),
        smeared_blocked: s.smeared_people_blocked.len(),
        precision: (!s.blocked.is_empty()).then(|| s.precision()),
    }
}

async fn scenario(State(st): State<Shared>, Query(q): Query<Params>) -> Json<ScenarioInfo> {
    let key = q.key();
    let sc = st.scenario(key).await;
    let w = &sc.world;
    let mut by_truth = BTreeMap::new();
    for pr in &w.prs {
        *by_truth.entry(truth_name(pr.truth)).or_default() += 1;
    }
    Json(ScenarioInfo {
        seed: key.seed,
        hostile: key.hostile,
        sybil: key.sybil,
        start: w.start,
        days: w.config.days,
        prs: w.prs.len(),
        by_truth,
        smeared: w.smeared.len(),
        projects: w
            .ledgers
            .keys()
            .map(|id| ProjectInfo {
                id: id.clone(),
                kind: kind_of(w, id),
            })
            .collect(),
        scores: sc
            .run
            .scores
            .iter()
            .map(|(s, score)| score_row(*s, score))
            .collect(),
        llm: st.llm.as_ref().map(|c| c.model.clone()),
    })
}

// --- /api/stream ------------------------------------------------------------

#[derive(Serialize)]
struct PrEvent<'a> {
    i: usize,
    at: DateTime<Utc>,
    author: &'a str,
    target: &'a str,
    title: &'a str,
    truth: &'static str,
    tier: Tier,
    /// Let through, but with denouncements worth reading first.
    review: bool,
    /// Someone the hostile project and Sybil ring are smearing.
    smeared: bool,
    campaign: bool,
    single_source: bool,
    /// Blocked by each system, keyed like the scoreboard.
    blocked: BTreeMap<&'static str, bool>,
}

fn pr_event(sc: &Scenario, i: usize) -> PrEvent<'_> {
    let pr = &sc.world.prs[i];
    let ev = &sc.run.evidence[i];
    PrEvent {
        i,
        at: pr.event.at,
        author: &pr.event.author,
        target: &pr.target,
        title: &pr.event.title,
        truth: truth_name(pr.truth),
        tier: ev.tier,
        review: ev.tier != Tier::Quarantined && !ev.denouncements.sources.is_empty(),
        smeared: sc.world.smeared.contains(&pr.event.author),
        campaign: ev.campaign.is_some(),
        single_source: ev.counterfactual.single_source(),
        blocked: System::ALL
            .iter()
            .zip(sc.run.blocked[i])
            .map(|(s, b)| (system_id(*s), b))
            .collect(),
    }
}

async fn stream(
    State(st): State<Shared>,
    Query(q): Query<Params>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let sc = st.scenario(q.key()).await;
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let period = Duration::from_micros(1_000_000 / q.speed.clamp(1, 1000));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        for i in q.from..sc.world.prs.len() {
            tick.tick().await;
            let event = Event::default()
                .event("pr")
                .json_data(pr_event(&sc, i))
                .expect("PR events serialize");
            if tx.send(Ok(event)).await.is_err() {
                return; // the browser went away
            }
        }
        let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

// --- /api/graph -------------------------------------------------------------

#[derive(Serialize)]
struct Node {
    id: String,
    kind: &'static str,
    trust: f64,
    hops: Option<usize>,
    path: Vec<String>,
    blame: Option<BlameInfo>,
}

#[derive(Serialize)]
struct BlameInfo {
    multiplier: f64,
    vouched_for_flagged: usize,
    denounced_your_people: usize,
}

#[derive(Serialize)]
struct Edge {
    from: String,
    to: String,
    weight: f64,
}

#[derive(Serialize)]
struct GraphResponse {
    root: String,
    at: DateTime<Utc>,
    upto: usize,
    flagged_accounts: usize,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

async fn graph(
    State(st): State<Shared>,
    Query(q): Query<Params>,
) -> Result<Json<GraphResponse>, (StatusCode, String)> {
    let sc = st.scenario(q.key()).await;
    let w = &sc.world;
    let root = q.root.unwrap_or_else(|| GHOSTTY.to_string());
    if !w.ledgers.contains_key(&root) {
        return Err((StatusCode::NOT_FOUND, format!("no project called '{root}'")));
    }
    let upto = q.upto.unwrap_or(w.prs.len() - 1).min(w.prs.len() - 1);
    let at = w.prs[upto].event.at;

    let response = tokio::task::spawn_blocking(move || {
        let w = &sc.world;
        let net = w.network();
        let mut engine = TrustEngine::new(&net, &root, Settings::default(), at);
        let mut flagged = 0;
        for (who, when) in &sc.run.campaign_flags {
            if *when <= at {
                engine = engine.flag(who, *when);
                flagged += 1;
            }
        }
        let view = engine.run();
        let nodes = w
            .ledgers
            .keys()
            .map(|id| {
                let p = view.projects.get(id);
                Node {
                    id: id.clone(),
                    kind: kind_of(w, id),
                    trust: p.map_or(0.0, |p| p.trust),
                    hops: p.map(|p| p.hops),
                    path: p.map(|p| p.path.clone()).unwrap_or_default(),
                    blame: p.and_then(|p| p.blame.as_ref()).map(|b| BlameInfo {
                        multiplier: b.multiplier,
                        vouched_for_flagged: b
                            .calls
                            .iter()
                            .filter(|(c, _)| matches!(c, BadCall::VouchedForQuarantined { .. }))
                            .count(),
                        denounced_your_people: b
                            .calls
                            .iter()
                            .filter(|(c, _)| matches!(c, BadCall::DenouncedRootsVouchee { .. }))
                            .count(),
                    }),
                }
            })
            .collect();
        let edges = w
            .ledgers
            .iter()
            .flat_map(|(from, atts)| {
                atts.iter()
                    .filter(|a| a.kind == Kind::TrustPeer && a.weight.unwrap_or(0.0) > 0.0)
                    .map(move |a| Edge {
                        from: from.clone(),
                        to: a.subject.clone(),
                        weight: a.weight.unwrap_or(0.0),
                    })
            })
            .collect();
        GraphResponse {
            root,
            at,
            upto,
            flagged_accounts: flagged,
            nodes,
            edges,
        }
    })
    .await
    .expect("graph task doesn't panic");
    Ok(Json(response))
}

// --- /api/verdict -----------------------------------------------------------

#[derive(Serialize)]
struct VerdictResponse {
    i: usize,
    author: String,
    target: String,
    title: String,
    truth: &'static str,
    evidence: crate::triage::Evidence,
    card: String,
    summary: Explanation,
}

async fn verdict(
    State(st): State<Shared>,
    Query(q): Query<Params>,
) -> Result<Json<VerdictResponse>, (StatusCode, String)> {
    let key = q.key();
    let sc = st.scenario(key).await;
    let i = q.i.unwrap_or(0);
    let Some(pr) = sc.world.prs.get(i) else {
        return Err((StatusCode::NOT_FOUND, format!("there is no PR number {i}")));
    };
    let evidence = sc.run.evidence[i].clone();

    let cached = st.summaries.lock().expect("lock").get(&(key, i)).cloned();
    let summary = match cached {
        Some(s) => s,
        None => {
            let ev = evidence.clone();
            let llm = st.llm.clone();
            let s = tokio::task::spawn_blocking(move || groq::explain(&ev, llm.as_ref()))
                .await
                .expect("explainer doesn't panic");
            st.summaries
                .lock()
                .expect("lock")
                .insert((key, i), s.clone());
            s
        }
    };

    Ok(Json(VerdictResponse {
        i,
        author: pr.event.author.clone(),
        target: pr.target.clone(),
        title: pr.event.title.clone(),
        truth: truth_name(pr.truth),
        card: explain::card(&evidence),
        evidence,
        summary,
    }))
}

// --- static files -----------------------------------------------------------

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // For UI work: serve straight from disk so edits show up on reload.
    if let Ok(dir) = std::env::var("BEACON_UI_DIR") {
        if path.split('/').any(|seg| seg == ".." || seg.contains('\\')) {
            return (StatusCode::NOT_FOUND, "Not found").into_response();
        }
        return match tokio::fs::read(std::path::Path::new(&dir).join(path)).await {
            Ok(bytes) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                ([(header::CONTENT_TYPE, mime.to_string())], bytes).into_response()
            }
            Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
        };
    }

    match Ui::get(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_string())],
            file.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn scenario_graph_and_verdict_endpoints() {
        let app = router(None);

        let (status, info) =
            get_json(app.clone(), "/api/scenario?seed=1&hostile=true&sybil=true").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(info["scores"].as_array().unwrap().len(), 4);
        assert!(
            info["projects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["kind"] == "sybil")
        );

        let (status, g) = get_json(
            app.clone(),
            "/api/graph?seed=1&hostile=true&sybil=true&root=ghostty",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let hostile = g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == HOSTILE)
            .unwrap()
            .clone();
        assert!(
            hostile["trust"].as_f64().unwrap() < 0.1,
            "blame should have cut it: {hostile}"
        );

        let (status, v) = get_json(
            app.clone(),
            "/api/verdict?seed=1&hostile=true&sybil=true&i=0",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["summary"]["author"]["by"], "template");

        let (status, _) = get_json(app, "/api/graph?seed=1&root=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serves_the_ui() {
        let resp = router(None)
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }
}
