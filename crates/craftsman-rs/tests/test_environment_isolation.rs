//! Offline guards: synthetic env setup belongs to child plans, not running
//! process globals. Runtime/data/protocol changes are not implied by this test.

const SOURCES: &[(&str, &str)] = &[
    ("config", include_str!("../src/config.rs")),
    ("mcp", include_str!("../src/mcp.rs")),
    ("route", include_str!("../src/route.rs")),
    ("bridge_support", include_str!("../src/bridge_support.rs")),
    (
        "test_environment",
        include_str!("../src/test_environment.rs"),
    ),
];

#[test]
fn env_tests_have_no_process_global_set_or_remove_calls() {
    for (name, source) in SOURCES {
        for function in ["set_var", "remove_var"] {
            assert!(
                !source.contains(&format!("std::env::{function}(")),
                "global env mutation in {name}"
            );
        }
    }
}

#[test]
fn native_environment_helper_is_marker_gated_before_environment_or_original_assertions() {
    let source = include_str!("../src/test_environment.rs");
    let body = source
        .split("fn native_environment_fixture() {")
        .nth(1)
        .unwrap();
    assert!(
        body.find("selected(std::env::args())").unwrap() < body.find("case.configured()").unwrap()
    );
    assert!(body.find("case.configured()").unwrap() < body.find("case.assertions()").unwrap());
    assert!(body.find("case.assertions()").unwrap() < body.find("case.completion()").unwrap());
    assert!(source.contains("command.envs(set.iter().copied())"));
    assert!(source.contains("command.env_remove(name)"));
}

#[test]
fn proxy_environment_helper_is_cfgtest_only_and_auth_reads_the_original_environment() {
    let main = normalized_source(include_str!("../src/main.rs"));
    assert!(main.contains("#[cfg(test)]\nmod test_environment;"));
    let support = normalized_source(include_str!("../src/bridge_support.rs"));
    assert!(support.contains("#[cfg(test)]\npub(crate) fn proxy_environment_fixture_for_tests"));
    assert!(
        support.contains("authorized_from_secret(headers, std::env::var_os(PROXY_SECRET_ENV))")
    );
}

fn normalized_source(source: &str) -> String {
    source.replace("\r\n", "\n")
}

#[test]
fn declaration_guard_normalizes_only_lf_and_crlf_not_other_declaration_bytes() {
    let exact = "#[cfg(test)]\nmod test_environment;";
    assert!(normalized_source(exact).contains(exact));
    assert!(normalized_source("#[cfg(test)]\r\nmod test_environment;").contains(exact));
    for source in [
        "#[cfg(test)]\rmod test_environment;",
        "#[cfg(not(test))]\r\nmod test_environment;",
        "#[cfg(test)]\r\nmod different_environment;",
        "#[cfg(test)]\r\n\tmod test_environment;",
    ] {
        assert!(!normalized_source(source).contains(exact));
    }
}
