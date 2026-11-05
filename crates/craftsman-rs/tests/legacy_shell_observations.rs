//! Source guards complement, but never replace, actual native phase evidence.

#[test]
fn legacy_runner_observation_interfaces_are_cfgtest_only() {
    let tools = include_str!("../src/tools.rs").replace("\r\n", "\n");
    assert!(tools.contains("#[cfg(test)]\nmod observations;"));
    assert!(tools.contains("#[cfg(test)]\n    legacy_observations:"));
    assert!(tools.contains("#[cfg(test)]\n            legacy_observations: None"));
    assert!(tools.contains("#[cfg(test)]\nimpl Drop for ShellRunner"));
    assert!(tools.contains("#[cfg(test)] owner: Option<&observations::Owner>"));
    assert!(tools.contains("let deadline = started + limit;"));
    assert!(tools.contains("const DRAIN_AFTER_KILL: Duration = Duration::from_secs(2);"));
    assert!(tools.contains("if std::thread::panicking()"));
    let observer_source = include_str!("../src/tools/observations.rs").replace("\r\n", "\n");
    let observer = observer_source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .unwrap();
    for forbidden in [
        "Context::from_waker",
        "wake_by_ref",
        "std::env::",
        "thread::spawn",
        "ProcessTree::",
        "derive(Debug",
        "Serialize",
        "ToolMeta",
        "proc_pid",
        "OpenProcess",
    ] {
        assert!(
            !observer.contains(forbidden),
            "unexpected observer channel: {forbidden}"
        );
    }
    assert!(observer.contains("Pin::new(&mut this.inner).poll_read(cx, buffer)"));
    assert!(observer.contains("const MAX_CALLS: usize = 8;"));
    assert!(observer.contains("const MAX_REPORT_BYTES: usize = 4096;"));
}
