use undo_history_lab::{Command, Group, Object, ReferenceModel};

fn object(id: u128, x: i64) -> Object {
    Object {
        id,
        z: 0,
        transform: [x, 0, 0, 1_000, 0, 0],
        visible: true,
        fill: 0xff00_00ff,
        points: vec![(10, 20)],
    }
}

#[test]
fn invalid_command_rolls_back_the_whole_group() {
    let mut model = ReferenceModel::default();
    model
        .commit(Group::new(
            "g-1",
            vec![Command::Create {
                object: object(1, 0),
            }],
        ))
        .unwrap();
    let before = model.canonical_json().unwrap();

    let error = model
        .commit(Group::new(
            "g-2",
            vec![
                Command::Move {
                    id: 1,
                    dx: 7,
                    dy: 9,
                },
                Command::Move {
                    id: 999,
                    dx: 1,
                    dy: 1,
                },
            ],
        ))
        .unwrap_err();

    assert_eq!(error.code(), "missing_object");
    assert_eq!(model.canonical_json().unwrap(), before);
    assert_eq!(model.undo_count(), 1);
}

#[test]
fn new_group_after_undo_invalidates_redo() {
    let mut model = ReferenceModel::default();
    model
        .commit(Group::new(
            "g-1",
            vec![Command::Create {
                object: object(1, 0),
            }],
        ))
        .unwrap();
    model
        .commit(Group::new(
            "g-2",
            vec![Command::Move {
                id: 1,
                dx: 10,
                dy: 0,
            }],
        ))
        .unwrap();
    model.undo(1).unwrap();
    assert_eq!(model.redo_count(), 1);

    model
        .commit(Group::new(
            "g-3",
            vec![Command::Recolor {
                id: 1,
                fill: 0x1122_33ff,
            }],
        ))
        .unwrap();

    assert_eq!(model.redo_count(), 0);
    let error = model.redo(1).unwrap_err();
    assert_eq!(error.code(), "redo_beyond_history");
}

#[test]
fn compaction_does_not_discard_the_available_redo_branch() {
    let mut model = ReferenceModel::default();
    for id in 1..=5 {
        model
            .commit(Group::new(
                format!("g-{id}"),
                vec![Command::Create {
                    object: object(id, 0),
                }],
            ))
            .unwrap();
    }
    model.undo(4).unwrap();
    model.compact(2);

    assert_eq!(model.undo_count(), 0);
    assert_eq!(model.redo_count(), 4);
    model.redo(4).unwrap();
    assert_eq!(model.document().objects().len(), 5);
}

#[test]
fn dedup_record_persists_exact_canonical_command_bytes() {
    let mut model = ReferenceModel::default();
    model
        .commit(Group::new(
            "seed",
            vec![Command::Create {
                object: object(1, 0),
            }],
        ))
        .unwrap();
    model
        .commit(Group::new(
            "exact",
            vec![Command::ToggleVisibility { id: 1 }],
        ))
        .unwrap();

    let persisted = serde_json::to_value(model).unwrap();
    let stored: Vec<u8> = persisted["dedup"]["exact"]["canonical_commands"]
        .as_array()
        .expect("dedup must persist canonical command bytes")
        .iter()
        .map(|byte| u8::try_from(byte.as_u64().unwrap()).unwrap())
        .collect();

    assert_eq!(
        stored,
        br#"[{"op":"toggle_visibility","id":"00000000000000000000000000000001"}]"#
    );
}
