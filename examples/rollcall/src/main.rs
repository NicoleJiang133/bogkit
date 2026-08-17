use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::mpsc;
use std::time::Instant;

use anny::metric::Cosine;
use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Html,
    routing::{get, post},
};
use fold::pipeline::{FlatMap, Keyed, terminal};
use fold::stream::KeyedStream;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

const DIM: usize = ese::DIMENSIONS;
const PRS: [u64; 9] = [1, 3, 4, 5, 6, 7, 8, 9, 11];
const GH_PULLS: &str = "https://api.github.com/repos/flowercomputers/bogkit/pulls";
// Cosine distance (1 - sim). Prefer a miss over a wrong merge.
// Probe: Dan→Dan Brewster ~0.45; next-best wrong names ~0.90.
const SEMANTIC_MAX_DIST: f32 = 0.55;

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct Pr {
    number: u64,
    title: String,
    user: User,
    created_at: String,
}

#[derive(Deserialize)]
struct CommitUser {
    login: String,
}

#[derive(Deserialize)]
struct GitAuthor {
    name: String,
    email: String,
}

#[derive(Deserialize)]
struct CommitInner {
    author: GitAuthor,
}

#[derive(Deserialize)]
struct RawCommit {
    author: Option<CommitUser>,
    commit: CommitInner,
}

