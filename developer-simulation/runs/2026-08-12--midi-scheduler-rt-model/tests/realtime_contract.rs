use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};

use midi_scheduler_rt_model::{
    CallbackError, Discontinuity, Event, EventKind, HostBlock, PlanSpec, ScheduledEvent, Scheduler,
    TempoNode, prepare_plan,
};

struct CountingAllocator;

thread_local! {
    static TRAP_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TRAP_ACTIVE.with(|active| {
            if active.get() {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: forwarding the allocator contract unchanged to `System`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        TRAP_ACTIVE.with(|active| {
            if active.get() {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: forwarding the allocator contract unchanged to `System`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` came from the forwarded `System` allocator.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        TRAP_ACTIVE.with(|active| {
            if active.get() {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: forwarding the allocator contract unchanged to `System`.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn plan() -> midi_scheduler_rt_model::PreparedPlan {
    let events = (0_i64..64)
        .map(|pulse| Event {
            pulse,
            stable_id: pulse.cast_unsigned() + 1,
            kind: EventKind::ControlChange {
                channel: 0,
                controller: 1,
                value: u8::try_from(pulse).expect("fixture value fits"),
            },
        })
        .collect();
    prepare_plan(PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 20_000,
        }],
        events,
    })
    .expect("instrumentation plan")
}

#[test]
fn one_hundred_thousand_instrumented_callbacks_allocate_zero_times() {
    // Break caught: any callback path grows a Vec/Box/String or another heap-backed collection.
    let plan = plan();
    let mut scheduler = Scheduler::new(8);
    let mut output = [ScheduledEvent::EMPTY; 8];
    ALLOCATIONS.store(0, Ordering::Relaxed);

    for iteration in 0_u64..100_000 {
        let host = HostBlock {
            token: iteration + 1,
            start_frame: i64::try_from(iteration % 64).expect("bounded frame fits"),
            length: 1,
            sample_rate: 48_000,
            discontinuity: Discontinuity::None,
        };
        TRAP_ACTIVE.with(|active| active.set(true));
        let result = scheduler.schedule(&plan, host, &mut output);
        TRAP_ACTIVE.with(|active| active.set(false));
        assert_eq!(result.error, None);
    }

    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
}

#[test]
fn forced_invalid_callback_inputs_return_flags_without_unwinding() {
    // Break caught: checked host validation is replaced with assert/unwrap arithmetic.
    let plan = plan();
    let mut scheduler = Scheduler::new(8);
    let mut output = [ScheduledEvent::EMPTY; 8];
    let invalid = [
        (
            HostBlock {
                token: 1,
                start_frame: 0,
                length: 0,
                sample_rate: 48_000,
                discontinuity: Discontinuity::None,
            },
            CallbackError::EmptyBlock,
        ),
        (
            HostBlock {
                token: 2,
                start_frame: 0,
                length: 2_049,
                sample_rate: 48_000,
                discontinuity: Discontinuity::None,
            },
            CallbackError::BlockTooLarge,
        ),
        (
            HostBlock {
                token: 3,
                start_frame: 0,
                length: 1,
                sample_rate: 12_345,
                discontinuity: Discontinuity::None,
            },
            CallbackError::UnsupportedSampleRate,
        ),
        (
            HostBlock {
                token: 4,
                start_frame: 0,
                length: 1,
                sample_rate: 96_000,
                discontinuity: Discontinuity::None,
            },
            CallbackError::PlanSampleRateMismatch,
        ),
        (
            HostBlock {
                token: 5,
                start_frame: i64::MAX,
                length: 1,
                sample_rate: 48_000,
                discontinuity: Discontinuity::None,
            },
            CallbackError::FrameRangeOverflow,
        ),
    ];

    for (host, expected) in invalid {
        let boundary = catch_unwind(AssertUnwindSafe(|| {
            scheduler.schedule(&plan, host, &mut output)
        }));
        let result = boundary.expect("callback must not unwind");
        assert_eq!(result.error, Some(expected));
    }

    let mut empty_output = [];
    let boundary = catch_unwind(AssertUnwindSafe(|| {
        scheduler.schedule(
            &plan,
            HostBlock {
                token: 6,
                start_frame: 0,
                length: 1,
                sample_rate: 48_000,
                discontinuity: Discontinuity::None,
            },
            &mut empty_output,
        )
    }));
    let result = boundary.expect("zero-capacity callback must not unwind");
    assert_eq!(result.written, 0);
    assert!(result.overflowed);
}
