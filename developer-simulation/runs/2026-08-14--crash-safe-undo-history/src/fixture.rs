//! Deterministic synthetic illustration and submitted-action generator.

use serde::{Deserialize, Serialize};

use crate::{Action, Command, Group, HistoryStatus, ModelError, Object, ReferenceModel};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub at_action: usize,
    pub status: HistoryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fixture {
    pub version: u8,
    pub seed: u64,
    pub initial_objects: Vec<Object>,
    pub actions: Vec<Action>,
    pub checkpoints: Vec<Checkpoint>,
    pub edit_submissions: usize,
    pub duplicate_edit_submissions: usize,
    pub expected_final: HistoryStatus,
}

pub fn generate_fixture(
    seed: u64,
    object_count: usize,
    action_count: usize,
) -> Result<Fixture, ModelError> {
    if object_count == 0 {
        return Err(ModelError::new(
            "invalid_object_count",
            "at least one initial object is required",
        ));
    }
    let mut random = XorShift64::new(seed);
    let initial_objects: Vec<Object> = (0..object_count)
        .map(|index| make_object(seed, index as u64, &mut random))
        .collect();
    let mut shadow = ReferenceModel::default();
    for (chunk_index, chunk) in initial_objects.chunks(20).enumerate() {
        shadow.commit(Group::new(
            format!("seed-{seed}-initial-{chunk_index}"),
            chunk
                .iter()
                .cloned()
                .map(|object| Command::Create { object })
                .collect(),
        ))?;
    }

    let mut actions = Vec::with_capacity(action_count);
    let mut checkpoints = Vec::with_capacity(action_count / 1_000);
    let mut accepted_groups = Vec::<Group>::new();
    let mut edit_submissions = 0_usize;
    let mut duplicate_edit_submissions = 0_usize;
    let mut next_object = object_count as u64;

    for action_index in 0..action_count {
        let action = if action_index % 50 == 48 && shadow.undo_count() > 0 {
            Action::Undo {
                count: 1 + random.usize(shadow.undo_count().min(4)),
            }
        } else if action_index % 50 == 49 && shadow.redo_count() > 0 {
            Action::Redo {
                count: 1 + random.usize(shadow.redo_count().min(4)),
            }
        } else if action_index % 37 == 36 && shadow.undo_count() > 0 {
            Action::Undo {
                count: 1 + random.usize(shadow.undo_count().min(3)),
            }
        } else {
            edit_submissions += 1;
            let group = if edit_submissions.is_multiple_of(10) && !accepted_groups.is_empty() {
                duplicate_edit_submissions += 1;
                accepted_groups[(edit_submissions / 10 - 1) % accepted_groups.len()].clone()
            } else {
                let command_count = 2 + random.usize(5);
                let mut trial = shadow.clone();
                let mut commands = Vec::with_capacity(command_count);
                for command_index in 0..command_count {
                    let command = generate_command(
                        &trial,
                        seed,
                        &mut next_object,
                        edit_submissions + command_index,
                        &mut random,
                    );
                    trial.commit(Group::new(
                        format!("generator-{action_index}-{command_index}"),
                        vec![command.clone()],
                    ))?;
                    commands.push(command);
                }
                let group = Group::new(format!("seed-{seed}-group-{edit_submissions}"), commands);
                accepted_groups.push(group.clone());
                group
            };
            Action::Commit { group }
        };
        shadow.apply_action(&action)?;
        actions.push(action);
        if (action_index + 1) % 1_000 == 0 {
            checkpoints.push(Checkpoint {
                at_action: action_index + 1,
                status: shadow.status()?,
            });
        }
    }

    Ok(Fixture {
        version: 1,
        seed,
        initial_objects,
        actions,
        checkpoints,
        edit_submissions,
        duplicate_edit_submissions,
        expected_final: shadow.status()?,
    })
}

pub fn seed_model(model: &mut ReferenceModel, fixture: &Fixture) -> Result<(), ModelError> {
    for (chunk_index, chunk) in fixture.initial_objects.chunks(20).enumerate() {
        model.commit(Group::new(
            format!("seed-{}-initial-{chunk_index}", fixture.seed),
            chunk
                .iter()
                .cloned()
                .map(|object| Command::Create { object })
                .collect(),
        ))?;
    }
    Ok(())
}

fn generate_command(
    model: &ReferenceModel,
    seed: u64,
    next_object: &mut u64,
    ordinal: usize,
    random: &mut XorShift64,
) -> Command {
    let ids: Vec<u128> = model.document().objects().keys().copied().collect();
    let id = ids[random.usize(ids.len())];
    let object = &model.document().objects()[&id];
    match ordinal % 9 {
        0 => {
            let object = make_object(seed ^ 0xa5a5_a5a5_a5a5_a5a5, *next_object, random);
            *next_object += 1;
            Command::Create { object }
        }
        1 if ids.len() > 4 => Command::Delete { id },
        2 => Command::Move {
            id,
            dx: random.i64(-500, 500),
            dy: random.i64(-500, 500),
        },
        3 => Command::Recolor {
            id,
            fill: random.next_u32() | 0xff,
        },
        4 => Command::ToggleVisibility { id },
        5 if object.points.len() < 32 => Command::InsertPoint {
            id,
            index: random.usize(object.points.len() + 1),
            point: (random.i64(-10_000, 10_000), random.i64(-10_000, 10_000)),
        },
        6 if !object.points.is_empty() => Command::MovePoint {
            id,
            index: random.usize(object.points.len()),
            dx: random.i64(-100, 100),
            dy: random.i64(-100, 100),
        },
        7 if !object.points.is_empty() => Command::RemovePoint {
            id,
            index: random.usize(object.points.len()),
        },
        _ => Command::Reorder {
            id,
            z: random.i64(-1_000_000, 1_000_000),
        },
    }
}

fn make_object(seed: u64, ordinal: u64, random: &mut XorShift64) -> Object {
    let id = (u128::from(seed) << 64) | u128::from(ordinal + 1);
    let point_count = random.usize(33);
    let points = (0..point_count)
        .map(|_| (random.i64(-10_000, 10_000), random.i64(-10_000, 10_000)))
        .collect();
    Object {
        id,
        z: i64::try_from(ordinal).unwrap_or(i64::MAX),
        transform: [
            1_000,
            random.i64(-50, 50),
            random.i64(-50, 50),
            1_000,
            random.i64(-100_000, 100_000),
            random.i64(-100_000, 100_000),
        ],
        visible: random.next_u64() & 1 == 0,
        fill: random.next_u32() | 0xff,
        points,
    }
}

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn usize(&mut self, upper: usize) -> usize {
        debug_assert!(upper > 0);
        let upper_u64 = u64::try_from(upper).expect("usize fits in u64 on supported hosts");
        usize::try_from(self.next_u64() % upper_u64)
            .expect("remainder is strictly smaller than the usize upper bound")
    }

    fn i64(&mut self, lower: i64, upper: i64) -> i64 {
        debug_assert!(lower <= upper);
        let width = u64::try_from(upper - lower + 1).expect("generator range width is positive");
        let offset = i64::try_from(self.next_u64() % width)
            .expect("generator range widths are bounded by i64::MAX");
        lower + offset
    }

    fn next_u32(&mut self) -> u32 {
        let bytes = self.next_u64().to_le_bytes();
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}