#[derive(Deserialize)]
struct ChatName {
    display: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct CommitFact {
    login: Option<String>,
    name: String,
    email: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Trace {
    number: u64,
    title: String,
    login: String,
    created_at: String,
    commits: Vec<CommitFact>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Edge {
    a: String,
    b: String,
    weight: u8,
}

#[derive(Clone, Serialize)]
struct ClusterView {
    id: String,
    primary: String,
    identifiers: Vec<String>,
    prs: Vec<u64>,
}

#[derive(Clone, Serialize)]
struct TraceView {
    key: String,
    number: u64,
    title: String,
    author: String,
    created_at: String,
}

#[derive(Clone, Serialize)]
struct RollcallState {
    people: usize,
    sources: usize,
    comparisons: String,
    avg_us: u64,
    avoided: usize,
    clusters: Vec<ClusterView>,
    unresolved: Vec<String>,
    traces: Vec<TraceView>,
    removed: Vec<TraceView>,
}

impl Default for RollcallState {
    fn default() -> Self {
        Self {
            people: 0,
            sources: 0,
            comparisons: "0 vs 0".to_string(),
            avg_us: 0,
            avoided: 0,
            clusters: Vec::new(),
            unresolved: Vec::new(),
            traces: Vec::new(),
            removed: Vec::new(),
        }
    }
}

enum Cmd {
    Remove(String),
    Undo(String),
    RestoreAll,
    Refresh,
}

type AppState = (mpsc::Sender<Cmd>, watch::Receiver<RollcallState>);

macro_rules! snapshot {
    ($st:expr) => {{
        $st.rtx(|(traces, vecs, edges)| {
            let tlist: Vec<(String, Trace)> = traces.iter().collect();
            let mut ids = Vec::new();
            for (_, t) in &tlist {
                ids.extend(identifiers(t));
            }
            let live: Vec<Edge> = edges
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(e, _)| e)
                .collect();
            let groups = connected(ids, live);
            let mut unresolved: Vec<String> = groups
                .iter()
                .filter(|g| !g.is_empty() && g.iter().all(|id| id.starts_with("chat:")))
                .flat_map(|g| {
                    g.iter()
                        .filter_map(|id| id.strip_prefix("chat:"))
                        .map(|s| s.to_string())
                })
                .collect();
            unresolved.sort();
            let clusters: Vec<ClusterView> = groups
                .iter()
                .filter(|g| {
                    g.iter()
                        .any(|id| id.starts_with("gh:") || id.starts_with("email:"))
                })
                .map(|g| {
                    let set: HashSet<&str> = g.iter().map(|s| s.as_str()).collect();
                    let mut prs: Vec<u64> = tlist
                        .iter()
                        .filter(|(_, t)| t.login != "chat")
                        .filter(|(_, t)| identifiers(t).iter().any(|i| set.contains(i.as_str())))
                        .map(|(_, t)| t.number)
                        .collect();
                    prs.sort();
                    prs.dedup();
                    ClusterView {
                        id: cluster_id(g),
                        primary: primary_name(g),
                        identifiers: g.iter().map(|id| display_ident(id)).collect(),
                        prs,
                    }
                })
                .collect();
            let mut traces: Vec<TraceView> = tlist.iter().map(|(k, t)| trace_view(k, t)).collect();
            traces.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            let mut chrono = tlist;
            chrono.sort_by(|a, b| a.1.created_at.cmp(&b.1.created_at));
            let mut universe: BTreeSet<String> = BTreeSet::new();
            let mut naive = 0u64;
            let mut actual = 0u64;
            let mut ns = 0u128;
            for (_, t) in &chrono {
                let src_ids = identifiers(t);
                universe.extend(src_ids);
                let n = universe.len() as u64;
                naive += n.saturating_mul(n.saturating_sub(1)) / 2;
                let t0 = Instant::now();
                let emb = ese::encode_single(&t.title);
                let hits = vecs.search(&emb);
                ns += t0.elapsed().as_nanos();
                actual += hits.len() as u64;
            }
            let nsrc = chrono.len() as u128;
            let avg_us = if nsrc == 0 { 0 } else { (ns / nsrc / 1000) as u64 };

            RollcallState {
                people: clusters.len(),
                sources: traces.len(),
                comparisons: format!("{} vs {}", commas(actual), commas(naive)),
                avg_us,
                avoided: chrono.len(),
                clusters,
                unresolved,
                traces,
                removed: Vec::new(),
            }
        })
    }};
}

fn main() {
    let db_path = std::env::temp_dir().join("bog-kit-rollcall.db");
    let _ = std::fs::remove_dir_all(&db_path);

    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (state_tx, state_rx) = watch::channel(RollcallState::default());
    std::thread::spawn(move || ingest(&db_path, cmd_rx, state_tx));
    serve(cmd_tx, state_rx);
}

fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

fn trace_view(key: &str, t: &Trace) -> TraceView {
    TraceView {
        key: key.to_string(),
        number: t.number,
        title: t.title.clone(),
        author: t.login.clone(),
        created_at: t.created_at.clone(),
    }
}

fn removed_views(stash: &HashMap<String, Trace>) -> Vec<TraceView> {
    let mut v: Vec<TraceView> = stash.iter().map(|(k, t)| trace_view(k, t)).collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    v
}

fn ingest(
    db_path: &std::path::Path,
    rx: mpsc::Receiver<Cmd>,
    state_tx: watch::Sender<RollcallState>,
) {
    let mut st = KeyedStream::new(
        db_path,
        (
            terminal::Table::new("traces"),
            FlatMap::new(
                |d: &Keyed<String, Trace>| hnsw_vecs(d),
                terminal::search::Hnsw::<String, f32, Cosine, DIM>::new("vecs", Cosine, 42),
            ),
            FlatMap::new(
                |d: &Keyed<String, Trace>| edges_from(&d.val),
                terminal::Bag::<Edge>::new("edges"),
            ),
        ),
    );

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
    st.wtx(|tx| {
        for n in PRS {
            let trace = load_trace(dir, n);
            tx.upsert(&format!("pr:{n}"), &trace);
        }
    });
    let chats: Vec<ChatName> = serde_json::from_str(
        &std::fs::read_to_string(format!("{dir}/chat-names.json")).unwrap(),
    )
    .unwrap();
    for (i, chat) in chats.iter().enumerate() {
        let q = ese::encode_single(&chat.display);
        let hits = st.rtx(|(_, vecs, _)| vecs.search(&q));
        let mut link = None;
        for hit in &hits {
            if hit.score >= SEMANTIC_MAX_DIST {
                break;
            }
            if let Some((_, id)) = hit.val.split_once('|')
                && id.starts_with("name:")
            {
                println!(
                    "semantic {} -> {} dist={:.4}",
                    chat.display, id, hit.score
                );
                link = Some(id.to_string());
                break;
            }
        }
        if link.is_none() {
            let nearest = hits
                .iter()
                .find_map(|h| {
                    h.val.split_once('|').and_then(|(_, id)| {
                        id.starts_with("name:").then_some((id, h.score))
                    })
                });
            match nearest {
                Some((id, d)) => println!(
                    "semantic {} -> (skip {} dist={:.4})",
                    chat.display, id, d
                ),
                None => println!("semantic {} -> (no name hit)", chat.display),
            }
        }
        let trace = Trace {
            number: 0,
            title: chat.display.clone(),
            login: "chat".into(),
            created_at: format!("2026-08-16T23:{i:02}:00Z"),
            commits: vec![],
            link,
        };
        st.wtx(|tx| {
            tx.upsert(&format!("chat:{}", chat.display), &trace);
        });
    }
    let mut stash: HashMap<String, Trace> = HashMap::new();
    let mut snap = snapshot!(st);
    snap.removed = removed_views(&stash);
    let _ = state_tx.send(snap);

    for cmd in rx {
        match cmd {
            Cmd::Remove(key) => {
                let old = st.wtx(|tx| tx.remove(&key));
                if let Some(t) = old {
                    stash.insert(key, t);
                }
            }
            Cmd::Undo(key) => {
                if let Some(t) = stash.remove(&key) {
                    st.wtx(|tx| {
                        tx.upsert(&key, &t);
                    });
                }
            }
            Cmd::RestoreAll => {
                let items: Vec<(String, Trace)> = stash.drain().collect();
                if !items.is_empty() {
                    st.wtx(|tx| {
                        for (key, t) in &items {
                            tx.upsert(key, t);
                        }
                    });
                }
            }
            Cmd::Refresh => {
                let pulls = fetch_pulls();
                let mut fresh = Vec::new();
                for pr in pulls {
                    let key = format!("pr:{}", pr.number);
                    if st.contains(&key) || stash.contains_key(&key) {
                        continue;
                    }
                    fresh.push((key, fetch_trace(&pr)));
                }
                println!("refresh: {} new PR(s)", fresh.len());
                if !fresh.is_empty() {
                    st.wtx(|tx| {
                        for (key, trace) in &fresh {
                            tx.upsert(key, trace);
                        }
                    });
                }
            }
        }
        let mut snap = snapshot!(st);
        snap.removed = removed_views(&stash);
        let _ = state_tx.send(snap);
    }
}

fn fetch_pulls() -> Vec<Pr> {
    let mut all = Vec::new();
    let mut page = 1;
    loop {
        let url = format!("{GH_PULLS}?state=all&per_page=100&page={page}");
        let batch: Vec<Pr> = serde_json::from_str(&gh_get(&url)).unwrap();
        let n = batch.len();
        all.extend(batch);
        if n < 100 {
            break;
        }
        page += 1;
    }
    all
}

fn fetch_trace(pr: &Pr) -> Trace {
    let mut commits = Vec::new();
    let mut page = 1;
    loop {
        let url = format!("{GH_PULLS}/{}/commits?per_page=100&page={page}", pr.number);
        let batch: Vec<RawCommit> = serde_json::from_str(&gh_get(&url)).unwrap();
        let n = batch.len();
        commits.extend(batch);
        if n < 100 {
            break;
        }
        page += 1;
    }
    Trace {
        number: pr.number,
        title: pr.title.clone(),
        login: pr.user.login.clone(),
        created_at: pr.created_at.clone(),
        commits: commits
            .into_iter()
            .map(|c| CommitFact {
                login: c.author.map(|a| a.login),
                name: c.commit.author.name,
                email: c.commit.author.email,
            })
            .collect(),
        link: None,
    }
}

fn gh_get(url: &str) -> String {
    let mut req = minreq::get(url).with_header("User-Agent", "rollcall");
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        req = req.with_header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send().unwrap();
    assert_eq!(
        resp.status_code, 200,
        "GET {url} -> {} {}",
        resp.status_code,
        resp.as_str().unwrap_or("")
    );
    resp.as_str().unwrap().to_string()
}

#[tokio::main]
async fn serve(cmd_tx: mpsc::Sender<Cmd>, state_rx: watch::Receiver<RollcallState>) {
    let app = Router::new()
        .route("/", get(index))
        .route("/p/{cluster_id}", get(index))
        .route("/ws", get(ws_upgrade))
        .route("/api/state", get(api_state))
        .route("/rm/{*key}", post(rm))
        .route("/undo/{*key}", post(undo))
        .route("/restore", post(restore))
        .route("/refresh", post(refresh))
        .route("/recap", post(recap))
        .route("/host-recap", post(host_recap))
        .with_state((cmd_tx, state_rx));

    let addr = "0.0.0.0:3000";
    println!("rollcall running on http://localhost:3000");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn api_state(State((_, rx)): State<AppState>) -> Json<RollcallState> {
    Json(rx.borrow().clone())
}

async fn rm(State((cmd_tx, _)): State<AppState>, Path(key): Path<String>) {
    cmd_tx.send(Cmd::Remove(src_key(&key))).unwrap();
}

async fn undo(State((cmd_tx, _)): State<AppState>, Path(key): Path<String>) {
    cmd_tx.send(Cmd::Undo(src_key(&key))).unwrap();
}

async fn restore(State((cmd_tx, _)): State<AppState>) {
    cmd_tx.send(Cmd::RestoreAll).unwrap();
}

async fn refresh(State((cmd_tx, _)): State<AppState>) {
    cmd_tx.send(Cmd::Refresh).unwrap();
}

#[derive(Deserialize)]
struct RecapReq {
    id: String,
}

#[derive(Serialize)]
struct RecapOut {
    text: String,
    platform: String,
}

async fn recap(State((_, rx)): State<AppState>, Json(req): Json<RecapReq>) -> Json<RecapOut> {
    let snap = rx.borrow().clone();
    let Some(cluster) = snap.clusters.iter().find(|c| c.id == req.id).cloned() else {
        return Json(RecapOut {
            text: "Cluster not found.".into(),
            platform: "linkedin".into(),
        });
    };
    let titles: Vec<String> = cluster
        .prs
        .iter()
        .filter_map(|n| {
            snap.traces
                .iter()
                .find(|t| t.number == *n)
                .map(|t| format!("#{} {}", t.number, t.title))
        })
        .collect();
    let clusters = snap.clusters.clone();
    let text = tokio::task::spawn_blocking(move || recap_text(&cluster, &titles, &clusters))
        .await
        .unwrap_or_else(|_| "Recap unavailable.".into());
    Json(RecapOut {
        text,
        platform: "linkedin".into(),
    })
}

fn recap_text(cluster: &ClusterView, titles: &[String], clusters: &[ClusterView]) -> String {
    match anthropic_recap(cluster, titles, clusters) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => template_recap(cluster, titles, clusters),
    }
}

#[derive(Deserialize)]
struct ChatMsg {
    display: String,
    text: String,
}

async fn host_recap(State((_, rx)): State<AppState>) -> Json<RecapOut> {
    let snap = rx.borrow().clone();
    let text = tokio::task::spawn_blocking(move || host_recap_text(&snap))
        .await
        .unwrap_or_else(|_| "Recap unavailable.".into());
    Json(RecapOut {
        text,
        platform: "linkedin".into(),
    })
}

fn host_recap_text(snap: &RollcallState) -> String {
    match anthropic_host_recap(snap) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => template_host_recap(snap),
    }
}

fn chat_transcript() -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
    let raw = std::fs::read_to_string(format!("{dir}/chat-messages.json")).unwrap();
    let msgs: Vec<ChatMsg> = serde_json::from_str(&raw).unwrap();
    msgs.iter()
        .map(|m| format!("{}: {}", m.display, m.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn all_ats(snap: &RollcallState) -> String {
    let mut handles: Vec<String> = snap
        .clusters
        .iter()
        .flat_map(|c| c.identifiers.iter())
        .filter_map(|i| i.strip_prefix("gh:"))
        .map(|h| format!("@{h}"))
        .collect();
    handles.sort();
    handles.dedup();
    handles.join(" ")
}

fn host_facts(snap: &RollcallState) -> (String, String, String) {
    let clusters = snap
        .clusters
        .iter()
        .map(|c| {
            format!(
                "- {} | {} | PRs {}",
                c.primary,
                c.identifiers.join(", "),
                if c.prs.is_empty() {
                    "none".into()
                } else {
                    c.prs
                        .iter()
                        .map(|n| format!("#{n}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let sources = snap
        .traces
        .iter()
        .map(|t| {
            if t.key.starts_with("chat:") {
                format!("- chat: {}", t.title)
            } else {
                format!("- #{} {}", t.number, t.title)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (clusters, sources, chat_transcript())
}

fn template_host_recap(snap: &RollcallState) -> String {
    let projects: Vec<String> = snap
        .traces
        .iter()
        .filter(|t| !t.key.starts_with("chat:"))
        .map(|t| format!("#{} {}", t.number, t.title))
        .collect();
    let work = if projects.is_empty() {
        "the projects that made it onto the board".to_string()
    } else {
        projects.join("; ")
    };
    format!(
        "Bog-a-thon III did not look like a conference. It looked like a chat that briefly became a potato-chip database, then a pile of PRs.\n\n\
Flower Computer hosted it. {n} people showed up and built with bogkit — fold, ese, anny. The work on the table: {work}.\n\n\
The room was small enough that a throwaway line about a potato chip as a database still counts as documentation. That is the recap.\n\n\
{ats}",
        n = snap.people,
        work = work,
        ats = all_ats(snap),
    )
}

fn anthropic_host_recap(snap: &RollcallState) -> Result<String, ()> {
    let (clusters, sources, chat) = host_facts(snap);
    let prompt = format!(
        "Write a LinkedIn post from the host's point of view about Bog-a-thon III, a hackathon run by Flower Computer. Third person. Ready to paste and publish.\n\
People built with bogkit (fold, ese, anny).\n\
Attendees (resolved people, count={n}):\n{clusters}\n\n\
Sources / project titles:\n{sources}\n\n\
Live chat from the room (use for atmosphere; quote if it helps, including the potato chip line):\n{chat}\n\n\
Rules:\n\
- Third person (the hosts describing the event), not a participant diary\n\
- Mention how many people showed up and name the actual projects from the titles\n\
- 100-180 words\n\
- 3-5 short paragraphs with a blank line between paragraphs\n\
- Open with one concrete hook. Never start with \"I'm excited to share\", \"Thrilled to announce\", \"We're proud to\", or similar filler\n\
- Close with one specific observation, not empty thanks\n\
- Sound like a person wrote it, not an AI\n\
- No emoji\n\
- At most two hashtags, none required\n\
- Last line is ONLY these handles: {ats}\n\
- Output the post only, no preamble or quotes",
        n = snap.people,
        clusters = clusters,
        sources = sources,
        chat = chat,
        ats = all_ats(snap),
    );
    anthropic_complete(&prompt, 700)
}

fn peer_ats(me: &ClusterView, clusters: &[ClusterView]) -> String {
    let mut mine = HashSet::new();
    mine.insert(me.primary.to_lowercase());
    for id in &me.identifiers {
        if let Some(h) = id.strip_prefix("gh:") {
            mine.insert(h.to_lowercase());
        }
    }
    let mut handles: Vec<String> = clusters
        .iter()
        .filter(|c| c.id != me.id)
        .map(|c| c.primary.as_str())
        .filter(|p| !mine.contains(&p.to_lowercase()))
        .map(|p| format!("@{p}"))
        .collect();
    handles.sort();
    handles.dedup();
    handles.push("@flowercomputer".into());
    handles.join(" ")
}

fn template_recap(cluster: &ClusterView, titles: &[String], clusters: &[ClusterView]) -> String {
    let work = if titles.is_empty() {
        "the traces that were still standing when I looked".to_string()
    } else {
        titles.join("; ")
    };
    format!(
        "I kept catching the same human under different names and got tired of pretending that was a filing problem.\n\n\
At Bog-a-thon III, the hackathon Flower Computer put on, I shipped this: {work}. The stack was bogkit — fold holding the identity graph live, ese embedding the messy labels, anny finding neighbors instead of grinding every pair.\n\n\
What I'll take home is watching a cluster come apart when a source disappeared. Merge is the demo. Being able to take it back is the point.\n\n\
{ats}",
        work = work,
        ats = peer_ats(cluster, clusters),
    )
}

#[derive(Serialize)]
struct AnthropicReq {
    model: &'static str,
    max_tokens: u32,
    messages: Vec<AnthropicUser>,
}

#[derive(Serialize)]
struct AnthropicUser {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResp {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    text: Option<String>,
}

fn anthropic_recap(cluster: &ClusterView, titles: &[String], clusters: &[ClusterView]) -> Result<String, ()> {
    let prompt = format!(
        "Write a LinkedIn post as this person, first person, ready to paste and publish.\n\
Context: Bog-a-thon III, a hackathon run by Flower Computer. People built with bogkit (fold, ese, anny).\n\
Name: {name}\nIdentifiers: {ids}\n\
Pull requests they actually shipped (use these titles, not vague summaries):\n{prs}\n\n\
Rules:\n\
- 100-150 words\n\
- 3-5 short paragraphs with a blank line between paragraphs\n\
- Open with one concrete hook. Never start with \"I'm excited to share\", \"Thrilled to announce\", \"I'm proud to\", or similar filler\n\
- Middle: what they actually built, using the PR titles\n\
- Close with one specific feeling or lesson — not empty thanks\n\
- Sound like a person wrote it, not an AI\n\
- No emoji\n\
- At most two hashtags, none required\n\
- Last line is ONLY these handles: {ats}\n\
- Output the post only, no preamble or quotes",
        name = cluster.primary,
        ids = cluster.identifiers.join(", "),
        prs = if titles.is_empty() {
            "(none in the current snapshot)".to_string()
        } else {
            titles.join("\n")
        },
        ats = peer_ats(cluster, clusters),
    );
    anthropic_complete(&prompt, 500)
}

fn anthropic_complete(prompt: &str, max_tokens: u32) -> Result<String, ()> {
    let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| ())?;
    if key.is_empty() {
        return Err(());
    }
    let body = serde_json::to_string(&AnthropicReq {
        model: "claude-haiku-4-5-20251001",
        max_tokens,
        messages: vec![AnthropicUser {
            role: "user",
            content: prompt.to_string(),
        }],
    })
    .map_err(|_| ())?;
    let resp = minreq::post("https://api.anthropic.com/v1/messages")
        .with_header("x-api-key", key)
        .with_header("anthropic-version", "2023-06-01")
        .with_header("content-type", "application/json")
        .with_timeout(20)
        .with_body(body)
        .send()
        .map_err(|_| ())?;
    if resp.status_code != 200 {
        return Err(());
    }
    let parsed: AnthropicResp =
        serde_json::from_str(resp.as_str().map_err(|_| ())?).map_err(|_| ())?;
    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .filter(|s| !s.trim().is_empty())
        .ok_or(())
}

async fn ws_upgrade(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, (cmd_tx, mut state_rx): AppState) {
    let state_json = |s: &RollcallState| serde_json::to_string(s).unwrap();
    let hello = state_json(&state_rx.borrow_and_update());
    if socket.send(Message::text(hello)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            changed = state_rx.changed() => {
                if changed.is_err() {
                    return;
                }
                let update = state_json(&state_rx.borrow_and_update());
                if socket.send(Message::text(update)).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(Message::Text(line))) = incoming else {
                    return;
                };
                let line = line.trim();
                let cmd = if line == "refresh" {
                    Cmd::Refresh
                } else if line == "restore" {
                    Cmd::RestoreAll
                } else if let Some(n) = line.strip_prefix("rm:") {
                    Cmd::Remove(src_key(n.trim()))
                } else if let Some(n) = line.strip_prefix("undo:") {
                    Cmd::Undo(src_key(n.trim()))
                } else {
                    continue;
                };
                if cmd_tx.send(cmd).is_err() {
                    return;
                }
            }
        }
    }
}

fn load_trace(dir: &str, n: u64) -> Trace {
    let pr: Pr = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/pr-{n}.json")).unwrap())
        .unwrap();
    let commits: Vec<RawCommit> = serde_json::from_str(
        &std::fs::read_to_string(format!("{dir}/pr-{n}-commits.json")).unwrap(),
    )
    .unwrap();
    Trace {
        number: pr.number,
        title: pr.title,
        login: pr.user.login,
        created_at: pr.created_at,
        commits: commits
            .into_iter()
            .map(|c| CommitFact {
                login: c.author.map(|a| a.login),
                name: c.commit.author.name,
                email: c.commit.author.email,
            })
            .collect(),
        link: None,
    }
}

fn gh(handle: &str) -> String {
    format!("gh:{handle}")
}
fn email_id(email: &str) -> String {
    format!("email:{email}")
}
fn name_id(name: &str) -> String {
    format!("name:{name}")
}

fn cluster_id(ids: &[String]) -> String {
    ids.iter()
        .find(|i| i.starts_with("gh:"))
        .cloned()
        .unwrap_or_else(|| ids[0].clone())
}

fn primary_name(ids: &[String]) -> String {
    if let Some(g) = ids.iter().find(|i| i.starts_with("gh:")) {
        return g[3..].to_string();
    }
    if let Some(n) = ids.iter().find(|i| i.starts_with("name:")) {
        return n[5..].to_string();
    }
    ids[0].clone()
}

fn noreply_handle(email: &str) -> Option<&str> {
    let rest = email.strip_suffix("@users.noreply.github.com")?;
    let (id, handle) = rest.split_once('+')?;
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && !handle.is_empty() {
        Some(handle)
    } else {
        None
    }
}

fn display_ident(id: &str) -> String {
    if let Some(d) = id.strip_prefix("chat:") {
        format!("~{d}")
    } else {
        id.to_string()
    }
}

fn hnsw_vecs(d: &Keyed<String, Trace>) -> Vec<Keyed<String, [f32; DIM]>> {
    let mut v = vec![Keyed::new(d.key.clone(), ese::encode_single(&d.val.title))];
    if d.val.login != "chat" {
        for id in identifiers(&d.val) {
            if let Some(text) = id.strip_prefix("name:") {
                v.push(Keyed::new(
                    format!("{}|{id}", d.key),
                    ese::encode_single(text),
                ));
            }
        }
    }
    v
}

fn src_key(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        format!("pr:{s}")
    } else {
        s.to_string()
    }
}

fn identifiers(t: &Trace) -> Vec<String> {
    if t.login == "chat" {
        return vec![format!("chat:{}", t.title)];
    }
    let mut ids = BTreeSet::new();
    ids.insert(gh(&t.login));
    for c in &t.commits {
        if let Some(login) = &c.login {
            ids.insert(gh(login));
        }
        if !c.name.is_empty() {
            ids.insert(name_id(&c.name));
        }
        if !c.email.is_empty() {
            ids.insert(email_id(&c.email));
        }
    }
    ids.into_iter().collect()
}

fn edges_from(t: &Trace) -> Vec<Edge> {
    if t.login == "chat" {
        let mut out = Vec::new();
        if let Some(link) = &t.link {
            let a = format!("chat:{}", t.title);
            if a != *link {
                let (a, b) = if a <= *link {
                    (a, link.clone())
                } else {
                    (link.clone(), a)
                };
                out.push(Edge { a, b, weight: 1 });
            }
        }
        return out;
    }
    let mut out = Vec::new();
    let pr_gh = gh(&t.login);
    for c in &t.commits {
        let mut seen = HashSet::new();
        let mut add = |a: String, b: String, weight: u8| {
            if a == b {
                return;
            }
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            if seen.insert((a.clone(), b.clone())) {
                out.push(Edge { a, b, weight });
            }
        };

        let c_gh = c.login.as_deref().map(gh);
        let c_name = (!c.name.is_empty()).then(|| name_id(&c.name));
        let c_email = (!c.email.is_empty()).then(|| email_id(&c.email));

        if let Some(e) = &c_email {
            if let Some(n) = &c_name {
                add(e.clone(), n.clone(), 3);
            }
            if let Some(h) = &c_gh {
                add(e.clone(), h.clone(), 3);
            }
        }
        if let Some(handle) = noreply_handle(&c.email) {
            add(email_id(&c.email), gh(handle), 3);
        }
        if let Some(n) = &c_name {
            add(n.clone(), pr_gh.clone(), 3);
        }
        if let Some(e) = &c_email {
            add(e.clone(), pr_gh.clone(), 3);
        }
        if let Some(h) = &c_gh {
            add(h.clone(), pr_gh.clone(), 3);
        }
        if let Some(local) = c.email.split_once('@').map(|(l, _)| l) {
            if local == t.login {
                add(email_id(&c.email), pr_gh.clone(), 2);
            }
            if let Some(login) = &c.login
                && local == login
            {
                add(email_id(&c.email), gh(login), 2);
            }
        }
    }
    out
}

fn connected(ids: Vec<String>, edges: Vec<Edge>) -> Vec<Vec<String>> {
    let mut parent: HashMap<String, String> = HashMap::new();
    for id in &ids {
        parent.entry(id.clone()).or_insert_with(|| id.clone());
    }
    fn find(parent: &mut HashMap<String, String>, x: &str) -> String {
        let p = parent
            .entry(x.to_string())
            .or_insert_with(|| x.to_string())
            .clone();
        if p == x {
            return p;
        }
        let r = find(parent, &p);
        parent.insert(x.to_string(), r.clone());
        r
    }
    for e in edges {
        let ra = find(&mut parent, &e.a);
        let rb = find(&mut parent, &e.b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in ids {
        let r = find(&mut parent, &id);
        groups.entry(r).or_default().insert(id);
    }
    let mut out: Vec<Vec<String>> = groups.into_values().map(|s| s.into_iter().collect()).collect();
    out.sort_by(|a, b| a[0].cmp(&b[0]));
    out
}
