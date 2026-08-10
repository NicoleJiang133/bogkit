use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fold::pipeline::{FilterMap, KeyBy, Keyed, terminal};
use fold::stream::Stream;
use serde::{Deserialize, Serialize};

const ITEM_NAMES: [(&str, &str, &str); 8] = [
    ("Cordless Drill", "blue drill", "CD"),
    ("Soldering Iron", "hot pencil", "SI"),
    ("Socket Set", "ratchet box", "SS"),
    ("Circular Saw", "round saw", "CS"),
    ("Multimeter", "meter", "MM"),
    ("Sewing Machine", "stitcher", "SM"),
    ("Pipe Wrench", "red wrench", "PW"),
    ("Heat Gun", "hot blower", "HG"),
];

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Item {
    id: String,
    asset_tag: String,
    serial: String,
    name: String,
    nickname: String,
    source_row: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Borrower {
    id: String,
    name: String,
    source_row: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum EventKind {
    Checkout { borrower_id: String, due_day: u32 },
    Return,
    RepairStart { reason: String },
    RepairFinish,
    Retire { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Event {
    id: String,
    sequence: u64,
    at_day: u32,
    item_id: String,
    kind: EventKind,
    source_row: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum ItemState {
    #[default]
    Available,
    CheckedOut {
        borrower_id: String,
        due_day: u32,
    },
    InRepair {
        reason: String,
    },
    Retired {
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Projection {
    state: ItemState,
    history: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Record {
    Item(Item),
    Borrower(Borrower),
    Event(Event),
}

type ItemBranch = FilterMap<
    fn(&Record) -> Option<Keyed<String, Item>>,
    terminal::Table<String, Item>,
    Record,
    Keyed<String, Item>,
>;
type BorrowerBranch = FilterMap<
    fn(&Record) -> Option<Keyed<String, Borrower>>,
    terminal::Table<String, Borrower>,
    Record,
    Keyed<String, Borrower>,
>;
type EventBranch = FilterMap<
    fn(&Record) -> Option<Keyed<String, Event>>,
    terminal::Table<String, Event>,
    Record,
    Keyed<String, Event>,
>;
type ProjectionAggregate = fold::pipeline::Aggregate<
    String,
    Event,
    Projection,
    fn(&mut Projection, &Event, isize),
    terminal::Table<String, Projection>,
>;
type ProjectionBranch = FilterMap<
    fn(&Record) -> Option<Event>,
    KeyBy<fn(&Event) -> String, ProjectionAggregate, String, Event>,
    Record,
    Event,
>;
type KioskPipeline = (ItemBranch, BorrowerBranch, EventBranch, ProjectionBranch);
type Kiosk = Stream<Record, KioskPipeline>;

#[derive(Debug)]
struct ImportSummary {
    accepted_events: usize,
    rejected_rows: Vec<(String, String)>,
    elapsed: Duration,
}

fn item_record(record: &Record) -> Option<Keyed<String, Item>> {
    match record {
        Record::Item(item) => Some(Keyed::new(item.id.clone(), item.clone())),
        _ => None,
    }
}

fn borrower_record(record: &Record) -> Option<Keyed<String, Borrower>> {
    match record {
        Record::Borrower(borrower) => Some(Keyed::new(borrower.id.clone(), borrower.clone())),
        _ => None,
    }
}

fn event_record(record: &Record) -> Option<Keyed<String, Event>> {
    match record {
        Record::Event(event) => Some(Keyed::new(event.id.clone(), event.clone())),
        _ => None,
    }
}

fn event_only(record: &Record) -> Option<Event> {
    match record {
        Record::Event(event) => Some(event.clone()),
        _ => None,
    }
}

fn event_item(event: &Event) -> String {
    event.item_id.clone()
}

fn projection_step(projection: &mut Projection, event: &Event, delta: isize) {
    if delta > 0 {
        for _ in 0..delta {
            projection.history.push(event.clone());
        }
    } else {
        for _ in 0..delta.unsigned_abs() {
            if let Some(index) = projection.history.iter().position(|e| e.id == event.id) {
                projection.history.remove(index);
            }
        }
    }
    projection
        .history
        .sort_by(|a, b| (a.sequence, &a.id).cmp(&(b.sequence, &b.id)));
    projection.state = replay(&projection.history).expect("accepted event history must replay");
}

fn open(path: &Path) -> Kiosk {
    Stream::new(
        path,
        (
            FilterMap::new(
                item_record as fn(&Record) -> Option<Keyed<String, Item>>,
                terminal::Table::new("items"),
            ),
            FilterMap::new(
                borrower_record as fn(&Record) -> Option<Keyed<String, Borrower>>,
                terminal::Table::new("borrowers"),
            ),
            FilterMap::new(
                event_record as fn(&Record) -> Option<Keyed<String, Event>>,
                terminal::Table::new("events_by_id"),
            ),
            FilterMap::new(
                event_only as fn(&Record) -> Option<Event>,
                KeyBy::new(
                    event_item as fn(&Event) -> String,
                    fold::pipeline::Aggregate::new(
                        "projection_by_item",
                        projection_step as fn(&mut Projection, &Event, isize),
                        terminal::Table::new("projections"),
                    ),
                ),
            ),
        ),
    )
}

fn transition(state: &ItemState, kind: &EventKind) -> Result<ItemState, String> {
    match (state, kind) {
        (
            ItemState::Available,
            EventKind::Checkout {
                borrower_id,
                due_day,
            },
        ) => Ok(ItemState::CheckedOut {
            borrower_id: borrower_id.clone(),
            due_day: *due_day,
        }),
        (ItemState::CheckedOut { .. }, EventKind::Return) => Ok(ItemState::Available),
        (ItemState::Available, EventKind::RepairStart { reason }) => Ok(ItemState::InRepair {
            reason: reason.clone(),
        }),
        (ItemState::InRepair { .. }, EventKind::RepairFinish) => Ok(ItemState::Available),
        (ItemState::Available, EventKind::Retire { reason }) => Ok(ItemState::Retired {
            reason: reason.clone(),
        }),
        (ItemState::Retired { .. }, _) => Err("item is retired".to_string()),
        (state, kind) => Err(format!("impossible transition: {state:?} -> {kind:?}")),
    }
}

fn replay(events: &[Event]) -> Result<ItemState, String> {
    let mut state = ItemState::Available;
    for event in events {
        state = transition(&state, &event.kind)
            .map_err(|error| format!("{} at {}: {error}", event.id, event.source_row))?;
    }
    Ok(state)
}

fn validate_event(kiosk: &Kiosk, event: &Event) -> Result<(), String> {
    kiosk.rtx(|(items, borrowers, events, projections)| {
        if events.contains(&event.id) {
            return Err(format!("duplicate event id {}", event.id));
        }
        let mut latest_sequence = None;
        for (_, accepted) in events.iter() {
            if accepted.sequence == event.sequence {
                return Err(format!("duplicate event sequence {}", event.sequence));
            }
            latest_sequence = Some(latest_sequence.map_or(accepted.sequence, |latest: u64| {
                latest.max(accepted.sequence)
            }));
        }
        if let Some(latest) = latest_sequence
            && event.sequence <= latest
        {
            return Err(format!(
                "out-of-order event sequence {}; latest accepted sequence is {latest}",
                event.sequence
            ));
        }
        if !items.contains(&event.item_id) {
            return Err(format!("unknown item {}", event.item_id));
        }
        if let EventKind::Checkout { borrower_id, .. } = &event.kind
            && !borrowers.contains(borrower_id)
        {
            return Err(format!("unknown borrower {borrower_id}"));
        }
        let state = projections
            .get(&event.item_id)
            .map_or(ItemState::Available, |projection| projection.state);
        transition(&state, &event.kind).map(|_| ())
    })
}

fn apply_event(kiosk: &mut Kiosk, event: Event) -> Result<(), String> {
    validate_event(kiosk, &event)?;
    kiosk.wtx(|tx| tx.insert(&Record::Event(event)));
    Ok(())
}

fn visible_fingerprint(kiosk: &Kiosk) -> String {
    kiosk.rtx(|(_, _, events, projections)| {
        let mut event_rows: Vec<_> = events.iter().collect();
        event_rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut projection_rows: Vec<_> = projections.iter().collect();
        projection_rows.sort_by(|a, b| a.0.cmp(&b.0));
        format!("events={event_rows:?}\nprojections={projection_rows:?}")
    })
}

fn state_at(kiosk: &Kiosk, item_id: &str, at_day: u32) -> Option<ItemState> {
    kiosk.rtx(|(_, _, _, projections)| {
        let projection = projections.get(&item_id.to_string())?;
        let events: Vec<_> = projection
            .history
            .into_iter()
            .filter(|event| event.at_day <= at_day)
            .collect();
        replay(&events).ok()
    })
}

fn insert_masters(kiosk: &mut Kiosk, items: &[Item], borrowers: &[Borrower]) {
    for chunk in items.chunks(1_000) {
        kiosk.wtx(|tx| {
            for item in chunk {
                tx.insert(&Record::Item(item.clone()));
            }
        });
    }
    for chunk in borrowers.chunks(1_000) {
        kiosk.wtx(|tx| {
            for borrower in chunk {
                tx.insert(&Record::Borrower(borrower.clone()));
            }
        });
    }
}

fn insert_events(kiosk: &mut Kiosk, events: &[Event]) {
    for chunk in events.chunks(1_000) {
        kiosk.wtx(|tx| {
            for event in chunk {
                tx.insert(&Record::Event(event.clone()));
            }
        });
    }
}

fn fixture() -> (Vec<Item>, Vec<Borrower>, Vec<Event>, Vec<Event>) {
    let items: Vec<_> = (0..8_000)
        .map(|index| {
            let (name, nickname, _) = ITEM_NAMES[index % ITEM_NAMES.len()];
            Item {
                id: format!("item-{index:05}"),
                asset_tag: format!("RC-{index:05}"),
                serial: format!("SN{index:08}"),
                name: format!("{name} {index:05}"),
                nickname: format!("{nickname} {index:05}"),
                source_row: format!("items.csv:{}", index + 2),
            }
        })
        .collect();
    let borrowers: Vec<_> = (0..1_200)
        .map(|index| Borrower {
            id: format!("borrower-{index:04}"),
            name: format!("Borrower {index:04}"),
            source_row: format!("borrowers.csv:{}", index + 2),
        })
        .collect();
    let mut events = Vec::with_capacity(100_000);
    for pair in 0..50_000_u64 {
        let item_index = pair as usize % items.len();
        let borrower_index = pair as usize % borrowers.len();
        let checkout_seq = pair * 2;
        events.push(Event {
            id: format!("event-{checkout_seq:06}"),
            sequence: checkout_seq,
            at_day: (checkout_seq / 500) as u32,
            item_id: items[item_index].id.clone(),
            kind: EventKind::Checkout {
                borrower_id: borrowers[borrower_index].id.clone(),
                due_day: (checkout_seq / 500) as u32 + 14,
            },
            source_row: format!("events.csv:{}", checkout_seq + 2),
        });
        events.push(Event {
            id: format!("event-{:06}", checkout_seq + 1),
            sequence: checkout_seq + 1,
            at_day: (checkout_seq / 500) as u32 + 1,
            item_id: items[item_index].id.clone(),
            kind: EventKind::Return,
            source_row: format!("events.csv:{}", checkout_seq + 3),
        });
    }
    let rejected = vec![
        Event {
            id: events[0].id.clone(),
            sequence: 100_000,
            at_day: 300,
            item_id: items[0].id.clone(),
            kind: EventKind::Checkout {
                borrower_id: borrowers[0].id.clone(),
                due_day: 314,
            },
            source_row: "events.csv:100002".to_string(),
        },
        Event {
            id: "event-bad-item".to_string(),
            sequence: 100_001,
            at_day: 300,
            item_id: "item-missing".to_string(),
            kind: EventKind::RepairStart {
                reason: "broken".to_string(),
            },
            source_row: "events.csv:100003".to_string(),
        },
        Event {
            id: "event-bad-borrower".to_string(),
            sequence: 100_002,
            at_day: 300,
            item_id: items[1].id.clone(),
            kind: EventKind::Checkout {
                borrower_id: "borrower-missing".to_string(),
                due_day: 314,
            },
            source_row: "events.csv:100004".to_string(),
        },
        Event {
            id: "event-bad-transition".to_string(),
            sequence: 100_003,
            at_day: 300,
            item_id: items[2].id.clone(),
            kind: EventKind::Return,
            source_row: "events.csv:100005".to_string(),
        },
    ];
    (items, borrowers, events, rejected)
}

fn validate_fixture(
    items: &[Item],
    borrowers: &[Borrower],
    candidates: &[Event],
) -> (Vec<Event>, Vec<(String, String)>) {
    let known_items: HashSet<_> = items.iter().map(|item| item.id.as_str()).collect();
    let known_borrowers: HashSet<_> = borrowers
        .iter()
        .map(|borrower| borrower.id.as_str())
        .collect();
    let mut ids = HashSet::new();
    let mut sequences = HashSet::new();
    let mut latest_sequence = None;
    let mut states: HashMap<&str, ItemState> = HashMap::new();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for event in candidates {
        let result = if ids.contains(event.id.as_str()) {
            Err(format!("duplicate event id {}", event.id))
        } else if sequences.contains(&event.sequence) {
            Err(format!("duplicate event sequence {}", event.sequence))
        } else if let Some(latest) = latest_sequence
            && event.sequence <= latest
        {
            Err(format!(
                "out-of-order event sequence {}; latest accepted sequence is {latest}",
                event.sequence
            ))
        } else if !known_items.contains(event.item_id.as_str()) {
            Err(format!("unknown item {}", event.item_id))
        } else if let EventKind::Checkout { borrower_id, .. } = &event.kind {
            if !known_borrowers.contains(borrower_id.as_str()) {
                Err(format!("unknown borrower {borrower_id}"))
            } else {
                let state = states.entry(event.item_id.as_str()).or_default();
                transition(state, &event.kind).map(|next| *state = next)
            }
        } else {
            let state = states.entry(event.item_id.as_str()).or_default();
            transition(state, &event.kind).map(|next| *state = next)
        };
        match result {
            Ok(()) => {
                ids.insert(event.id.as_str());
                sequences.insert(event.sequence);
                latest_sequence = Some(event.sequence);
                accepted.push(event.clone());
            }
            Err(error) => rejected.push((event.source_row.clone(), error)),
        }
    }
    (accepted, rejected)
}

fn import_fixture(path: &Path) -> ImportSummary {
    let started = Instant::now();
    let (items, borrowers, events, rejected_probes) = fixture();
    let mut candidates = events;
    candidates.extend(rejected_probes);
    let (accepted, mut rejected_rows) = validate_fixture(&items, &borrowers, &candidates);
    rejected_rows.push((
        "events.csv:100006".to_string(),
        "malformed row: missing event id".to_string(),
    ));
    let mut kiosk = open(path);
    insert_masters(&mut kiosk, &items, &borrowers);
    insert_events(&mut kiosk, &accepted);
    kiosk.checkpoint();
    ImportSummary {
        accepted_events: accepted.len(),
        rejected_rows,
        elapsed: started.elapsed(),
    }
}

fn write_canonical_projection(kiosk: &Kiosk, path: &Path) {
    let file = std::fs::File::create(path).expect("create canonical projection");
    let mut writer = BufWriter::new(file);
    kiosk.rtx(|(_, _, _, projections)| {
        for (item_id, projection) in projections.iter() {
            writeln!(writer, "{item_id:?}\t{projection:?}").expect("write canonical row");
        }
    });
    writer.flush().expect("flush canonical projection");
}

fn files_equal(left: &Path, right: &Path) -> bool {
    let left_file = std::fs::File::open(left).expect("open left canonical projection");
    let right_file = std::fs::File::open(right).expect("open right canonical projection");
    let mut left = BufReader::new(left_file);
    let mut right = BufReader::new(right_file);
    let mut left_buf = [0_u8; 64 * 1_024];
    let mut right_buf = [0_u8; 64 * 1_024];
    loop {
        let left_count = left
            .read(&mut left_buf)
            .expect("read left canonical projection");
        let right_count = right
            .read(&mut right_buf)
            .expect("read right canonical projection");
        if left_count != right_count || left_buf[..left_count] != right_buf[..right_count] {
            return false;
        }
        if left_count == 0 {
            return true;
        }
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current.push(std::cmp::min(
                std::cmp::min(current[right_index] + 1, previous[right_index + 1] + 1),
                previous[right_index] + usize::from(left_char != *right_char),
            ));
        }
        previous = current;
    }
    previous[right.len()]
}

fn initials(value: &str) -> String {
    value
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_lowercase)
        .collect()
}

fn search(kiosk: &Kiosk, query: &str, limit: usize) -> Vec<String> {
    let query = normalize(query);
    kiosk.rtx(|(items, _, _, _)| {
        let mut scored: Vec<(usize, String)> = items
            .iter()
            .map(|(_, item)| {
                let fields = [
                    normalize(&item.asset_tag),
                    normalize(&item.serial),
                    normalize(&item.name),
                    normalize(&item.nickname),
                    format!("{}{}", initials(&item.name), normalize(&item.name)),
                ];
                let score = fields
                    .iter()
                    .map(|field| {
                        if field == &query {
                            0
                        } else if field.contains(&query) || query.contains(field) {
                            1
                        } else {
                            edit_distance(field, &query) + 2
                        }
                    })
                    .min()
                    .unwrap_or(usize::MAX);
                (score, item.id)
            })
            .collect();
        scored.sort();
        scored.into_iter().take(limit).map(|(_, id)| id).collect()
    })
}

fn percentile_95(mut timings: Vec<Duration>) -> Duration {
    timings.sort();
    timings[(timings.len() * 95 / 100).min(timings.len() - 1)]
}

fn peak_rss_mib() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied rusage structure on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: a zero getrusage return means the structure was initialized.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    let bytes = usage.ru_maxrss as f64;
    #[cfg(not(target_os = "macos"))]
    let bytes = usage.ru_maxrss as f64 * 1_024.0;
    Some(bytes / 1_048_576.0)
}

fn unique_temp(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!(
            "repair-cafe-{label}-{}-{stamp}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
}

fn seed_small(path: &Path) -> Kiosk {
    let mut kiosk = open(path);
    let item = Item {
        id: "item-00001".to_string(),
        asset_tag: "RC-00001".to_string(),
        serial: "SN00000001".to_string(),
        name: "Cordless Drill 00001".to_string(),
        nickname: "blue drill 00001".to_string(),
        source_row: "items.csv:2".to_string(),
    };
    let borrower = Borrower {
        id: "borrower-0001".to_string(),
        name: "Alex".to_string(),
        source_row: "borrowers.csv:2".to_string(),
    };
    insert_masters(&mut kiosk, &[item], &[borrower]);
    kiosk
}

fn event(id: &str, sequence: u64, at_day: u32, kind: EventKind) -> Event {
    Event {
        id: id.to_string(),
        sequence,
        at_day,
        item_id: "item-00001".to_string(),
        kind,
        source_row: format!("events.csv:{}", sequence + 2),
    }
}

fn run_demo() {
    let path = unique_temp("demo");
    let mut kiosk = seed_small(&path);
    let checkout = event(
        "event-000001",
        1,
        10,
        EventKind::Checkout {
            borrower_id: "borrower-0001".to_string(),
            due_day: 24,
        },
    );
    apply_event(&mut kiosk, checkout.clone()).expect("checkout");
    println!("after checkout: {:?}", state_at(&kiosk, "item-00001", 10));

    let before_rejection = visible_fingerprint(&kiosk);
    let duplicate = apply_event(&mut kiosk, checkout).expect_err("duplicate rejected");
    assert_eq!(before_rejection, visible_fingerprint(&kiosk));
    println!("rejected without mutation: {duplicate}");

    let returned = event("event-000002", 2, 12, EventKind::Return);
    apply_event(&mut kiosk, returned).expect("return");
    kiosk.checkpoint();
    drop(kiosk);
    let kiosk = open(&path);
    println!(
        "reopened current state: {:?}",
        state_at(&kiosk, "item-00001", 99)
    );
    println!(
        "historical day 10: {:?}",
        state_at(&kiosk, "item-00001", 10)
    );
    println!(
        "lookup 'blue dril 00001': {:?}",
        search(&kiosk, "blue dril 00001", 5)
    );
    drop(kiosk);
    std::fs::remove_dir_all(path).expect("remove demo database");
}

fn run_benchmark() {
    let path = unique_temp("benchmark");
    let summary = import_fixture(&path);
    println!(
        "fixture: 8000 items, 1200 borrowers, {} accepted events in {:.3}s",
        summary.accepted_events,
        summary.elapsed.as_secs_f64()
    );
    for (row, error) in &summary.rejected_rows {
        println!("rejected {row}: {error}");
    }
    println!(
        "peak RSS after import: {:.1} MiB",
        peak_rss_mib().expect("read process peak RSS")
    );

    let open_started = Instant::now();
    let kiosk = open(&path);
    let open_elapsed = open_started.elapsed();
    let mut lookup_timings = Vec::with_capacity(1_000);
    for index in 0..1_000 {
        let item_id = format!("item-{:05}", index * 7 % 8_000);
        let started = Instant::now();
        kiosk.rtx(|(_, _, _, projections)| {
            assert!(projections.get(&item_id).is_some());
        });
        lookup_timings.push(started.elapsed());
    }
    let lookup_p95 = percentile_95(lookup_timings);

    let mut search_passes = 0;
    let mut search_total = Duration::ZERO;
    for query_index in 0..50 {
        let item_index = (query_index * 157) % 8_000;
        let (name, nickname, prefix) = ITEM_NAMES[item_index % ITEM_NAMES.len()];
        let mut typo_nickname = nickname.to_string();
        typo_nickname.remove(typo_nickname.len() - 1);
        let query = match query_index % 5 {
            0 => format!("RC-{item_index:05}"),
            1 => format!("SN{item_index:08}"),
            2 => format!("{prefix} {item_index:05}"),
            3 => format!("{typo_nickname} {item_index:05}"),
            _ => format!("{name} {item_index:05}"),
        };
        let started = Instant::now();
        let hits = search(&kiosk, &query, 5);
        search_total += started.elapsed();
        if hits.contains(&format!("item-{item_index:05}")) {
            search_passes += 1;
        }
    }
    let rebuild_root = unique_temp("rebuilds");
    std::fs::create_dir_all(&rebuild_root).expect("create rebuild root");
    let canonical = rebuild_root.join("baseline.current");
    write_canonical_projection(&kiosk, &canonical);
    println!(
        "peak RSS after queries: {:.1} MiB",
        peak_rss_mib().expect("read process peak RSS")
    );
    drop(kiosk);

    let mut rebuild_matches = 0;
    for run in 0..3 {
        let run_root = rebuild_root.join(run.to_string());
        std::fs::create_dir_all(&run_root).expect("create rebuild run root");
        let rebuild_path = run_root.join("db");
        let rebuild_canonical = run_root.join("current.view");
        let status =
            std::process::Command::new(std::env::current_exe().expect("current executable"))
                .arg("rebuild-child")
                .arg(&rebuild_path)
                .arg(&rebuild_canonical)
                .status()
                .expect("run rebuild child");
        assert!(status.success());
        if files_equal(&canonical, &rebuild_canonical) {
            rebuild_matches += 1;
        }
    }
    println!("reopen: {:.3}s", open_elapsed.as_secs_f64());
    println!(
        "warm current-item lookup p95: {:.3}ms",
        lookup_p95.as_secs_f64() * 1_000.0
    );
    println!(
        "search golden set: {search_passes}/50 in {:.3}s",
        search_total.as_secs_f64()
    );
    println!("deterministic rebuilds matching stored view: {rebuild_matches}/3");
    let peak_rss = peak_rss_mib().expect("read process peak RSS");
    println!("coordinator process peak RSS: {peak_rss:.1} MiB");
    assert_eq!(summary.rejected_rows.len(), 5);
    assert!(open_elapsed < Duration::from_secs(2));
    assert!(lookup_p95 < Duration::from_millis(50));
    assert!(search_passes >= 45);
    assert_eq!(rebuild_matches, 3);
    assert!(peak_rss < 256.0);

    std::fs::remove_dir_all(path).expect("remove benchmark database");
    std::fs::remove_dir_all(rebuild_root).expect("remove rebuild databases");
}

fn run_rebuild_child(path: &Path, canonical: &Path) {
    let summary = import_fixture(path);
    assert_eq!(summary.accepted_events, 100_000);
    let kiosk = open(path);
    write_canonical_projection(&kiosk, canonical);
    let peak_rss = peak_rss_mib().expect("read process peak RSS");
    println!("rebuild child peak RSS: {peak_rss:.1} MiB");
    assert!(peak_rss < 256.0);
}

fn run_abort_child(path: &Path) -> ! {
    let mut kiosk = open(path);
    let event = event(
        "event-aborted",
        1,
        10,
        EventKind::Checkout {
            borrower_id: "borrower-0001".to_string(),
            due_day: 24,
        },
    );
    kiosk.wtx(|tx| {
        tx.insert(&Record::Event(event));
        tx.rtx(|(_, _, events, projections)| {
            assert!(events.contains(&"event-aborted".to_string()));
            assert!(projections.contains(&"item-00001".to_string()));
        });
        std::process::abort();
    });
    unreachable!()
}

fn run_interruption() {
    let path = unique_temp("interrupt");
    let kiosk = seed_small(&path);
    let old = visible_fingerprint(&kiosk);
    drop(kiosk);

    let status = std::process::Command::new(std::env::current_exe().expect("current executable"))
        .arg("abort-child")
        .arg(&path)
        .status()
        .expect("run abort child");
    assert!(!status.success());
    let mut reopened = open(&path);
    assert_eq!(old, visible_fingerprint(&reopened));
    println!("interruption inside write: old state recovered");

    apply_event(
        &mut reopened,
        event(
            "event-committed",
            1,
            10,
            EventKind::Checkout {
                borrower_id: "borrower-0001".to_string(),
                due_day: 24,
            },
        ),
    )
    .expect("committed event");
    let committed = visible_fingerprint(&reopened);
    drop(reopened);
    let reopened = open(&path);
    assert_eq!(committed, visible_fingerprint(&reopened));
    println!("interruption after write: complete new state recovered");
    drop(reopened);
    std::fs::remove_dir_all(path).expect("remove interruption database");
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("demo") => run_demo(),
        Some("benchmark") => run_benchmark(),
        Some("interruption") => run_interruption(),
        Some("abort-child") => {
            let path = PathBuf::from(args.next().expect("database path"));
            run_abort_child(&path);
        }
        Some("rebuild-child") => {
            let path = PathBuf::from(args.next().expect("database path"));
            let canonical = PathBuf::from(args.next().expect("canonical projection path"));
            run_rebuild_child(&path, &canonical);
        }
        Some(other) => panic!("unknown command {other:?}; use demo, benchmark, or interruption"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_kiosk(test: impl FnOnce(&mut Kiosk)) {
        let path = unique_temp("test");
        let mut kiosk = seed_small(&path);
        test(&mut kiosk);
        drop(kiosk);
        std::fs::remove_dir_all(path).expect("remove test database");
    }

    #[test]
    fn transition_table_and_rejections_are_atomic() {
        with_kiosk(|kiosk| {
            let cases = [
                event(
                    "checkout",
                    1,
                    1,
                    EventKind::Checkout {
                        borrower_id: "borrower-0001".to_string(),
                        due_day: 14,
                    },
                ),
                event("return", 2, 2, EventKind::Return),
                event(
                    "repair",
                    3,
                    3,
                    EventKind::RepairStart {
                        reason: "switch".to_string(),
                    },
                ),
                event("repair-finish", 4, 4, EventKind::RepairFinish),
                event(
                    "retire",
                    5,
                    5,
                    EventKind::Retire {
                        reason: "unsafe".to_string(),
                    },
                ),
            ];
            for event in cases {
                apply_event(kiosk, event).expect("legal transition");
            }
            assert!(matches!(
                state_at(kiosk, "item-00001", 99),
                Some(ItemState::Retired { .. })
            ));

            for invalid in [
                event("retire", 6, 6, EventKind::Return),
                Event {
                    item_id: "missing".to_string(),
                    id: "unknown-item".to_string(),
                    ..event("unused", 7, 7, EventKind::Return)
                },
                event(
                    "unknown-borrower",
                    8,
                    8,
                    EventKind::Checkout {
                        borrower_id: "missing".to_string(),
                        due_day: 9,
                    },
                ),
            ] {
                let before = visible_fingerprint(kiosk);
                assert!(apply_event(kiosk, invalid).is_err());
                assert_eq!(before, visible_fingerprint(kiosk));
            }
        });
    }

    #[test]
    fn reopening_and_historical_query_are_exact() {
        let path = unique_temp("reopen-test");
        let mut kiosk = seed_small(&path);
        apply_event(
            &mut kiosk,
            event(
                "checkout",
                1,
                10,
                EventKind::Checkout {
                    borrower_id: "borrower-0001".to_string(),
                    due_day: 24,
                },
            ),
        )
        .expect("checkout");
        apply_event(&mut kiosk, event("return", 2, 20, EventKind::Return)).expect("return");
        let before = visible_fingerprint(&kiosk);
        drop(kiosk);
        let kiosk = open(&path);
        assert_eq!(before, visible_fingerprint(&kiosk));
        assert_eq!(
            state_at(&kiosk, "item-00001", 15),
            Some(ItemState::CheckedOut {
                borrower_id: "borrower-0001".to_string(),
                due_day: 24
            })
        );
        assert_eq!(
            state_at(&kiosk, "item-00001", 20),
            Some(ItemState::Available)
        );
        drop(kiosk);
        std::fs::remove_dir_all(path).expect("remove test database");
    }

    #[test]
    fn fixture_validation_reports_every_bad_row() {
        let (items, borrowers, mut events, rejected) = fixture();
        events.extend(rejected);
        let (accepted, rejected) = validate_fixture(&items, &borrowers, &events);
        assert_eq!(accepted.len(), 100_000);
        assert_eq!(rejected.len(), 4);
        assert!(
            rejected
                .iter()
                .any(|(_, error)| error.contains("duplicate"))
        );
        assert!(
            rejected
                .iter()
                .any(|(_, error)| error.contains("unknown item"))
        );
        assert!(
            rejected
                .iter()
                .any(|(_, error)| error.contains("unknown borrower"))
        );
        assert!(
            rejected
                .iter()
                .any(|(_, error)| error.contains("impossible transition"))
        );
    }

    #[test]
    fn live_append_order_rejections_do_not_mutate_and_writer_remains_usable() {
        with_kiosk(|kiosk| {
            apply_event(
                kiosk,
                event(
                    "checkout-sequence-10",
                    10,
                    10,
                    EventKind::Checkout {
                        borrower_id: "borrower-0001".to_string(),
                        due_day: 24,
                    },
                ),
            )
            .expect("initial checkout");

            let invalid = [
                (
                    event("return-sequence-5", 5, 12, EventKind::Return),
                    "out-of-order event sequence 5; latest accepted sequence is 10",
                ),
                (
                    event("return-sequence-10", 10, 12, EventKind::Return),
                    "duplicate event sequence 10",
                ),
            ];
            for (event, expected_error) in invalid {
                let before = visible_fingerprint(kiosk);
                assert_eq!(apply_event(kiosk, event), Err(expected_error.to_string()));
                assert_eq!(visible_fingerprint(kiosk), before);
                assert_eq!(
                    state_at(kiosk, "item-00001", 99),
                    Some(ItemState::CheckedOut {
                        borrower_id: "borrower-0001".to_string(),
                        due_day: 24,
                    })
                );
            }

            apply_event(
                kiosk,
                event("return-sequence-11", 11, 12, EventKind::Return),
            )
            .expect("subsequent valid return");
            assert_eq!(
                state_at(kiosk, "item-00001", 99),
                Some(ItemState::Available)
            );
            kiosk.rtx(|(_, _, events, projections)| {
                assert_eq!(events.iter().count(), 2);
                assert_eq!(
                    projections
                        .get(&"item-00001".to_string())
                        .expect("item projection")
                        .history
                        .len(),
                    2
                );
            });
        });
    }

    #[test]
    fn import_enforces_the_same_append_order_rule() {
        let (fixture_items, fixture_borrowers, _, _) = fixture();
        let items = vec![fixture_items[1].clone()];
        let borrowers = vec![fixture_borrowers[1].clone()];
        let candidates = vec![
            event(
                "checkout-sequence-10",
                10,
                10,
                EventKind::Checkout {
                    borrower_id: borrowers[0].id.clone(),
                    due_day: 24,
                },
            ),
            event("return-sequence-5", 5, 12, EventKind::Return),
            event("return-sequence-10", 10, 12, EventKind::Return),
            event("return-sequence-11", 11, 12, EventKind::Return),
        ];
        let (accepted, rejected) = validate_fixture(&items, &borrowers, &candidates);
        assert_eq!(
            accepted
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(
            rejected,
            vec![
                (
                    "events.csv:7".to_string(),
                    "out-of-order event sequence 5; latest accepted sequence is 10".to_string()
                ),
                (
                    "events.csv:12".to_string(),
                    "duplicate event sequence 10".to_string()
                )
            ]
        );
    }

    #[test]
    fn normalized_search_resolves_to_canonical_item() {
        with_kiosk(|kiosk| {
            assert_eq!(search(kiosk, "RC-00001", 5)[0], "item-00001");
            assert!(search(kiosk, "blue dril 00001", 5).contains(&"item-00001".to_string()));
        });
    }
}
