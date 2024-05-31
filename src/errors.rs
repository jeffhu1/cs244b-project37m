// Errors for scheduler. from aptos repo in the delayed_fields.rs
// Represents something that should never happen - i.e. a code invariant error,
// which we would generally just panic, but since we are inside of the VM,
// we cannot do that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanicError {
    CodeInvariantError(String),
}

pub fn code_invariant_error<M: std::fmt::Debug>(message: M) -> PanicError {
    let msg = format!(
        "Delayed materialization code invariant broken (there is a bug in the code), {:?}",
        message
    );
    // TODO: deleted some logging thing
    PanicError::CodeInvariantError(msg)
}
