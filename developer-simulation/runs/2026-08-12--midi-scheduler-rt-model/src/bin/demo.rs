use std::process::Command;
use std::time::Instant;

use midi_scheduler_rt_model::{
    Discontinuity, Event, EventKind, FixedHandoff, HostBlock, PlanSpec, ScheduledEvent, Scheduler,
    TempoNode, prepare_plan,
};

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(250_000);
    let plan = prepare_plan(demo_spec()).expect("static demo fixture must validate");
    let mut handoff = FixedHandoff::new();
    let published = handoff.publish(plan).expect("first handoff slot is free");
    let ticket = handoff.begin_callback().expect("published callback ticket");
    let mut scheduler = Scheduler::new(64);
    let mut output = [ScheduledEvent::EMPTY; 32];

    let (first_written, mut latencies, event_digest) = handoff
        .with_plan(ticket, |published_plan| {
            let first = scheduler.schedule(
                published_plan,
                HostBlock {
                    token: 1,
                    start_frame: 0,
                    length: 16,
                    sample_rate: 48_000,
                    discontinuity: Discontinuity::None,
                },
                &mut output,
            );
            let first_written = first.written;
            for warmup in 0_u64..10_000 {
                let _ = scheduler.schedule(
                    published_plan,
                    HostBlock {
                        token: warmup + 2,
                        start_frame: i64::try_from(warmup % 240).unwrap_or_default(),
                        length: 16,
                        sample_rate: 48_000,
                        discontinuity: Discontinuity::None,
                    },
                    &mut output,
                );
            }

            let mut latencies = Vec::with_capacity(iterations);
            let mut event_digest = 0xcbf2_9ce4_8422_2325_u64;
            for iteration in 0..iterations {
                let started = Instant::now();
                let result = scheduler.schedule(
                    published_plan,
                    HostBlock {
                        token: iteration as u64 + 20_000,
                        start_frame: i64::try_from(iteration % 240).unwrap_or_default(),
                        length: 16,
                        sample_rate: 48_000,
                        discontinuity: Discontinuity::None,
                    },
                    &mut output,
                );
                let elapsed = started.elapsed().as_nanos();
                latencies.push(u64::try_from(elapsed).unwrap_or(u64::MAX));
                for event in &output[..result.written] {
                    event_digest ^= event.stable_id;
                    event_digest = event_digest.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            (first_written, latencies, event_digest)
        })
        .expect("ticket remains pinned");
    handoff.end_callback(ticket).expect("ticket ends once");

    latencies.sort_unstable();
    let p99_index = iterations.saturating_mul(99).saturating_div(100);
    let p99_ns = latencies
        .get(p99_index.min(latencies.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0);
    let max_ns = latencies.last().copied().unwrap_or(0);
    let histogram = histogram(&latencies);
    let binary_digest = executable_digest().unwrap_or(0);

    println!("scheduler-demo");
    println!("first_block_events={first_written}");
    println!("published_generation={}", published.generation);
    println!("published_plan_digest={:016x}", published.digest);
    println!("event_log_digest={event_digest:016x}");
    println!("bounded_release_iterations={iterations}");
    println!("p99_ns={p99_ns}");
    println!("max_ns={max_ns}");
    println!("histogram_ns_le_1000_2000_5000_10000_20000_50000_100000_over={histogram:?}");
    println!("binary_fnv1a64={binary_digest:016x}");
    println!("compiler_profile=release lto=thin codegen_units=1");
    println!("cpu_model={}", cpu_model());
    println!("core_affinity=not_pinned");
    println!("background_load_policy=uncontrolled_interactive_host");
}

fn demo_spec() -> PlanSpec {
    let events = (0_i64..256)
        .map(|pulse| Event {
            pulse,
            stable_id: pulse.cast_unsigned() + 1,
            kind: EventKind::ControlChange {
                channel: u8::try_from(pulse % 16).expect("fixture channel fits"),
                controller: 1,
                value: u8::try_from(pulse % 128).expect("fixture value fits"),
            },
        })
        .collect();
    PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 20_000,
        }],
        events,
    }
}

fn histogram(samples: &[u64]) -> [u64; 8] {
    let limits = [1_000_u64, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000];
    let mut bins = [0_u64; 8];
    for &sample in samples {
        let bin = limits
            .iter()
            .position(|limit| sample <= *limit)
            .unwrap_or(limits.len());
        bins[bin] += 1;
    }
    bins
}

fn executable_digest() -> Option<u64> {
    let path = std::env::current_exe().ok()?;
    let bytes = std::fs::read(path).ok()?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(hash)
}

fn cpu_model() -> String {
    Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| format!("{}-unknown-model", std::env::consts::ARCH))
}
