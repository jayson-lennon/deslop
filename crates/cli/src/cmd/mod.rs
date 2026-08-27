//! Subcommand implementations.

pub mod fix_cmd;
pub mod init_cmd;
pub mod lint_cmd;
pub mod rules_cmd;

/// Command execution failure (already reported to stderr).
#[derive(Debug, wherror::Error)]
#[error("{0}")]
pub struct CmdError(pub String);

/// Shorthand: fail with a formatted message.
pub(crate) fn fail(msg: impl std::fmt::Display) -> error_stack::Report<CmdError> {
    error_stack::Report::new(CmdError(msg.to_string()))
}
