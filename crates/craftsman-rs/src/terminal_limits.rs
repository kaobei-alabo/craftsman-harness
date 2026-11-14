//! Explicit terminal turn budgets; other frontends retain their own policies.
use crate::config::Config;

pub(crate) const DEFAULT_MAX_ROUNDS: usize = 200;
const MAX_ROUNDS: u64 = 1_000;

pub(crate) fn max_rounds(cfg: &Config) -> Result<usize, &'static str> {
    let Some(terminal) = cfg.extra.get("terminal") else {
        return Ok(DEFAULT_MAX_ROUNDS);
    };
    let terminal = terminal
        .as_object()
        .ok_or("Invalid terminal settings: terminal must be an object.")?;
    let Some(value) = terminal.get("max_rounds") else {
        return Ok(DEFAULT_MAX_ROUNDS);
    };
    let rounds = value
        .as_u64()
        .filter(|rounds| (1..=MAX_ROUNDS).contains(rounds))
        .ok_or("Invalid terminal.max_rounds: use a whole number from 1 to 1000.")?;
    Ok(rounds as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(terminal: serde_json::Value) -> Config {
        let mut cfg = Config::default();
        cfg.extra.insert("terminal".into(), terminal);
        cfg
    }

    #[test]
    fn terminal_round_budget_accepts_defaults_and_positive_bounded_integers() {
        assert_eq!(max_rounds(&Config::default()), Ok(200));
        assert_eq!(
            max_rounds(&config(json!({"theme":"user-setting"}))),
            Ok(200)
        );
        for n in [1, 50, 200, 1000] {
            assert_eq!(max_rounds(&config(json!({"max_rounds":n}))), Ok(n));
        }
    }

    #[test]
    fn invalid_terminal_round_budgets_refuse_without_fallback_or_value_echo() {
        for value in [
            json!(0),
            json!(-1),
            json!(1001),
            json!(u64::MAX),
            json!(1.5),
            json!("synthetic-secret"),
            json!(true),
            json!(null),
        ] {
            let error = max_rounds(&config(json!({"max_rounds":value}))).unwrap_err();
            assert_eq!(
                error,
                "Invalid terminal.max_rounds: use a whole number from 1 to 1000."
            );
        }
        for value in [json!(null), json!([]), json!("synthetic-secret")] {
            assert_eq!(
                max_rounds(&config(value)),
                Err("Invalid terminal settings: terminal must be an object.")
            );
        }
    }
}
