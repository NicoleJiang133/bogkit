use std::panic::{AssertUnwindSafe, catch_unwind};

use fold::pipeline::terminal;
use fold::stream::KeyedStream;

#[test]
fn returning_an_error_from_fold_write_closure_still_commits_prior_writes() {
    // Characterizes the documented boundary relevant to decoded-batch rejection.
    let temporary = tempfile::tempdir().unwrap();
    let mut stream = KeyedStream::new(
        temporary.path(),
        terminal::Table::<u64, String>::new("rows"),
    );
    let result: Result<(), &'static str> = stream.wtx(|transaction| {
        transaction.upsert(&1, &"first response".to_owned());
        Err("second response was invalid")
    });

    assert_eq!(result, Err("second response was invalid"));
    assert_eq!(stream.get(&1), Some("first response".to_owned()));
}

#[test]
fn caught_panic_rolls_back_rejected_row_but_poisons_the_same_streams_later_write() {
    // Characterizes the tracked current-main defect: rollback happens, but the writer is unusable.
    let temporary = tempfile::tempdir().unwrap();
    let mut stream = KeyedStream::new(
        temporary.path(),
        terminal::Table::<u64, String>::new("rows"),
    );
    let result = catch_unwind(AssertUnwindSafe(|| {
        stream.wtx(|transaction| {
            transaction.upsert(&1, &"first response".to_owned());
            panic!("reject batch");
        });
    }));

    assert!(result.is_err());
    assert_eq!(stream.get(&1), None);

    let later_write = catch_unwind(AssertUnwindSafe(|| {
        stream.wtx(|transaction| {
            transaction.upsert(&2, &"later valid response".to_owned());
        });
    }));
    let poison = later_write.expect_err("current main unexpectedly accepted the later write");
    let message = poison
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| poison.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic");
    assert!(
        message.contains("poisoned tx lock"),
        "unexpected panic: {message}"
    );
    assert_eq!(stream.get(&2), None);
}
