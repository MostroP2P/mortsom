//! Reaching the actor's CLI from a device context, and turning CLI failures
//! into adapter failures with their categories.

use mortsom_app_contract::{AdapterError, DeviceContext};
use mortsom_cli_driver::{Call, CliClient, CliError, CliSession, Envelope};

/// The CLI client behind `device`'s session.
pub(crate) fn client<'a>(device: &'a DeviceContext<'_>) -> Result<&'a CliClient, AdapterError> {
    device
        .ui
        .command_line()
        .and_then(|any| any.downcast_ref::<CliSession>())
        .map(|session| session.client().as_ref())
        .ok_or_else(|| {
            AdapterError::environment(
                "profile `cli` needs a mostro-cli session, and this session drives a screen",
            )
        })
}

/// A CLI failure as an adapter failure, keeping its category.
pub(crate) fn adapter_error(error: CliError) -> AdapterError {
    AdapterError::new(error.category(), error.to_string())
}

/// Runs `call` as `device`'s actor; any failure becomes an adapter failure.
pub(crate) async fn run(device: &DeviceContext<'_>, call: Call) -> Result<Envelope, AdapterError> {
    client(device)?.invoke(call).await.map_err(adapter_error)
}

/// Runs `call` and keeps the CLI's own failure, for callers that decide on
/// its code.
pub(crate) async fn try_run(
    device: &DeviceContext<'_>,
    call: Call,
) -> Result<Result<Envelope, CliError>, AdapterError> {
    Ok(client(device)?.invoke(call).await)
}

/// An outcome lacking a field the contract promises.
pub(crate) fn missing(command: &str, field: &str) -> AdapterError {
    AdapterError::contract(format!("`{command}` outcome carries no `{field}`"))
}
