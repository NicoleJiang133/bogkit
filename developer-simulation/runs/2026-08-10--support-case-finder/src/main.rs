use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anny::metric::Cosine;
use fold::pipeline::{Keyed, Map, Scored, terminal};
use fold::stream::KeyedStream;
use serde::{Deserialize, Serialize};

const DIM: usize = ese::DIMENSIONS;
const RRF_K: f64 = 60.0;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Case {
    case_id: u64,
    language: String,
    product_area: String,
    subject: String,
    body: String,
    resolution: String,
    modified_at: String,
}

impl Case {
    fn search_text(&self) -> String {
        format!("{} {} {}", self.subject, self.body, self.resolution)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Hit {
    case_id: u64,
    score: f64,
    product_area: String,
    language: String,
    excerpt: String,
}

#[derive(Clone, Copy)]
struct Query {
    language: &'static str,
    text: &'static str,
    useful_case: u64,
}

fn case_to_text(row: &Keyed<u64, Case>) -> Keyed<u64, String> {
    Keyed::new(row.key, row.val.search_text())
}

fn text_to_embedding(row: &Keyed<u64, String>) -> Keyed<u64, [f32; DIM]> {
    Keyed::new(row.key, ese::encode_single(&row.val))
}

macro_rules! open_store {
    ($path:expr) => {
        KeyedStream::new(
            $path,
            (
                Map::new(
                    case_to_text as fn(&Keyed<u64, Case>) -> Keyed<u64, String>,
                    (
                        terminal::search::Bm25::new("case_bm25"),
                        Map::new(
                            text_to_embedding as fn(&Keyed<u64, String>) -> Keyed<u64, [f32; DIM]>,
                            terminal::search::Hnsw::<u64, f32, Cosine, DIM>::new(
                                "case_vectors",
                                Cosine,
                                42,
                            ),
                        ),
                    ),
                ),
                terminal::Table::new("approved_cases"),
            ),
        )
    };
}

macro_rules! search {
    ($store:expr, $query:expr, $kind:expr, $limit:expr) => {{
        let query = $query;
        let query_embedding = ese::encode_single(query);
        $store.rtx(|((bm25, vectors), cases)| {
            let lexical = bm25.search(query, 10);
            let semantic = vectors.search(&query_embedding);
            let ranked = match $kind {
                SearchKind::Lexical => rank_lexical(&lexical),
                SearchKind::Semantic => rank_semantic(&semantic),
                SearchKind::Hybrid => fuse(&lexical, &semantic),
            };
            ranked
                .into_iter()
                .take($limit)
                .filter_map(|(case_id, score)| {
                    cases.get(&case_id).map(|case| Hit {
                        case_id,
                        score,
                        product_area: case.product_area,
                        language: case.language,
                        excerpt: exact_excerpt(&case.body),
                    })
                })
                .collect::<Vec<_>>()
        })
    }};
}

#[derive(Clone, Copy, Debug)]
enum SearchKind {
    Lexical,
    Semantic,
    Hybrid,
}

fn rank_lexical(hits: &[Scored<f64, u64>]) -> Vec<(u64, f64)> {
    let mut out: Vec<_> = hits.iter().map(|hit| (hit.val, hit.score)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

fn rank_semantic(hits: &[Scored<f32, u64>]) -> Vec<(u64, f64)> {
    let mut out: Vec<_> = hits
        .iter()
        .map(|hit| (hit.val, -(hit.score as f64)))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

fn fuse(lexical: &[Scored<f64, u64>], semantic: &[Scored<f32, u64>]) -> Vec<(u64, f64)> {
    let mut scores = BTreeMap::<u64, f64>::new();
    for (rank, hit) in lexical.iter().enumerate() {
        *scores.entry(hit.val).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    for (rank, hit) in semantic.iter().enumerate() {
        *scores.entry(hit.val).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    let mut out: Vec<_> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

fn exact_excerpt(body: &str) -> String {
    body.chars().take(96).collect()
}

fn fixture_cases() -> Vec<Case> {
    vec![
        case(
            1001,
            "en",
            "calendar",
            "Calendar stops updating",
            "Events added after a password change do not appear on the phone.",
            "Reconnect the calendar integration to replace the expired OAuth token.",
        ),
        case(
            1002,
            "en",
            "billing",
            "Duplicate renewal debit",
            "The monthly subscription was charged two times on the same day.",
            "Void the duplicate payment and repair the retry idempotency key.",
        ),
        case(
            1003,
            "en",
            "identity",
            "Password reset loop",
            "The recovery email opens sign-in instead of allowing a new password.",
            "Clear the stale login challenge before issuing another recovery link.",
        ),
        case(
            1004,
            "en",
            "exports",
            "Old rows in CSV",
            "A newly downloaded report still includes records removed yesterday.",
            "Invalidate the cached export after each visibility change.",
        ),
        case(
            1005,
            "en",
            "notifications",
            "Alerts arrive late",
            "Push notifications are delayed until the mobile application is opened.",
            "Renew the device registration after upgrading the application.",
        ),
        case(
            1006,
            "en",
            "mobile",
            "Crash while attaching photo",
            "The Android application closes when a large image is selected.",
            "Resize the image before decoding it into memory.",
        ),
        case(
            2001,
            "es",
            "calendar",
            "El calendario no se actualiza",
            "Las citas nuevas no aparecen después de cambiar la contraseña.",
            "Vuelva a conectar el calendario para renovar el token caducado.",
        ),
        case(
            2002,
            "es",
            "billing",
            "Cargo de renovación duplicado",
            "La suscripción mensual se cobró dos veces el mismo día.",
            "Anule el segundo pago y repare la clave de idempotencia.",
        ),
        case(
            2003,
            "es",
            "identity",
            "Bucle al restablecer la contraseña",
            "El correo de recuperación abre el inicio de sesión y no permite elegir una contraseña.",
            "Borre el desafío de acceso anterior antes de enviar otro enlace.",
        ),
        case(
            2004,
            "es",
            "exports",
            "Filas antiguas en CSV",
            "El informe descargado todavía incluye registros eliminados ayer.",
            "Invalide la exportación guardada después de cada cambio de visibilidad.",
        ),
        case(
            2005,
            "es",
            "notifications",
            "Avisos retrasados",
            "Las notificaciones llegan solamente al abrir la aplicación móvil.",
            "Renueve el registro del dispositivo después de actualizar la aplicación.",
        ),
        case(
            2006,
            "es",
            "mobile",
            "Fallo al adjuntar una foto",
            "La aplicación Android se cierra al elegir una imagen grande.",
            "Reduzca la imagen antes de cargarla en memoria.",
        ),
    ]
}

fn case(
    case_id: u64,
    language: &str,
    product_area: &str,
    subject: &str,
    body: &str,
    resolution: &str,
) -> Case {
    Case {
        case_id,
        language: language.to_owned(),
        product_area: product_area.to_owned(),
        subject: subject.to_owned(),
        body: body.to_owned(),
        resolution: resolution.to_owned(),
        modified_at: "2026-08-01T00:00:00Z".to_owned(),
    }
}

fn fixture_queries() -> Vec<Query> {
    vec![
        Query {
            language: "en",
            text: "appointments missing after credentials rotated",
            useful_case: 1001,
        },
        Query {
            language: "en",
            text: "two payments for one plan",
            useful_case: 1002,
        },
        Query {
            language: "en",
            text: "forgotten passcode link returns to login",
            useful_case: 1003,
        },
        Query {
            language: "es",
            text: "agenda vacía después de cambiar la clave",
            useful_case: 2001,
        },
        Query {
            language: "es",
            text: "me facturaron dos veces el plan",
            useful_case: 2002,
        },
        Query {
            language: "es",
            text: "correo para recuperar acceso vuelve al inicio",
            useful_case: 2003,
        },
    ]
}

fn unique_temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/run-data")
        .join(format!(
            "bogkit-support-{label}-{}-{nonce}",
            std::process::id()
        ))
}

fn remove_if_exists(path: &Path) {
    if path.exists() {
        fs::remove_dir_all(path).expect("remove temporary store");
    }
}

fn directory_size(path: &Path) -> u64 {
    fs::read_dir(path)
        .expect("read store directory")
        .map(|entry| {
            let entry = entry.expect("read directory entry");
            let metadata = entry.metadata().expect("read entry metadata");
            if metadata.is_dir() {
                directory_size(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

fn recall_count(results: &[Vec<Hit>], queries: &[Query], limit: usize) -> usize {
    results
        .iter()
        .zip(queries)
        .filter(|(hits, query)| {
            hits.iter()
                .take(limit)
                .any(|hit| hit.case_id == query.useful_case)
        })
        .count()
}

fn recall_by_language(
    results: &[Vec<Hit>],
    queries: &[Query],
    language: &str,
    limit: usize,
) -> (usize, usize) {
    let relevant: Vec<_> = results
        .iter()
        .zip(queries)
        .filter(|(_, query)| query.language == language)
        .collect();
    let found = relevant
        .iter()
        .filter(|(hits, query)| {
            hits.iter()
                .take(limit)
                .any(|hit| hit.case_id == query.useful_case)
        })
        .count();
    (found, relevant.len())
}

fn ids(results: &[Vec<Hit>]) -> Vec<Vec<u64>> {
    results
        .iter()
        .map(|hits| hits.iter().map(|hit| hit.case_id).collect())
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--interrupt-worker") => {
            let path = PathBuf::from(args.next().expect("worker requires a store path"));
            let mut store = open_store!(&path);
            store.wtx(|tx| {
                let mut changed = fixture_cases()[0].clone();
                changed.body = "UNCOMMITTED SENTINEL".to_owned();
                tx.upsert(&changed.case_id, &changed);
                std::process::abort();
            });
            unreachable!("the interrupt worker must abort");
        }
        Some("--snapshot-worker") => {
            let path = PathBuf::from(args.next().expect("worker requires a store path"));
            let store = open_store!(&path);
            for query in fixture_queries() {
                let hits = search!(store, query.text, SearchKind::Hybrid, 5);
                let ids: Vec<_> = hits.into_iter().map(|hit| hit.case_id).collect();
                println!("{ids:?}");
            }
            return;
        }
        Some("--scale") => {
            let cases = args
                .next()
                .expect("scale requires a case count")
                .parse::<u64>()
                .expect("case count must be an integer");
            let updates = args
                .next()
                .expect("scale requires an update count")
                .parse::<u64>()
                .expect("update count must be an integer");
            run_scale(cases, updates);
            return;
        }
        Some(other) => panic!("unknown argument: {other}"),
        None => {}
    }
    run_demo();
}

fn run_demo() {
    let path = unique_temp_path("demo");
    remove_if_exists(&path);
    let build_started = Instant::now();
    let mut store = open_store!(&path);
    store.wtx(|tx| {
        for row in fixture_cases() {
            tx.upsert(&row.case_id, &row);
        }
    });
    store.checkpoint();
    let build_time = build_started.elapsed();
    let queries = fixture_queries();

    let lexical: Vec<Vec<Hit>> = queries
        .iter()
        .map(|query| search!(store, query.text, SearchKind::Lexical, 5))
        .collect();
    let semantic: Vec<Vec<Hit>> = queries
        .iter()
        .map(|query| search!(store, query.text, SearchKind::Semantic, 5))
        .collect();
    let hybrid: Vec<Vec<Hit>> = queries
        .iter()
        .map(|query| search!(store, query.text, SearchKind::Hybrid, 5))
        .collect();

    println!(
        "Fixture only: {} approved cases, {} disclosed queries",
        fixture_cases().len(),
        queries.len()
    );
    println!(
        "build: {:?}; store: {} bytes",
        build_time,
        directory_size(&path)
    );
    println!(
        "recall@5: lexical {}/{}, semantic {}/{}, hybrid {}/{}",
        recall_count(&lexical, &queries, 5),
        queries.len(),
        recall_count(&semantic, &queries, 5),
        queries.len(),
        recall_count(&hybrid, &queries, 5),
        queries.len()
    );
    println!(
        "recall@1: lexical {}/{}, semantic {}/{}, hybrid {}/{}",
        recall_count(&lexical, &queries, 1),
        queries.len(),
        recall_count(&semantic, &queries, 1),
        queries.len(),
        recall_count(&hybrid, &queries, 1),
        queries.len()
    );
    for language in ["en", "es"] {
        let (found, total) = recall_by_language(&hybrid, &queries, language, 5);
        println!("hybrid recall@5 [{language}]: {found}/{total}");
    }
    for (query, hits) in queries.iter().zip(&hybrid) {
        let ids: Vec<_> = hits.iter().map(|hit| hit.case_id).collect();
        println!("hybrid [{}] {:?} -> {:?}", query.language, query.text, ids);
    }

    store.checkpoint();
    drop(store);
    let executable = std::env::current_exe().expect("resolve demo executable");
    let snapshots: Vec<Vec<u8>> = (0..3)
        .map(|_| {
            let output = Command::new(&executable)
                .arg("--snapshot-worker")
                .arg(&path)
                .output()
                .expect("launch snapshot worker");
            assert!(output.status.success(), "snapshot worker failed");
            output.stdout
        })
        .collect();
    assert!(
        snapshots.windows(2).all(|pair| pair[0] == pair[1]),
        "ranked case IDs changed between process runs"
    );
    println!("determinism: byte-identical ordered IDs across three process runs");

    let mut store = open_store!(&path);

    let before_interrupt = search!(store, queries[0].text, SearchKind::Hybrid, 5);
    store.checkpoint();
    drop(store);
    let worker_status = Command::new(std::env::current_exe().expect("resolve demo executable"))
        .arg("--interrupt-worker")
        .arg(&path)
        .status()
        .expect("launch interrupt worker");
    assert!(
        !worker_status.success(),
        "interrupt worker unexpectedly committed"
    );
    let mut store = open_store!(&path);
    let after_interrupt = search!(store, queries[0].text, SearchKind::Hybrid, 5);
    assert_eq!(ids(&[before_interrupt]), ids(&[after_interrupt]));
    assert_ne!(
        store.get(&1001).expect("case remains").body,
        "UNCOMMITTED SENTINEL"
    );
    println!("interrupted refresh: previous committed index remained queryable");

    let update_start = Instant::now();
    store.wtx(|tx| {
        let mut edited = fixture_cases()[0].clone();
        edited.body =
            "Events and appointments disappear because the OAuth credential expired.".to_owned();
        edited.modified_at = "2026-08-10T00:00:00Z".to_owned();
        tx.upsert(&1001, &edited);
        tx.remove(&1002);
        let added = case(
            3001,
            "en",
            "calendar",
            "Calendar authorization expired",
            "Appointments disappear when the workspace credential is revoked.",
            "Reconnect the approved calendar integration.",
        );
        tx.upsert(&3001, &added);
    });
    store.checkpoint();
    let update_time = update_start.elapsed();
    assert!(!store.contains(&1002));
    assert!(store.contains(&3001));
    let deleted_query = search!(store, "two payments for one plan", SearchKind::Hybrid, 10);
    assert!(deleted_query.iter().all(|hit| hit.case_id != 1002));
    let edited_query = search!(store, "expired OAuth credential", SearchKind::Hybrid, 5);
    assert!(edited_query.iter().any(|hit| hit.case_id == 1001));
    println!(
        "successful mixed refresh: {:?}; edit visible, insert visible, deletion absent",
        update_time
    );

    let mut latencies = Vec::new();
    for _ in 0..100 {
        for query in &queries {
            let started = Instant::now();
            let _ = search!(store, query.text, SearchKind::Hybrid, 5);
            latencies.push(started.elapsed());
        }
    }
    latencies.sort();
    let p95 = latencies[(latencies.len() * 95).div_ceil(100) - 1];
    println!(
        "warm hybrid query p95: {:?} over {} fixture queries",
        p95,
        latencies.len()
    );

    for hits in [&lexical, &semantic, &hybrid] {
        for hit in hits.iter().flatten() {
            assert!(
                [
                    "calendar",
                    "billing",
                    "identity",
                    "exports",
                    "notifications",
                    "mobile"
                ]
                .contains(&hit.product_area.as_str())
            );
            assert!(["en", "es"].contains(&hit.language.as_str()));
            let source = fixture_cases()
                .into_iter()
                .find(|row| row.case_id == hit.case_id)
                .expect("fixture hit exists");
            assert!(source.body.contains(&hit.excerpt));
            let displayed = format!(
                "{} {} {} {}",
                hit.case_id, hit.product_area, hit.language, hit.excerpt
            )
            .to_lowercase();
            for prohibited in ["email_address", "account_id", "customer_name", "attachment"] {
                assert!(!displayed.contains(prohibited));
            }
        }
    }
    println!(
        "privacy/source check: displayed hits use approved metadata and exact source excerpts"
    );

    remove_if_exists(&path);
}

fn synthetic_case(case_id: u64) -> Case {
    const AREAS: [&str; 6] = [
        "calendar",
        "billing",
        "identity",
        "exports",
        "notifications",
        "mobile",
    ];
    const ISSUES: [&str; 8] = [
        "authorization expired after a credential rotation",
        "duplicate debit caused by a retried renewal",
        "recovery link returns to the sign in page",
        "report contains rows removed the previous day",
        "notification is delayed until the application opens",
        "large photograph causes the application to close",
        "workspace setting does not propagate to the device",
        "scheduled import remains in a pending state",
    ];
    let language = if case_id.is_multiple_of(2) {
        "es"
    } else {
        "en"
    };
    let area = AREAS[case_id as usize % AREAS.len()];
    let issue = ISSUES[case_id as usize % ISSUES.len()];
    case(
        case_id,
        language,
        area,
        &format!("Synthetic scale case {case_id}"),
        &format!("Approved fixture record {case_id}: {issue}."),
        &format!("Apply the documented {area} recovery procedure for fixture {case_id}."),
    )
}

fn run_scale(case_count: u64, update_count: u64) {
    assert!(case_count > update_count, "updates require existing rows");
    let path = unique_temp_path("scale");
    remove_if_exists(&path);
    let mut store = open_store!(&path);

    let build_started = Instant::now();
    store.wtx(|tx| {
        for case_id in 0..case_count {
            let row = synthetic_case(case_id);
            tx.upsert(&case_id, &row);
        }
    });
    store.checkpoint();
    let build_time = build_started.elapsed();
    let size = directory_size(&path);

    let update_started = Instant::now();
    store.wtx(|tx| {
        for i in 0..update_count {
            match i % 3 {
                0 => {
                    tx.remove(&i);
                }
                1 => {
                    let mut row = synthetic_case(i);
                    row.body.push_str(" Edited in the representative refresh.");
                    row.modified_at = "2026-08-10T00:00:00Z".to_owned();
                    tx.upsert(&i, &row);
                }
                _ => {
                    let id = case_count + i;
                    let row = synthetic_case(id);
                    tx.upsert(&id, &row);
                }
            }
        }
    });
    store.checkpoint();
    let update_time = update_started.elapsed();

    assert!(!store.contains(&0), "deleted scale row remained visible");
    assert!(
        store
            .get(&1)
            .expect("edited scale row exists")
            .body
            .contains("representative refresh")
    );
    assert!(
        store.contains(&(case_count + 2)),
        "inserted scale row is missing"
    );

    let mut latencies = Vec::new();
    for _ in 0..1_000 {
        let started = Instant::now();
        let _ = search!(
            store,
            "renewal charged twice after retry",
            SearchKind::Hybrid,
            5
        );
        latencies.push(started.elapsed());
    }
    latencies.sort();
    let p95 = latencies[(latencies.len() * 95).div_ceil(100) - 1];
    println!("synthetic scale only: {case_count} cases, {update_count} mixed updates");
    println!("build and checkpoint: {build_time:?}");
    println!("store after build: {size} bytes");
    println!("update and checkpoint: {update_time:?}");
    println!("warm hybrid query p95: {p95:?} over 1000 repeated queries");

    remove_if_exists(&path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reciprocal_rank_fusion_is_deterministic_on_equal_scores() {
        let lexical = vec![Scored::new(1.0, 9), Scored::new(1.0, 3)];
        let semantic = vec![Scored::new(0.1, 3), Scored::new(0.1, 9)];
        let tied = 1.0 / 61.0 + 1.0 / 62.0;
        assert_eq!(fuse(&lexical, &semantic), vec![(3, tied), (9, tied)]);
    }

    #[test]
    fn excerpt_is_always_a_source_substring() {
        let body = "áéíóú soporte exacto";
        assert!(body.contains(&exact_excerpt(body)));
    }
}
