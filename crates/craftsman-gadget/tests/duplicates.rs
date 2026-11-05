use craftsman_gadget::{CallError, GadgetError, gadget_call};

#[gadget_call(name = "same", description = "First definition.")]
fn first() -> Result<u32, GadgetError> {
    Ok(1)
}

#[gadget_call(name = "same", description = "Second definition.")]
fn second() -> Result<u32, GadgetError> {
    Ok(2)
}

#[test]
fn duplicate_names_never_dispatch_an_ambiguous_function() {
    assert!(
        craftsman_gadget::describe()
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    assert!(
        matches!(craftsman_gadget::call("same", "{}"), Err(CallError::Failed(s)) if s.contains("duplicate"))
    );
}
