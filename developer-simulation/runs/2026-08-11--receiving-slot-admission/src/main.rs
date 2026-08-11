use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use fold::pipeline::terminal;
use fold::stream::KeyedStream;
use serde::{Deserialize, Serialize};

const DOOR_COUNT: u16 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Door {
    id: u16,
    height: u8,
    refrigerated: bool,
    pallet_capacity: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HoldSpec {
    tenant: u16,
    request_id: u64,
    arrival: u32,
    duration: u8,
    pallets: u16,
    height: u8,
    refrigerated: bool,
    priority: u8,
    lifetime: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Hold {
        spec: HoldSpec,
        now: u32,
    },
    Confirm {
        tenant: u16,
        request_id: u64,
        now: u32,
    },
    Cancel {
        tenant: u16,
        request_id: u64,
        now: u32,
    },
    Reschedule {
        tenant: u16,
        request_id: u64,
        expected_version: u32,
        arrival: u32,
        duration: u8,
        now: u32,
    },
    Expire {
        sweep_id: u64,
        now: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Held { expires_at: u32 },
    Confirmed,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Booking {
    spec: HoldSpec,
    door: u16,
    version: u32,
    accepted_order: u64,
    status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Outcome {
    accepted: bool,
    code: &'static str,
    door: Option<u16>,
    interval: Option<u32>,
    version: Option<u32>,
}

impl Outcome {
    fn accepted(code: &'static str, booking: Option<&Booking>) -> Self {
        Self {
            accepted: true,
            code,
            door: booking.map(|booking| booking.door),
            interval: booking.map(|booking| booking.spec.arrival),
            version: booking.map(|booking| booking.version),
        }
    }

    fn rejected(code: &'static str, door: Option<u16>, interval: Option<u32>) -> Self {
        Self {
            accepted: false,
            code,
            door,
            interval,
            version: None,
        }
    }
}

#[derive(Clone, Debug)]
struct StoredDecision {
    fingerprint: String,
    outcome: Outcome,
}

#[derive(Clone, Debug)]
struct Audit {
    command_key: String,
    outcome: Outcome,
}

#[derive(Clone, Debug)]
struct Authority {
    doors: Vec<Door>,
    bookings: BTreeMap<(u16, u64), Booking>,
    decisions: HashMap<String, StoredDecision>,
    audits: Vec<Audit>,
    next_order: u64,
}

impl Authority {
    fn one_warehouse() -> Self {
        let doors = (0..DOOR_COUNT)
            .map(|id| Door {
                id,
                height: match id % 3 {
                    0 => 1,
                    1 => 2,
                    _ => 3,
                },
                refrigerated: id % 4 == 0,
                pallet_capacity: 18 + (id % 5) * 4,
            })
            .collect();
        Self {
            doors,
            bookings: BTreeMap::new(),
            decisions: HashMap::new(),
            audits: Vec::new(),
            next_order: 0,
        }
    }

    #[cfg(test)]
    fn one_door(pallet_capacity: u16) -> Self {
        Self {
            doors: vec![Door {
                id: 0,
                height: 3,
                refrigerated: true,
                pallet_capacity,
            }],
            bookings: BTreeMap::new(),
            decisions: HashMap::new(),
            audits: Vec::new(),
            next_order: 0,
        }
    }

    fn execute(&mut self, command: &Command) -> Outcome {
        let key = command_key(command);
        let fingerprint = fingerprint(command);
        if let Some(stored) = self.decisions.get(&key) {
            return if stored.fingerprint == fingerprint {
                stored.outcome.clone()
            } else {
                Outcome::rejected("request_id_payload_mismatch", None, None)
            };
        }

        // The database-supplied logical timestamp governs both cleanup and
        // the command decision. Replays return their durable result without
        // advancing state; every new command first makes expired holds
        // ineligible for capacity in the same modeled authority operation.
        let expired = self.expire_due(command_now(command));
        let outcome = self.execute_new(command, expired);
        self.decisions.insert(
            key.clone(),
            StoredDecision {
                fingerprint,
                outcome: outcome.clone(),
            },
        );
        self.audits.push(Audit {
            command_key: key,
            outcome: outcome.clone(),
        });
        outcome
    }

    fn execute_new(&mut self, command: &Command, expired: usize) -> Outcome {
        match command {
            Command::Hold { spec, now } => self.hold(spec.clone(), *now),
            Command::Confirm {
                tenant,
                request_id,
                now,
            } => self.confirm(*tenant, *request_id, *now),
            Command::Cancel {
                tenant,
                request_id,
                now,
            } => self.cancel(*tenant, *request_id, *now),
            Command::Reschedule {
                tenant,
                request_id,
                expected_version,
                arrival,
                duration,
                now,
            } => self.reschedule(
                *tenant,
                *request_id,
                *expected_version,
                *arrival,
                *duration,
                *now,
            ),
            Command::Expire { .. } => Outcome {
                accepted: true,
                code: "expiry_sweep_applied",
                door: None,
                interval: Some(
                    u32::try_from(expired).expect("expired booking count must fit in u32"),
                ),
                version: None,
            },
        }
    }

    fn hold(&mut self, spec: HoldSpec, now: u32) -> Outcome {
        if spec.duration == 0 || spec.duration > 8 || spec.lifetime == 0 || spec.lifetime > 15 {
            return Outcome::rejected("invalid_request", None, Some(spec.arrival));
        }
        let compatible: Vec<u16> = self
            .doors
            .iter()
            .filter(|door| door.height >= spec.height && (!spec.refrigerated || door.refrigerated))
            .map(|door| door.id)
            .collect();
        if compatible.is_empty() {
            return Outcome::rejected("no_compatible_door", None, Some(spec.arrival));
        }
        let (door, rejection) = match self.choose_door(&spec, None, &compatible) {
            Ok(door) => (door, None),
            Err(rejection) => (0, Some(rejection)),
        };
        if let Some(rejection) = rejection {
            return rejection;
        }
        let order = self.next_order;
        self.next_order += 1;
        let key = (spec.tenant, spec.request_id);
        let expires_at = now + spec.lifetime;
        let booking = Booking {
            spec,
            door,
            version: 0,
            accepted_order: order,
            status: Status::Held { expires_at },
        };
        self.bookings.insert(key, booking);
        Outcome::accepted("hold_accepted", self.bookings.get(&key))
    }

    fn confirm(&mut self, tenant: u16, request_id: u64, now: u32) -> Outcome {
        let key = (tenant, request_id);
        let Some(existing) = self.bookings.get(&key).cloned() else {
            return Outcome::rejected("booking_not_found", None, None);
        };
        match existing.status {
            Status::Held { expires_at } if expires_at <= now => {
                self.bookings.get_mut(&key).expect("booking exists").status = Status::Expired;
                Outcome::rejected(
                    "expired_hold",
                    Some(existing.door),
                    Some(existing.spec.arrival),
                )
            }
            Status::Held { .. } => {
                for interval in
                    existing.spec.arrival..existing.spec.arrival + u32::from(existing.spec.duration)
                {
                    if self.bookings.iter().any(|(other_key, booking)| {
                        *other_key != key
                            && booking.door == existing.door
                            && matches!(booking.status, Status::Confirmed)
                            && overlaps(booking.spec.arrival, booking.spec.duration, interval)
                    }) {
                        return Outcome::rejected(
                            "time_conflict",
                            Some(existing.door),
                            Some(interval),
                        );
                    }
                }
                let booking = self.bookings.get_mut(&key).expect("booking exists");
                booking.status = Status::Confirmed;
                Outcome::accepted("booking_confirmed", Some(booking))
            }
            Status::Confirmed => Outcome::accepted("booking_confirmed", Some(&existing)),
            Status::Expired => Outcome::rejected(
                "expired_hold",
                Some(existing.door),
                Some(existing.spec.arrival),
            ),
            Status::Cancelled => Outcome::rejected(
                "booking_cancelled",
                Some(existing.door),
                Some(existing.spec.arrival),
            ),
        }
    }

    fn cancel(&mut self, tenant: u16, request_id: u64, now: u32) -> Outcome {
        let key = (tenant, request_id);
        let Some(existing) = self.bookings.get(&key).cloned() else {
            return Outcome::rejected("booking_not_found", None, None);
        };
        if matches!(existing.status, Status::Held { expires_at } if expires_at <= now) {
            self.bookings.get_mut(&key).expect("booking exists").status = Status::Expired;
            return Outcome::rejected(
                "expired_hold",
                Some(existing.door),
                Some(existing.spec.arrival),
            );
        }
        if matches!(existing.status, Status::Cancelled | Status::Expired) {
            return Outcome::rejected(
                "booking_inactive",
                Some(existing.door),
                Some(existing.spec.arrival),
            );
        }
        let booking = self.bookings.get_mut(&key).expect("booking exists");
        booking.status = Status::Cancelled;
        Outcome::accepted("booking_cancelled", Some(booking))
    }

    fn reschedule(
        &mut self,
        tenant: u16,
        request_id: u64,
        expected_version: u32,
        arrival: u32,
        duration: u8,
        now: u32,
    ) -> Outcome {
        let key = (tenant, request_id);
        let Some(existing) = self.bookings.get(&key).cloned() else {
            return Outcome::rejected("booking_not_found", None, Some(arrival));
        };
        if !(1..=8).contains(&duration) {
            return Outcome::rejected("invalid_request", Some(existing.door), Some(arrival));
        }
        if existing.version != expected_version {
            return Outcome::rejected(
                "stale_reschedule_version",
                Some(existing.door),
                Some(arrival),
            );
        }
        if matches!(existing.status, Status::Held { expires_at } if expires_at <= now) {
            self.bookings.get_mut(&key).expect("booking exists").status = Status::Expired;
            return Outcome::rejected(
                "expired_hold",
                Some(existing.door),
                Some(existing.spec.arrival),
            );
        }
        if matches!(existing.status, Status::Cancelled | Status::Expired) {
            return Outcome::rejected("booking_inactive", Some(existing.door), Some(arrival));
        }
        let mut proposed = existing.spec.clone();
        proposed.arrival = arrival;
        proposed.duration = duration;
        let compatible: Vec<u16> = self
            .doors
            .iter()
            .filter(|door| {
                door.height >= proposed.height && (!proposed.refrigerated || door.refrigerated)
            })
            .map(|door| door.id)
            .collect();
        let door = match self.choose_door(&proposed, Some(key), &compatible) {
            Ok(door) => door,
            Err(outcome) => return outcome,
        };
        let booking = self.bookings.get_mut(&key).expect("booking exists");
        booking.spec = proposed;
        booking.door = door;
        booking.version += 1;
        Outcome::accepted("booking_rescheduled", Some(booking))
    }

    fn choose_door(
        &self,
        spec: &HoldSpec,
        exclude: Option<(u16, u64)>,
        compatible: &[u16],
    ) -> Result<u16, Outcome> {
        let mut first_time_conflict = None;
        let mut first_capacity_conflict = None;
        for &door_id in compatible {
            let door = &self.doors[usize::from(door_id)];
            let mut rejected = false;
            for interval in spec.arrival..spec.arrival + u32::from(spec.duration) {
                let mut used = 0u16;
                for (key, booking) in &self.bookings {
                    if Some(*key) == exclude
                        || booking.door != door_id
                        || !is_active(&booking.status)
                    {
                        continue;
                    }
                    if overlaps(booking.spec.arrival, booking.spec.duration, interval) {
                        if matches!(booking.status, Status::Confirmed) {
                            first_time_conflict.get_or_insert((door_id, interval));
                            rejected = true;
                            break;
                        }
                        used = used.saturating_add(booking.spec.pallets);
                    }
                }
                if rejected {
                    break;
                }
                if used.saturating_add(spec.pallets) > door.pallet_capacity {
                    first_capacity_conflict.get_or_insert((door_id, interval));
                    rejected = true;
                    break;
                }
            }
            if !rejected {
                return Ok(door_id);
            }
        }
        if let Some((door, interval)) = first_time_conflict {
            Err(Outcome::rejected(
                "time_conflict",
                Some(door),
                Some(interval),
            ))
        } else if let Some((door, interval)) = first_capacity_conflict {
            Err(Outcome::rejected(
                "pallet_capacity",
                Some(door),
                Some(interval),
            ))
        } else {
            Err(Outcome::rejected(
                "no_compatible_door",
                None,
                Some(spec.arrival),
            ))
        }
    }

    fn expire_due(&mut self, now: u32) -> usize {
        let mut count = 0;
        for booking in self.bookings.values_mut() {
            if matches!(booking.status, Status::Held { expires_at } if expires_at <= now) {
                booking.status = Status::Expired;
                count += 1;
            }
        }
        count
    }

    fn check_invariants(&self) -> Result<(), String> {
        if self.audits.len() != self.decisions.len() {
            return Err("decision/audit cardinality mismatch".to_string());
        }
        for (key, stored) in &self.decisions {
            let audit = self
                .audits
                .iter()
                .find(|audit| audit.command_key == *key)
                .ok_or_else(|| format!("decision {key} has no audit"))?;
            if audit.outcome != stored.outcome {
                return Err(format!("decision {key} differs from its audit"));
            }
        }
        for door in &self.doors {
            let max_interval = self
                .bookings
                .values()
                .map(|booking| booking.spec.arrival + u32::from(booking.spec.duration))
                .max()
                .unwrap_or(0);
            for interval in 0..max_interval {
                let active: Vec<&Booking> = self
                    .bookings
                    .values()
                    .filter(|booking| {
                        booking.door == door.id
                            && is_active(&booking.status)
                            && overlaps(booking.spec.arrival, booking.spec.duration, interval)
                    })
                    .collect();
                let used: u16 = active.iter().map(|booking| booking.spec.pallets).sum();
                if used > door.pallet_capacity {
                    return Err(format!(
                        "door {} interval {interval} uses {used}>{}",
                        door.id, door.pallet_capacity
                    ));
                }
                let confirmed = active
                    .iter()
                    .filter(|booking| matches!(booking.status, Status::Confirmed))
                    .count();
                if confirmed > 1 {
                    return Err(format!(
                        "door {} interval {interval} has {confirmed} confirmed bookings",
                        door.id
                    ));
                }
            }
        }
        Ok(())
    }
}

fn is_active(status: &Status) -> bool {
    matches!(status, Status::Held { .. } | Status::Confirmed)
}

fn overlaps(arrival: u32, duration: u8, interval: u32) -> bool {
    interval >= arrival && interval < arrival + u32::from(duration)
}

fn command_now(command: &Command) -> u32 {
    match command {
        Command::Hold { now, .. }
        | Command::Confirm { now, .. }
        | Command::Cancel { now, .. }
        | Command::Reschedule { now, .. }
        | Command::Expire { now, .. } => *now,
    }
}

fn command_key(command: &Command) -> String {
    match command {
        Command::Hold { spec, .. } => format!("{}:{}:hold", spec.tenant, spec.request_id),
        Command::Confirm {
            tenant, request_id, ..
        } => format!("{tenant}:{request_id}:confirm"),
        Command::Cancel {
            tenant, request_id, ..
        } => format!("{tenant}:{request_id}:cancel"),
        Command::Reschedule {
            tenant,
            request_id,
            expected_version,
            ..
        } => {
            format!("{tenant}:{request_id}:reschedule:{expected_version}")
        }
        Command::Expire { sweep_id, .. } => format!("expiry:{sweep_id}"),
    }
}

fn fingerprint(command: &Command) -> String {
    match command {
        Command::Hold { spec, .. } => format!(
            "hold/{}/{}/{}/{}/{}/{}/{}/{}/{}",
            spec.tenant,
            spec.request_id,
            spec.arrival,
            spec.duration,
            spec.pallets,
            spec.height,
            spec.refrigerated,
            spec.priority,
            spec.lifetime
        ),
        Command::Confirm {
            tenant, request_id, ..
        } => format!("confirm/{tenant}/{request_id}"),
        Command::Cancel {
            tenant, request_id, ..
        } => format!("cancel/{tenant}/{request_id}"),
        Command::Reschedule {
            tenant,
            request_id,
            expected_version,
            arrival,
            duration,
            ..
        } => {
            format!("reschedule/{tenant}/{request_id}/{expected_version}/{arrival}/{duration}")
        }
        Command::Expire { sweep_id, now } => format!("expire/{sweep_id}/{now}"),
    }
}

#[derive(Clone)]
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn range(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}

fn fixture(seed: u64, len: usize) -> Vec<Command> {
    let mut rng = Lcg(seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut commands = Vec::with_capacity(len);
    let mut specs: Vec<HoldSpec> = Vec::new();
    for index in 0..len {
        let now = u32::try_from(index / 6)
            .expect("fixture logical timestamp derived from command count must fit in u32");
        let command = match index % 10 {
            0..=4 => {
                let request_id =
                    u64::try_from(specs.len()).expect("fixture request count must fit in u64");
                let spec = HoldSpec {
                    tenant: u16::try_from(rng.range(4) + 1)
                        .expect("bounded tenant value in 1..=4 must fit in u16"),
                    request_id,
                    arrival: u32::try_from(rng.range(96))
                        .expect("bounded arrival value below 96 must fit in u32"),
                    duration: u8::try_from(rng.range(8) + 1)
                        .expect("bounded duration value in 1..=8 must fit in u8"),
                    pallets: u16::try_from(rng.range(18) + 1)
                        .expect("bounded pallet value in 1..=18 must fit in u16"),
                    height: u8::try_from(rng.range(3) + 1)
                        .expect("bounded height value in 1..=3 must fit in u8"),
                    refrigerated: rng.range(5) == 0,
                    priority: u8::try_from(rng.range(4))
                        .expect("bounded priority value below 4 must fit in u8"),
                    lifetime: u32::try_from(rng.range(15) + 1)
                        .expect("bounded lifetime value in 1..=15 must fit in u32"),
                };
                specs.push(spec.clone());
                Command::Hold { spec, now }
            }
            5 if !specs.is_empty() => {
                let spec = specs[random_index(&mut rng, specs.len())].clone();
                Command::Hold { spec, now }
            }
            6 if !specs.is_empty() => {
                let spec = &specs[random_index(&mut rng, specs.len())];
                Command::Confirm {
                    tenant: spec.tenant,
                    request_id: spec.request_id,
                    now,
                }
            }
            7 if !specs.is_empty() => {
                let spec = &specs[random_index(&mut rng, specs.len())];
                Command::Reschedule {
                    tenant: spec.tenant,
                    request_id: spec.request_id,
                    expected_version: u32::try_from(rng.range(3))
                        .expect("bounded version value below 3 must fit in u32"),
                    arrival: u32::try_from(rng.range(96))
                        .expect("bounded arrival value below 96 must fit in u32"),
                    duration: u8::try_from(rng.range(8) + 1)
                        .expect("bounded duration value in 1..=8 must fit in u8"),
                    now,
                }
            }
            8 if !specs.is_empty() => {
                let spec = &specs[random_index(&mut rng, specs.len())];
                Command::Cancel {
                    tenant: spec.tenant,
                    request_id: spec.request_id,
                    now,
                }
            }
            _ => Command::Expire {
                sweep_id: u64::try_from(index).expect("fixture command index must fit in u64"),
                now,
            },
        };
        commands.push(command);
    }
    commands
}

fn random_index(rng: &mut Lcg, len: usize) -> usize {
    let upper = u64::try_from(len).expect("fixture collection length must fit in u64");
    usize::try_from(rng.range(upper))
        .expect("random index below a usize-derived bound must fit in usize")
}

fn run_sequential() -> Result<(), String> {
    let deterministic = fixture(7, 10_000);
    let mut first = Authority::one_warehouse();
    let first_outcomes: Vec<Outcome> = deterministic
        .iter()
        .map(|command| first.execute(command))
        .collect();
    first.check_invariants()?;

    let mut replay = Authority::one_warehouse();
    let replay_outcomes: Vec<Outcome> = deterministic
        .iter()
        .map(|command| replay.execute(command))
        .collect();
    if replay_outcomes != first_outcomes || replay.bookings != first.bookings {
        return Err("same deterministic fixture produced different results".to_string());
    }

    for seed in 0..100 {
        let commands = fixture(seed, 200);
        let mut left = Authority::one_warehouse();
        let mut right = Authority::one_warehouse();
        for command in &commands {
            if left.execute(command) != right.execute(command) {
                return Err(format!("seed {seed} replay diverged"));
            }
        }
        left.check_invariants()?;
        right.check_invariants()?;
        if left.bookings != right.bookings {
            return Err(format!("seed {seed} final state diverged"));
        }
    }
    println!(
        "sequential: 10,000-command fixture + 100 generated seeds deterministic; local invariants passed"
    );
    Ok(())
}

fn run_concurrency_model() -> Result<(), String> {
    for run in 0..30u64 {
        let shared = Arc::new(Mutex::new(Authority::one_warehouse()));
        let commands = Arc::new(fixture(50_000 + run, 400));
        let mut workers = Vec::new();
        for _ in 0..4 {
            let shared = Arc::clone(&shared);
            let commands = Arc::clone(&commands);
            workers.push(thread::spawn(move || {
                for command in commands.iter() {
                    shared
                        .lock()
                        .expect("authority mutex poisoned")
                        .execute(command);
                }
            }));
        }
        for worker in workers {
            worker.join().map_err(|_| "worker panicked".to_string())?;
        }
        let state = shared
            .lock()
            .map_err(|_| "authority mutex poisoned".to_string())?;
        state.check_invariants()?;
    }
    println!(
        "concurrency-model: 4 threads x 30 runs passed local mutex/invariant checks (not PostgreSQL or six-process evidence)"
    );
    Ok(())
}

fn verify_boundary() -> Result<(), String> {
    let root = std::env::temp_dir().join(format!("bogkit-dock-boundary-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;

    run_crash_case(&root, "before", 41, false)?;
    run_crash_case(&root, "after", 42, true)?;

    fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    println!(
        "crash-boundary: real child exits before and after mocked authoritative commit reproduced independent Fold commit gap"
    );
    Ok(())
}

fn run_crash_case(
    root: &Path,
    point: &str,
    expected_code: i32,
    authority_should_exist: bool,
) -> Result<(), String> {
    let case = root.join(point);
    fs::create_dir_all(&case).map_err(|error| error.to_string())?;
    let status = ProcessCommand::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg("--crash-child")
        .arg(point)
        .arg(&case)
        .status()
        .map_err(|error| error.to_string())?;
    assert_exit(status, expected_code)?;

    let authority_path = case.join("postgres-mock-transaction.txt");
    if authority_path.exists() != authority_should_exist {
        return Err(format!(
            "{point}: unexpected authoritative state after child exit"
        ));
    }
    if authority_path.exists() {
        let body = fs::read_to_string(&authority_path).map_err(|error| error.to_string())?;
        if !body.contains("booking=req-crash,status=held")
            || !body.contains("audit=req-crash,outcome=hold_accepted")
        {
            return Err(format!(
                "{point}: booking and audit were not atomically present"
            ));
        }
    }

    let mirror_path = case.join("fold-mirror");
    let mut mirror = KeyedStream::new(&mirror_path, terminal::Table::new("decisions"));
    let request = "req-crash".to_string();
    if mirror.get(&request).is_some() {
        return Err(format!("{point}: Fold unexpectedly committed before crash"));
    }

    if !authority_path.exists() {
        atomic_authority_commit(&authority_path)?;
    }
    let original = MirrorDecision {
        code: "hold_accepted".to_string(),
        door: 0,
    };
    mirror.wtx(|tx| {
        tx.upsert(&request, &original);
    });
    if mirror.get(&request) != Some(original) {
        return Err(format!(
            "{point}: retry did not repair mirror with original durable result"
        ));
    }
    Ok(())
}

fn assert_exit(status: ExitStatus, expected: i32) -> Result<(), String> {
    match status.code() {
        Some(code) if code == expected => Ok(()),
        other => Err(format!("child exit was {other:?}, expected {expected}")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct MirrorDecision {
    code: String,
    door: u16,
}

fn crash_child(point: &str, case: &Path) -> ! {
    if point == "before" {
        std::process::exit(41);
    }
    let authority_path = case.join("postgres-mock-transaction.txt");
    atomic_authority_commit(&authority_path).expect("mocked authoritative commit failed");
    std::process::exit(42);
}

fn atomic_authority_commit(path: &Path) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        "booking=req-crash,status=held\naudit=req-crash,outcome=hold_accepted\n",
    )
    .map_err(|error| error.to_string())?;
    fs::File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn run_local_benchmark() -> Result<(), String> {
    let commands = fixture(99, 1_000);
    let mut state = Authority::one_warehouse();
    let mut samples = Vec::with_capacity(100_000);
    let started = Instant::now();
    for _ in 0..100 {
        for command in &commands {
            let one = Instant::now();
            state.execute(command);
            samples.push(one.elapsed());
        }
    }
    let elapsed = started.elapsed();
    samples.sort_unstable();
    let p95 = samples[samples.len() * 95 / 100];
    let throughput = 100_000.0 / elapsed.as_secs_f64();
    state.check_invariants()?;
    println!(
        "local-benchmark: 100,000 commands, {:.0} commands/s, p95 {:.6} ms (repeat-heavy in-memory model; invalid for PostgreSQL acceptance)",
        throughput,
        p95.as_secs_f64() * 1_000.0
    );
    Ok(())
}

fn run_demo() -> Result<(), String> {
    run_sequential()?;
    run_concurrency_model()?;
    verify_boundary()?;
    run_local_benchmark()?;

    let mut state = Authority::one_warehouse();
    let spec = HoldSpec {
        tenant: 7,
        request_id: 44,
        arrival: 10,
        duration: 2,
        pallets: 8,
        height: 1,
        refrigerated: true,
        priority: 2,
        lifetime: 15,
    };
    let held = state.execute(&Command::Hold {
        spec: spec.clone(),
        now: 100,
    });
    let retry = state.execute(&Command::Hold {
        spec: spec.clone(),
        now: 110,
    });
    if held != retry {
        return Err("idempotent hold retry changed the durable outcome".to_string());
    }
    let mismatch = state.execute(&Command::Hold {
        spec: HoldSpec { pallets: 9, ..spec },
        now: 110,
    });
    if mismatch.code != "request_id_payload_mismatch" {
        return Err("payload mismatch was not visible".to_string());
    }
    let equality = state.execute(&Command::Confirm {
        tenant: 7,
        request_id: 44,
        now: 115,
    });
    if equality.code != "expired_hold" {
        return Err("confirm-at-expiry rule was not expiry-wins".to_string());
    }
    println!("edge-cases: exact retry replayed; payload mismatch visible; expiry wins at equality");
    println!(
        "fit: NO-FIT for production authority because Fold owns a separate fjall transaction and cannot join PostgreSQL"
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--crash-child") {
        let point = args.get(2).expect("missing crash point");
        let case = PathBuf::from(args.get(3).expect("missing case path"));
        crash_child(point, &case);
    }
    if let Err(error) = run_demo() {
        eprintln!("ERROR: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_fixtures_are_deterministic_and_safe() {
        run_sequential().unwrap();
    }

    #[test]
    fn exact_retry_and_payload_mismatch_are_distinct() {
        let mut state = Authority::one_warehouse();
        let spec = HoldSpec {
            tenant: 1,
            request_id: 2,
            arrival: 4,
            duration: 2,
            pallets: 4,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 10,
        };
        let original = state.execute(&Command::Hold {
            spec: spec.clone(),
            now: 0,
        });
        let retry = state.execute(&Command::Hold {
            spec: spec.clone(),
            now: 9,
        });
        assert_eq!(original, retry);
        let mismatch = state.execute(&Command::Hold {
            spec: HoldSpec { pallets: 5, ..spec },
            now: 9,
        });
        assert_eq!(mismatch.code, "request_id_payload_mismatch");
        assert_eq!(state.audits.len(), 1);
    }

    #[test]
    fn expiry_wins_at_equality() {
        let mut state = Authority::one_warehouse();
        let spec = HoldSpec {
            tenant: 1,
            request_id: 3,
            arrival: 8,
            duration: 1,
            pallets: 4,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 15,
        };
        assert!(state.execute(&Command::Hold { spec, now: 10 }).accepted);
        assert_eq!(
            state
                .execute(&Command::Confirm {
                    tenant: 1,
                    request_id: 3,
                    now: 25
                })
                .code,
            "expired_hold"
        );
    }

    #[test]
    fn reschedule_version_and_command_identity_are_checked() {
        let mut state = Authority::one_warehouse();
        let spec = HoldSpec {
            tenant: 1,
            request_id: 4,
            arrival: 8,
            duration: 1,
            pallets: 4,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 15,
        };
        assert!(state.execute(&Command::Hold { spec, now: 10 }).accepted);
        let changed = state.execute(&Command::Reschedule {
            tenant: 1,
            request_id: 4,
            expected_version: 0,
            arrival: 20,
            duration: 2,
            now: 11,
        });
        assert_eq!(changed.code, "booking_rescheduled");
        let reused_operation = state.execute(&Command::Reschedule {
            tenant: 1,
            request_id: 4,
            expected_version: 0,
            arrival: 21,
            duration: 2,
            now: 12,
        });
        assert_eq!(reused_operation.code, "request_id_payload_mismatch");

        let distinct_wrong_version = state.execute(&Command::Reschedule {
            tenant: 1,
            request_id: 4,
            expected_version: 99,
            arrival: 22,
            duration: 2,
            now: 12,
        });
        assert_eq!(distinct_wrong_version.code, "stale_reschedule_version");
    }

    #[test]
    fn expired_hold_releases_capacity_without_an_explicit_sweep() {
        let mut state = Authority::one_door(10);
        let expired = HoldSpec {
            tenant: 1,
            request_id: 10,
            arrival: 20,
            duration: 1,
            pallets: 10,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 5,
        };
        assert!(
            state
                .execute(&Command::Hold {
                    spec: expired,
                    now: 10
                })
                .accepted
        );

        let replacement = HoldSpec {
            tenant: 1,
            request_id: 11,
            arrival: 20,
            duration: 1,
            pallets: 10,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 5,
        };
        assert!(
            state
                .execute(&Command::Hold {
                    spec: replacement,
                    now: 15
                })
                .accepted
        );
        assert!(matches!(state.bookings[&(1, 10)].status, Status::Expired));
    }

    #[test]
    fn reschedule_ignores_unrelated_hold_expired_at_command_time() {
        let mut state = Authority::one_door(10);
        let moving = HoldSpec {
            tenant: 1,
            request_id: 20,
            arrival: 10,
            duration: 1,
            pallets: 6,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 15,
        };
        let blocker = HoldSpec {
            tenant: 1,
            request_id: 21,
            arrival: 30,
            duration: 1,
            pallets: 10,
            height: 1,
            refrigerated: false,
            priority: 1,
            lifetime: 1,
        };
        assert!(
            state
                .execute(&Command::Hold {
                    spec: moving,
                    now: 0
                })
                .accepted
        );
        assert!(
            state
                .execute(&Command::Hold {
                    spec: blocker,
                    now: 0
                })
                .accepted
        );

        let changed = state.execute(&Command::Reschedule {
            tenant: 1,
            request_id: 20,
            expected_version: 0,
            arrival: 30,
            duration: 1,
            now: 1,
        });
        assert_eq!(changed.code, "booking_rescheduled");
        assert!(matches!(state.bookings[&(1, 21)].status, Status::Expired));
    }

    #[test]
    fn invalid_reschedule_durations_do_not_mutate_booking_or_version() {
        for duration in [0, 9] {
            let mut state = Authority::one_door(10);
            let original = HoldSpec {
                tenant: 1,
                request_id: 30,
                arrival: 10,
                duration: 2,
                pallets: 6,
                height: 1,
                refrigerated: false,
                priority: 1,
                lifetime: 15,
            };
            assert!(
                state
                    .execute(&Command::Hold {
                        spec: original,
                        now: 0
                    })
                    .accepted
            );
            let before = state.bookings[&(1, 30)].clone();

            let rejected = state.execute(&Command::Reschedule {
                tenant: 1,
                request_id: 30,
                expected_version: 0,
                arrival: 40,
                duration,
                now: 1,
            });
            assert_eq!(rejected.code, "invalid_request");
            assert_eq!(state.bookings[&(1, 30)], before);
        }
    }

    #[test]
    fn four_worker_collision_model_preserves_invariants() {
        run_concurrency_model().unwrap();
    }
}
