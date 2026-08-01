use dv_core::CancellationToken;

/// Installs the one process handler before any workspace or SDK work begins.
pub(crate) fn install() -> Result<CancellationToken, String> {
  let token = CancellationToken::new();
  let signal = token.clone();
  ctrlc::set_handler(move || signal.request()).map_err(|error| format!("failed to install Ctrl+C/SIGINT cancellation: {error}"))?;
  Ok(token)
}
