//! Offline guards: synthetic env setup belongs to child plans, not running
//! process globals. Runtime/data/protocol changes are not implied by this test.

const SOURCES: &[(&str, &str)] = &[
    ("config", include_str!("../src/config.rs")),
    ("mcp", include_str!("../src/mcp.rs")),
    ("route", include_str!("../src/route.rs")),
    ("serve", include_str!("../src/serve.rs")),
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
fn router_secret_fixture_and_environment_helper_are_absent_from_normal_frontend_modules() {
    let main = normalized_source(include_str!("../src/main.rs"));
    assert!(main.contains("#[cfg(test)]\nmod test_environment;"));
    let serve = normalized_source(include_str!("../src/serve.rs"));
    assert!(serve.contains("#[cfg(test)]\n    proxy_fixture:"));
    assert!(serve.contains("#[cfg(test)]\n        proxy_fixture: None"));
    assert!(serve.contains("authorized_from_secret(headers, std::env::var_os(PROXY_SECRET_ENV))"));
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
