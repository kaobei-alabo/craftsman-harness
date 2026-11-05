//! TEST-only immutable child environments for the original env assertions.
//! Never set/remove the running process environment, inspect real stdin/config,
//! or treat an unmarked/unknown helper's no-op return as fixture execution.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const HELPER: &str = "test_environment::tests::native_environment_fixture";
const MARKER: &str = "__craftsman_environment_fixture__:";
const OUTPUT_LIMIT: u64 = 64 * 1024;
const EXECUTION_LIMIT: Duration = Duration::from_secs(20);
const CLEANUP_LIMIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(crate) enum Case {
    Config,
    McpHeader,
    McpStdio,
    Route,
    ProxyPresent,
    ProxyAbsent,
    ProxyEmpty,
}
impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::McpHeader => "mcp-header",
            Self::McpStdio => "mcp-stdio",
            Self::Route => "route",
            Self::ProxyPresent => "proxy-present",
            Self::ProxyAbsent => "proxy-absent",
            Self::ProxyEmpty => "proxy-empty",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "config" => Self::Config,
            "mcp-header" => Self::McpHeader,
            "mcp-stdio" => Self::McpStdio,
            "route" => Self::Route,
            "proxy-present" => Self::ProxyPresent,
            "proxy-absent" => Self::ProxyAbsent,
            "proxy-empty" => Self::ProxyEmpty,
            _ => return None,
        })
    }
    fn environment(
        self,
    ) -> (
        &'static [(&'static str, &'static str)],
        &'static [&'static str],
    ) {
        match self {
            Self::Config => (
                &[("CRAFTSMAN_TEST_TOKEN", "tok-123")],
                &["CRAFTSMAN_TEST_MISSING_9"],
            ),
            Self::McpHeader => (
                &[("CRAFTSMAN_MCP_TEST_TOKEN", "tok-7")],
                &["CRAFTSMAN_MCP_MISSING_9"],
            ),
            Self::McpStdio => (&[("CRAFTSMAN_STDIO_TEST_KEY", "sk-9")], &[]),
            Self::Route => (&[("CRAFTSMAN_ROUTE_TEST_KEY", "tok-1")], &[]),
            Self::ProxyPresent => (&[(crate::serve::PROXY_SECRET_ENV, "s3cret")], &[]),
            Self::ProxyAbsent => (&[], &[crate::serve::PROXY_SECRET_ENV]),
            Self::ProxyEmpty => (&[(crate::serve::PROXY_SECRET_ENV, "")], &[]),
        }
    }
    fn configured(self) -> bool {
        let (set, absent) = self.environment();
        set.iter()
            .all(|(name, expected)| std::env::var(name).as_deref() == Ok(*expected))
            && absent.iter().all(|name| std::env::var_os(name).is_none())
    }
    fn completion(self) -> String {
        format!("CRAFTSMAN_ENV_FIXTURE_DONE:{}", self.name())
    }
    fn assertions(self) {
        match self {
            Self::Config => crate::config::config_environment_fixture_for_tests(),
            Self::McpHeader => crate::mcp::mcp_header_environment_fixture_for_tests(),
            Self::McpStdio => crate::mcp::mcp_stdio_environment_fixture_for_tests(),
            Self::Route => {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("isolated route fixture runtime")
                    .block_on(crate::route::route_environment_fixture_for_tests());
            }
            Self::ProxyPresent => crate::serve::proxy_environment_fixture_for_tests(true),
            Self::ProxyAbsent | Self::ProxyEmpty => {
                crate::serve::proxy_environment_fixture_for_tests(false)
            }
        }
    }
}

fn selected(arguments: impl Iterator<Item = String>) -> Option<Case> {
    let mut markers =
        arguments.filter_map(|argument| argument.strip_prefix(MARKER).map(str::to_owned));
    let name = markers.next()?;
    if markers.next().is_some() {
        return None;
    }
    Case::from_name(&name)
}

pub(crate) fn run(case: Case) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("isolated environment fixture runtime")
        .block_on(run_async(case));
}
pub(crate) async fn run_async(case: Case) {
    let executable = std::env::current_exe().expect("native environment fixture executable");
    let mut command = tokio::process::Command::new(executable);
    command.args([
        "--ignored",
        "--exact",
        HELPER,
        "--test-threads=1",
        "--nocapture",
        &format!("{MARKER}{}", case.name()),
    ]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let (set, absent) = case.environment();
    command.envs(set.iter().copied());
    for name in absent {
        command.env_remove(name);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|_| panic!("native environment fixture spawn failed"));
    let stdout = child.stdout.take().expect("fixture stdout piped");
    let stderr = child.stderr.take().expect("fixture stderr piped");
    let outcome = tokio::time::timeout(EXECUTION_LIMIT, async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_pipe = stdout.take(OUTPUT_LIMIT + 1);
        let mut err_pipe = stderr.take(OUTPUT_LIMIT + 1);
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            out_pipe.read_to_end(&mut out),
            err_pipe.read_to_end(&mut err),
        )?;
        Ok::<_, std::io::Error>((status, out, err))
    })
    .await;
    let (status, stdout, stderr) = match outcome {
        Ok(Ok(result)) => result,
        _ => {
            let _ = tokio::time::timeout(CLEANUP_LIMIT, child.kill()).await;
            panic!("native environment fixture capture/exit failed within its bounded test limit");
        }
    };
    assert!(
        stdout.len() as u64 <= OUTPUT_LIMIT && stderr.len() as u64 <= OUTPUT_LIMIT,
        "whole environment fixture output overbound"
    );
    let stdout = std::str::from_utf8(&stdout).expect("strict fixture stdout UTF-8");
    let stderr = std::str::from_utf8(&stderr).expect("strict fixture stderr UTF-8");
    assert!(
        status.success(),
        "original isolated environment assertions failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert_eq!(
        stderr
            .lines()
            .filter(|line| *line == case.completion())
            .count(),
        1,
        "native fixture must prove the actual original assertions completed, not an unmarked no-op"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "native subprocess only; complete argv marker required BEFORE environment/stdin/config inspection"]
    fn native_environment_fixture() {
        let Some(case) = selected(std::env::args()) else {
            return;
        };
        assert!(
            case.configured(),
            "synthetic fixture environment was not configured before process startup"
        );
        case.assertions();
        eprintln!("\n{}\n", case.completion());
    }

    #[test]
    fn absent_unknown_duplicate_markers_refuse_before_any_environment_case_selection() {
        for arguments in [
            vec!["helper"],
            vec!["helper", "unmarked"],
            vec!["helper", "__craftsman_environment_fixture__:unknown"],
            vec![
                "__craftsman_environment_fixture__:config",
                "__craftsman_environment_fixture__:config",
            ],
        ] {
            assert!(selected(arguments.into_iter().map(str::to_owned)).is_none());
        }
    }

    #[test]
    fn registered_markers_select_only_the_same_fixed_child_environment_plan() {
        for name in [
            "config",
            "mcp-header",
            "mcp-stdio",
            "route",
            "proxy-present",
            "proxy-absent",
            "proxy-empty",
        ] {
            let case = selected([format!("{MARKER}{name}")].into_iter()).unwrap();
            assert_eq!(case.name(), name);
            assert!(case.environment().0.len() <= 1 && case.environment().1.len() <= 1);
        }
    }

    #[test]
    fn immutable_child_env_preserves_real_production_proxy_source_and_header_gate() {
        for case in [Case::ProxyPresent, Case::ProxyAbsent, Case::ProxyEmpty] {
            run(case);
        }
    }
}
