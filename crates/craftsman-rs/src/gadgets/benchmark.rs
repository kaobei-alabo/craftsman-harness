//! Developer-only measurements, not model-quality gates. This is nested in the
//! test module so it reuses the same isolated home and source-built fixtures.

use super::super::{GadgetCatalogMiddleware, authoring, catalogue};
use super::{Capabilities, TestHome, fixtures};
use craftsman_harness::{AgentMiddleware, AgentRegistry, AgentState, Tool, ToolCtx};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Summary {
    samples: usize,
    min_ns: u64,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    max_ns: u64,
    mean_ns: u64,
}
fn summarize(samples: &[Duration]) -> Summary {
    assert!(!samples.is_empty(), "a distribution needs samples");
    let mut values: Vec<u64> = samples.iter().map(|d| nanos(*d)).collect();
    values.sort_unstable();
    let percentile = |p: usize| values[(values.len() * p).div_ceil(100) - 1];
    let total: u128 = values.iter().map(|n| u128::from(*n)).sum();
    Summary {
        samples: values.len(),
        min_ns: values[0],
        p50_ns: percentile(50),
        p95_ns: percentile(95),
        p99_ns: percentile(99),
        max_ns: *values.last().unwrap(),
        mean_ns: u64::try_from(total / values.len() as u128).unwrap(),
    }
}
fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).expect("measurement duration fits u64")
}
fn setting(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    let value =
        std::env::var(name).map_or(default, |s| s.parse().expect("integer benchmark setting"));
    assert!(
        (minimum..=maximum).contains(&value),
        "benchmark setting out of bounds: {name}"
    );
    value
}
fn require_release(debug_assertions: bool) {
    // Keep ordinary debug test compilation valid; reject only execution of
    // this explicitly selected, ignored measurement in a debug build.
    assert!(!debug_assertions, "use the release-mode benchmark runner");
}

async fn dispatch(tool: &Arc<dyn Tool>, context: &mut ToolCtx<'_>) -> Duration {
    let started = Instant::now();
    let result = tool
        .call(
            json!({"gadget":"demo","function":"ping","args":{}}),
            context,
        )
        .await;
    let elapsed = started.elapsed();
    let outcome = result.expect("dispatcher succeeded");
    assert!(
        !outcome.is_error,
        "{}",
        super::super::cli::terminal_safe(&outcome.content)
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&outcome.content).unwrap(),
        json!({"pong":true})
    );
    elapsed // include dispatch, rescan, validation and fresh guest instantiation
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "informational release-mode benchmark; run scripts/benchmark-gadgets.py"]
async fn measure_dispatch_and_catalog() {
    require_release(cfg!(debug_assertions));
    let samples = setting("CRAFTSMAN_GADGET_BENCH_SAMPLES", 500, 20, 10_000);
    let warmups = setting("CRAFTSMAN_GADGET_BENCH_WARMUPS", 20, 1, 1_000);
    let fixture_started = Instant::now();
    let home = TestHome::new(Capabilities::default());
    let component_sha256 = format!("{:x}", Sha256::digest(&fixtures().0));
    let fixture_setup_ns = nanos(fixture_started.elapsed());
    let runtime_started = Instant::now();
    let runtime = home.runtime(home.options()); // explicit pure grant, disk cache off
    let runtime_init_ns = nanos(runtime_started.elapsed());
    let definitions = runtime.tools().iter().map(|t| t.def()).collect::<Vec<_>>();
    let tool = runtime
        .tools()
        .into_iter()
        .find(|t| t.def().name == "gadget_call")
        .unwrap();
    let mut state = AgentState::default();
    let (notifications, _receiver) = craftsman_harness::notification_channel();
    let mut context = ToolCtx {
        state: &mut state,
        backend: Arc::new(crate::backend_fs::RealFs::new(home.workspace.clone())),
        registry: Arc::new(AgentRegistry::default()),
        notifications,
    };
    let cold_dispatch_ns = nanos(dispatch(&tool, &mut context).await);
    for _ in 0..warmups {
        dispatch(&tool, &mut context).await;
    }
    let mut calls = Vec::with_capacity(samples);
    for _ in 0..samples {
        calls.push(dispatch(&tool, &mut context).await);
    }

    let middleware = GadgetCatalogMiddleware::new(runtime.clone());
    let mut catalog_state = AgentState::default();
    let catalog_started = Instant::now();
    middleware.prepare_model(&mut catalog_state).await.unwrap();
    let initial_catalog_ns = nanos(catalog_started.elapsed());
    assert_eq!(catalogue::notifications(&catalog_state), 1);
    for _ in 0..warmups {
        middleware.prepare_model(&mut catalog_state).await.unwrap();
    }
    let mut refreshes = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        middleware.prepare_model(&mut catalog_state).await.unwrap();
        refreshes.push(started.elapsed());
    }
    assert_eq!(
        catalogue::notifications(&catalog_state),
        1,
        "unchanged rounds add no notices"
    );
    assert_eq!(
        runtime.compiled_count(),
        1,
        "warm calls reuse the compiled component"
    );
    assert_eq!(
        definitions,
        runtime.tools().iter().map(|t| t.def()).collect::<Vec<_>>()
    );
    assert!(
        !home.config.join("gadgets/.grants.json").exists(),
        "invocation grant must remain ephemeral"
    );

    let report = json!({
        "schema":"craftsman-harness/gadget-benchmark@1", "scope":"runtime_microbenchmark",
        "crate_version":env!("CARGO_PKG_VERSION"), "rust_toolchain":authoring::pinned_toolchain(),
        "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
        "debug_assertions":cfg!(debug_assertions), "native_code_disk_cache":false,
        "samples":samples, "warmups":warmups, "component_sha256":component_sha256,
        "benchmark_binary_sha256":format!("{:x}", Sha256::digest(std::fs::read(std::env::current_exe().unwrap()).unwrap())),
        "model_requests":0, "gadget_list_calls":0, "quality_gate_cleared":false,
        "fixture_setup_ns":fixture_setup_ns, "runtime_init_ns":runtime_init_ns,
        "cold_dispatch_ns":cold_dispatch_ns, "initial_catalog_ns":initial_catalog_ns,
        "warm_dispatch":summarize(&calls), "warm_catalog":summarize(&refreshes),
        "unchanged_catalog_notices_added":0, "tool_definitions_changed":false,
        "compiled_components":runtime.compiled_count(),
    });
    let bytes = serde_json::to_vec(&report).unwrap();
    match std::env::var_os("CRAFTSMAN_GADGET_BENCH_OUTPUT") {
        Some(path) => {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path).expect("create-only benchmark output");
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
        }
        None => println!("{}", String::from_utf8(bytes).unwrap()), // controlled test data only
    }
}

#[test]
fn percentiles_use_nearest_rank_and_sort_unsorted_input() {
    let values: Vec<_> = (1..=20).rev().map(Duration::from_nanos).collect();
    assert_eq!(
        summarize(&values),
        Summary {
            samples: 20,
            min_ns: 1,
            p50_ns: 10,
            p95_ns: 19,
            p99_ns: 20,
            max_ns: 20,
            mean_ns: 10
        }
    );
}
#[test]
fn distribution_handles_one_sample_without_overflowing_the_sum() {
    let sample = Duration::from_nanos(u64::MAX);
    assert_eq!(summarize(&[sample, sample]).mean_ns, u64::MAX);
    assert_eq!(summarize(&[Duration::ZERO]).p95_ns, 0);
}
#[test]
#[should_panic(expected = "a distribution needs samples")]
fn empty_distribution_is_not_reported_as_a_successful_measurement() {
    summarize(&[]);
}
