use std::{
  env, fmt,
  path::{Path, PathBuf},
  process::Stdio,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::header::HeaderValue;
use serde_json::{Map, Value, json};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  process::{Child, ChildStdin, ChildStdout, Command},
  sync::Notify,
  time::timeout,
};
use zeroize::{Zeroize, Zeroizing};

const PLUGIN_PROTOCOL_VERSION: &str = "2.0.0";
const MINIMUM_PLUGIN_PROTOCOL_VERSION: &str = "1.0.0";
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_GRACE: Duration = Duration::from_millis(250);
const MAX_MESSAGE_BYTES: u64 = 1 << 20;
const MAX_LOG_MESSAGE_BYTES: usize = 64 << 10;
const MAX_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;

/// Receives one bounded, provider-authored interactive message.
pub type CredentialProviderLogSink = fn(&str);

/// Credential-provider behavior carried through package operations.
#[derive(Clone, Debug, Default)]
pub(crate) struct CredentialProviderOptions {
  pub(crate) configured: bool,
  pub(crate) interactive: bool,
  pub(crate) cancellation: Option<PackageCancellation>,
  pub(crate) log_sink: Option<CredentialProviderLogSink>,
}

/// Stable credential-provider failure categories used by package diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialProviderErrorKind {
  Discovery,
  Protocol,
  Timeout,
  Process,
  Cancelled,
}

/// A secret-free provider failure.
#[derive(Debug)]
pub(crate) struct CredentialProviderError {
  kind: CredentialProviderErrorKind,
  context: String,
  message: String,
}

impl CredentialProviderError {
  pub(crate) fn kind(&self) -> CredentialProviderErrorKind {
    self.kind
  }

  pub(crate) fn context(&self) -> &str {
    &self.context
  }
}

impl fmt::Display for CredentialProviderError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl std::error::Error for CredentialProviderError {}

pub(crate) struct AcquiredCredential {
  pub(crate) authorization: HeaderValue,
  pub(crate) provider_index: usize,
}

pub(crate) async fn acquire(
  source: &str,
  options: &CredentialProviderOptions,
  is_retry: bool,
  preferred_provider: Option<usize>,
) -> Result<Option<AcquiredCredential>, CredentialProviderError> {
  let context = env::current_dir().map_err(|error| CredentialProviderError {
    kind: CredentialProviderErrorKind::Discovery,
    context: "credential providers".into(),
    message: format!("failed to read the working directory for credential-provider discovery: {error}"),
  })?;
  let output = options.log_sink.map(CredentialProviderOutput::new);
  let settings =
    CredentialProviderSettings::from_environment(&context, options.interactive, options.cancellation.clone(), output).map_err(CredentialProviderError::from)?;
  let Some(settings) = settings else {
    return Ok(None);
  };
  let credential = match settings.acquire(source, is_retry, preferred_provider).await {
    Ok(credential) => credential,
    Err(error) if error.kind() == ProviderErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(CredentialProviderError::from(error)),
  };
  let mut plaintext = Zeroizing::new(Vec::with_capacity(
    credential.username.len().saturating_add(credential.password.len()).saturating_add(1),
  ));
  plaintext.extend_from_slice(credential.username.as_bytes());
  plaintext.push(b':');
  plaintext.extend_from_slice(credential.password.as_bytes());
  let encoded = Zeroizing::new(BASE64.encode(&*plaintext));
  let mut header_bytes = Zeroizing::new(Vec::with_capacity(encoded.len().saturating_add(6)));
  header_bytes.extend_from_slice(b"Basic ");
  header_bytes.extend_from_slice(encoded.as_bytes());
  let mut header = HeaderValue::from_bytes(&header_bytes).map_err(|_| CredentialProviderError {
    kind: CredentialProviderErrorKind::Protocol,
    context: "credential provider".into(),
    message: "credential provider returned values which cannot form an HTTP Basic header".into(),
  })?;
  header.set_sensitive(true);
  Ok(Some(AcquiredCredential {
    authorization: header,
    provider_index: credential.provider_index,
  }))
}

pub(crate) fn is_configured() -> bool {
  env::var_os("NUGET_NETCORE_PLUGIN_PATHS")
    .or_else(|| env::var_os("NUGET_PLUGIN_PATHS"))
    .is_some_and(|paths| !paths.is_empty())
}

impl From<ProviderError> for CredentialProviderError {
  fn from(error: ProviderError) -> Self {
    let kind = match error.kind {
      ProviderErrorKind::Configuration | ProviderErrorKind::NotFound => CredentialProviderErrorKind::Discovery,
      ProviderErrorKind::Io => CredentialProviderErrorKind::Process,
      ProviderErrorKind::Protocol => CredentialProviderErrorKind::Protocol,
      ProviderErrorKind::Timeout => CredentialProviderErrorKind::Timeout,
      ProviderErrorKind::Cancelled | ProviderErrorKind::UserCancelled => CredentialProviderErrorKind::Cancelled,
    };
    Self {
      kind,
      context: error.context,
      message: error.message,
    }
  }
}

/// Command-lifetime cancellation observed by credential-provider processes.
#[derive(Clone, Debug)]
pub struct PackageCancellation {
  state: Arc<CancellationState>,
}

#[derive(Debug)]
struct CancellationState {
  cancelled: AtomicBool,
  notify: Notify,
}

impl PackageCancellation {
  /// Creates an unset cancellation handle.
  pub fn new() -> Self {
    Self {
      state: Arc::new(CancellationState {
        cancelled: AtomicBool::new(false),
        notify: Notify::new(),
      }),
    }
  }

  /// Requests cancellation and wakes provider protocol waits.
  pub fn cancel(&self) {
    self.state.cancelled.store(true, Ordering::Release);
    self.state.notify.notify_waiters();
  }

  pub(crate) fn is_cancelled(&self) -> bool {
    self.state.cancelled.load(Ordering::Acquire)
  }

  async fn cancelled(&self) {
    if self.is_cancelled() {
      return;
    }
    let notified = self.state.notify.notified();
    if self.is_cancelled() {
      return;
    }
    notified.await;
  }
}

impl Default for PackageCancellation {
  fn default() -> Self {
    Self::new()
  }
}

/// Live sink for provider device-flow or login instructions.
///
/// The callback is cold-path infrastructure and is invoked only for explicit
/// interactive provider requests. Provider processes are trusted not to place
/// returned credentials in protocol log messages.
#[derive(Clone)]
pub struct CredentialProviderOutput {
  callback: Arc<dyn Fn(&str) + Send + Sync>,
}

impl CredentialProviderOutput {
  /// Creates a thread-safe output callback.
  pub fn new(callback: impl Fn(&str) + Send + Sync + 'static) -> Self {
    Self { callback: Arc::new(callback) }
  }

  fn write(&self, message: &str) {
    (self.callback)(message);
  }
}

impl fmt::Debug for CredentialProviderOutput {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("CredentialProviderOutput(<redacted callback>)")
  }
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialProviderSettings {
  paths: Arc<[PathBuf]>,
  context: PathBuf,
  handshake_timeout: Duration,
  request_timeout: Duration,
  interactive: bool,
  cancellation: Option<PackageCancellation>,
  output: Option<CredentialProviderOutput>,
}

impl CredentialProviderSettings {
  pub(crate) fn from_environment(
    context: &Path,
    interactive: bool,
    cancellation: Option<PackageCancellation>,
    output: Option<CredentialProviderOutput>,
  ) -> Result<Option<Self>, ProviderError> {
    let configured = env::var_os("NUGET_NETCORE_PLUGIN_PATHS").or_else(|| env::var_os("NUGET_PLUGIN_PATHS"));
    let Some(configured) = configured else {
      return Ok(None);
    };
    let mut paths = Vec::new();
    for path in env::split_paths(&configured) {
      if path.as_os_str().is_empty() {
        continue;
      }
      if !path.is_absolute() {
        return Err(ProviderError::configuration(
          path.display().to_string(),
          "NuGet credential-provider paths must be absolute",
        ));
      }
      if !paths.contains(&path) {
        paths.push(path);
      }
    }
    if paths.is_empty() {
      return Ok(None);
    }
    Ok(Some(Self {
      paths: paths.into(),
      context: context.to_owned(),
      handshake_timeout: environment_timeout("NUGET_PLUGIN_HANDSHAKE_TIMEOUT_IN_SECONDS", DEFAULT_HANDSHAKE_TIMEOUT)?,
      request_timeout: environment_timeout("NUGET_PLUGIN_REQUEST_TIMEOUT_IN_SECONDS", DEFAULT_REQUEST_TIMEOUT)?,
      interactive,
      cancellation,
      output,
    }))
  }

  pub(crate) async fn acquire(&self, uri: &str, is_retry: bool, preferred_provider: Option<usize>) -> Result<ProviderCredential, ProviderError> {
    if self.cancellation.as_ref().is_some_and(PackageCancellation::is_cancelled) {
      return Err(ProviderError::cancelled("credential provider", "credential acquisition was cancelled"));
    }
    if let Some(index) = preferred_provider {
      return match self.run(index, uri, is_retry).await? {
        ProviderAttempt::Credential(credential) => Ok(credential),
        ProviderAttempt::NotApplicable => Err(ProviderError::not_found(
          self.paths[index].display().to_string(),
          "the previously selected credential provider no longer applies to this source",
        )),
        ProviderAttempt::UserCancelled => Err(ProviderError::user_cancelled(
          self.paths[index].display().to_string(),
          "credential acquisition was cancelled by the provider",
        )),
      };
    }
    let mut user_cancelled = None;
    for index in 0..self.paths.len() {
      match self.run(index, uri, is_retry).await? {
        ProviderAttempt::Credential(credential) => return Ok(credential),
        ProviderAttempt::NotApplicable => {},
        ProviderAttempt::UserCancelled => user_cancelled = Some(index),
      }
    }
    if let Some(index) = user_cancelled {
      Err(ProviderError::user_cancelled(
        self.paths[index].display().to_string(),
        if self.interactive {
          "credential acquisition was cancelled by the provider"
        } else {
          "the provider requires interactive authentication; rerun with --interactive"
        },
      ))
    } else {
      Err(ProviderError::not_found(
        "credential providers",
        "no configured NuGet credential provider supports this source",
      ))
    }
  }

  async fn run(&self, provider_index: usize, uri: &str, is_retry: bool) -> Result<ProviderAttempt, ProviderError> {
    let path = &self.paths[provider_index];
    let mut session = ProviderSession::start(path, &self.context, self.cancellation.clone(), self.output.clone()).await?;
    session.handshake(self.handshake_timeout).await?;
    let monitor = session
      .request("MonitorNuGetProcessExit", json!({"ProcessId": std::process::id()}), self.request_timeout)
      .await?;
    require_success(&monitor, path, "process-monitor request")?;
    let initialize = session
      .request(
        "Initialize",
        json!({
          "ClientVersion": env!("CARGO_PKG_VERSION"),
          "Culture": "en-US",
          "RequestTimeout": format_timespan(self.request_timeout),
        }),
        self.request_timeout,
      )
      .await?;
    require_success(&initialize, path, "initialization request")?;
    let claims = session
      .request(
        "GetOperationClaims",
        json!({"PackageSourceRepository": null, "ServiceIndex": null}),
        self.request_timeout,
      )
      .await?;
    if !payload_array_contains(&claims, "Claims", "Authentication")? {
      session.close().await;
      return Ok(ProviderAttempt::NotApplicable);
    }
    let log_level = if self.interactive { "Information" } else { "Minimal" };
    let log_level_response = session.request("SetLogLevel", json!({"LogLevel": log_level}), self.request_timeout).await?;
    if payload_string(&log_level_response, "ResponseCode")? != "Success" {
      session.close().await;
      return Err(ProviderError::protocol(
        path.display().to_string(),
        "credential provider rejected the log-level request",
      ));
    }
    let mut response = session
      .request(
        "GetAuthenticationCredentials",
        json!({
          "Uri": uri,
          "IsRetry": is_retry,
          "IsNonInteractive": !self.interactive,
          "CanShowDialog": self.interactive,
        }),
        self.request_timeout,
      )
      .await?;
    let response_code = payload_string(&response, "ResponseCode")?.to_owned();
    let outcome = match response_code.as_str() {
      "Success" => {
        let username = take_payload_secret(&mut response, "Username")?;
        let password = take_payload_secret(&mut response, "Password")?;
        if username.is_empty() && password.is_empty() {
          return Err(ProviderError::protocol(
            path.display().to_string(),
            "credential provider returned success without a username or password",
          ));
        }
        if let Some(types) = response.payload().get("AuthenticationTypes")
          && !types.is_null()
          && !types.as_array().is_some_and(|values| {
            values
              .iter()
              .any(|value| value.as_str().is_some_and(|value| value.eq_ignore_ascii_case("basic")))
          })
        {
          ProviderAttempt::NotApplicable
        } else {
          ProviderAttempt::Credential(ProviderCredential {
            username,
            password,
            provider_index,
          })
        }
      },
      "Error" => ProviderAttempt::NotApplicable,
      "NotFound" => ProviderAttempt::UserCancelled,
      _ => {
        return Err(ProviderError::protocol(
          path.display().to_string(),
          "credential provider returned an unknown authentication response code",
        ));
      },
    };
    session.close().await;
    Ok(outcome)
  }
}

fn environment_timeout(name: &str, default: Duration) -> Result<Duration, ProviderError> {
  let Some(value) = env::var_os(name) else {
    return Ok(default);
  };
  let value = value
    .to_str()
    .ok_or_else(|| ProviderError::configuration(name, format!("{name} must contain Unicode decimal seconds")))?;
  let seconds = value
    .parse::<u64>()
    .ok()
    .filter(|seconds| (1..=MAX_TIMEOUT_SECONDS).contains(seconds))
    .ok_or_else(|| ProviderError::configuration(name, format!("{name} must be between 1 and {MAX_TIMEOUT_SECONDS} seconds")))?;
  Ok(Duration::from_secs(seconds))
}

enum ProviderAttempt {
  Credential(ProviderCredential),
  NotApplicable,
  UserCancelled,
}

pub(crate) struct ProviderCredential {
  pub(crate) username: Zeroizing<String>,
  pub(crate) password: Zeroizing<String>,
  pub(crate) provider_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderErrorKind {
  Configuration,
  Io,
  Protocol,
  Timeout,
  Cancelled,
  UserCancelled,
  NotFound,
}

#[derive(Debug)]
pub(crate) struct ProviderError {
  kind: ProviderErrorKind,
  context: String,
  message: String,
}

impl ProviderError {
  fn configuration(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::Configuration, context, message)
  }

  fn io(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::Io, context, message)
  }

  fn protocol(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::Protocol, context, message)
  }

  fn timeout(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::Timeout, context, message)
  }

  fn cancelled(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::Cancelled, context, message)
  }

  fn user_cancelled(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::UserCancelled, context, message)
  }

  fn not_found(context: impl Into<String>, message: impl Into<String>) -> Self {
    Self::new(ProviderErrorKind::NotFound, context, message)
  }

  fn new(kind: ProviderErrorKind, context: impl Into<String>, message: impl Into<String>) -> Self {
    Self {
      kind,
      context: context.into(),
      message: message.into(),
    }
  }

  pub(crate) fn kind(&self) -> ProviderErrorKind {
    self.kind
  }
}

impl fmt::Display for ProviderError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl std::error::Error for ProviderError {}

struct ProviderSession {
  path: PathBuf,
  child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
  next_request_id: u32,
  cancellation: Option<PackageCancellation>,
  output: Option<CredentialProviderOutput>,
}

impl ProviderSession {
  async fn start(
    path: &Path,
    context: &Path,
    cancellation: Option<PackageCancellation>,
    output: Option<CredentialProviderOutput>,
  ) -> Result<Self, ProviderError> {
    if !path.is_file() {
      return Err(ProviderError::configuration(
        path.display().to_string(),
        "NuGet credential-provider path is not a file",
      ));
    }
    if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll")) {
      return Err(ProviderError::configuration(
        path.display().to_string(),
        format!(
          "credential provider requires a .NET host selected from {}; dv never invokes dotnet as a fallback, so use a self-contained nuget-plugin executable",
          context.display()
        ),
      ));
    }
    let mut command = Command::new(path);
    command
      .arg("-Plugin")
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true);
    #[cfg(windows)]
    {
      use std::os::windows::process::CommandExt as _;
      command.as_std_mut().creation_flags(0x0800_0000);
    }
    let mut child = command
      .spawn()
      .map_err(|error| ProviderError::io(path.display().to_string(), format!("failed to start credential provider: {error}")))?;
    let stdin = child
      .stdin
      .take()
      .ok_or_else(|| ProviderError::io(path.display().to_string(), "credential provider stdin was not piped"))?;
    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| ProviderError::io(path.display().to_string(), "credential provider stdout was not piped"))?;
    Ok(Self {
      path: path.to_owned(),
      child,
      stdin,
      stdout: BufReader::new(stdout),
      next_request_id: 1,
      cancellation,
      output,
    })
  }

  async fn handshake(&mut self, inactivity: Duration) -> Result<(), ProviderError> {
    let request_id = self.allocate_request_id();
    self
      .send(json!({
        "RequestId": request_id,
        "Type": "Request",
        "Method": "Handshake",
        "Payload": {"ProtocolVersion": PLUGIN_PROTOCOL_VERSION, "MinimumProtocolVersion": MINIMUM_PLUGIN_PROTOCOL_VERSION},
      }))
      .await?;
    let mut received_response = false;
    let mut sent_response = false;
    while !received_response || !sent_response {
      let incoming = self.next_message(inactivity, &request_id, "Handshake").await?;
      match (incoming.kind(), incoming.method()) {
        ("Request", "Handshake") => {
          let response = json!({
            "RequestId": incoming.request_id(),
            "Type": "Response",
            "Method": "Handshake",
            "Payload": {"ResponseCode": "Success", "ProtocolVersion": PLUGIN_PROTOCOL_VERSION},
          });
          self.send(response).await?;
          sent_response = true;
        },
        ("Response", "Handshake") if incoming.request_id() == request_id => {
          if payload_string(&incoming, "ResponseCode")? != "Success" || payload_string(&incoming, "ProtocolVersion")? != PLUGIN_PROTOCOL_VERSION {
            return Err(ProviderError::protocol(
              self.path.display().to_string(),
              "credential provider did not negotiate plugin protocol 2.0.0",
            ));
          }
          received_response = true;
        },
        _ => {
          return Err(ProviderError::protocol(
            self.path.display().to_string(),
            "credential provider sent an unexpected message during handshake",
          ));
        },
      }
    }
    Ok(())
  }

  async fn request(&mut self, method: &str, payload: Value, inactivity: Duration) -> Result<SensitiveMessage, ProviderError> {
    let request_id = self.allocate_request_id();
    self
      .send(json!({"RequestId": request_id, "Type": "Request", "Method": method, "Payload": payload}))
      .await?;
    loop {
      let incoming = self.next_message(inactivity, &request_id, method).await?;
      match incoming.kind() {
        "Progress" if incoming.request_id() == request_id && incoming.method() == method => {},
        "Response" if incoming.request_id() == request_id && incoming.method() == method => return Ok(incoming),
        "Fault" if incoming.request_id() == request_id => {
          return Err(ProviderError::protocol(
            self.path.display().to_string(),
            format!("credential provider faulted while handling {method}"),
          ));
        },
        "Cancel" if incoming.request_id() == request_id => {
          return Err(ProviderError::user_cancelled(
            self.path.display().to_string(),
            format!("credential provider cancelled {method}"),
          ));
        },
        "Request" if incoming.method() == "Log" => self.handle_log(&incoming).await?,
        "Request" if incoming.method() == "Handshake" => self.respond_to_handshake(&incoming).await?,
        _ => {
          return Err(ProviderError::protocol(
            self.path.display().to_string(),
            format!("credential provider sent an unexpected message while handling {method}"),
          ));
        },
      }
    }
  }

  async fn next_message(&mut self, inactivity: Duration, request_id: &str, method: &str) -> Result<SensitiveMessage, ProviderError> {
    let read = async {
      let mut bytes = Zeroizing::new(Vec::with_capacity(4096));
      let mut limited = (&mut self.stdout).take(MAX_MESSAGE_BYTES + 1);
      let count = limited
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|error| ProviderError::io(self.path.display().to_string(), format!("failed to read credential-provider output: {error}")))?;
      if count == 0 {
        return Err(ProviderError::protocol(
          self.path.display().to_string(),
          "credential provider exited before completing its response",
        ));
      }
      if count as u64 > MAX_MESSAGE_BYTES || !bytes.ends_with(b"\n") {
        return Err(ProviderError::protocol(
          self.path.display().to_string(),
          format!("credential-provider message exceeds {MAX_MESSAGE_BYTES} bytes"),
        ));
      }
      while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
      }
      SensitiveMessage::parse(&bytes, &self.path)
    };
    let boundary = async {
      match &self.cancellation {
        Some(cancellation) => {
          tokio::select! {
            result = timeout(inactivity, read) => result.map_err(|_| ProviderError::timeout(
              self.path.display().to_string(),
              format!("credential-provider {method} request timed out after {} seconds", inactivity.as_secs_f64()),
            ))?,
            _ = cancellation.cancelled() => Err(ProviderError::cancelled(
              self.path.display().to_string(),
              format!("credential-provider {method} request was cancelled"),
            )),
          }
        },
        None => timeout(inactivity, read).await.map_err(|_| {
          ProviderError::timeout(
            self.path.display().to_string(),
            format!("credential-provider {method} request timed out after {} seconds", inactivity.as_secs_f64()),
          )
        })?,
      }
    }
    .await;
    if boundary
      .as_ref()
      .is_err_and(|error| matches!(error.kind(), ProviderErrorKind::Timeout | ProviderErrorKind::Cancelled))
    {
      self.cancel_and_stop(request_id, method).await;
    }
    boundary
  }

  async fn handle_log(&mut self, incoming: &SensitiveMessage) -> Result<(), ProviderError> {
    let message = payload_string(incoming, "Message")?;
    if message.len() > MAX_LOG_MESSAGE_BYTES {
      return Err(ProviderError::protocol(
        self.path.display().to_string(),
        format!("credential-provider log message exceeds {MAX_LOG_MESSAGE_BYTES} bytes"),
      ));
    }
    if let Some(output) = &self.output {
      output.write(message);
    }
    self
      .send(json!({
        "RequestId": incoming.request_id(),
        "Type": "Response",
        "Method": "Log",
        "Payload": {"ResponseCode": "Success"},
      }))
      .await
  }

  async fn respond_to_handshake(&mut self, incoming: &SensitiveMessage) -> Result<(), ProviderError> {
    self
      .send(json!({
        "RequestId": incoming.request_id(),
        "Type": "Response",
        "Method": "Handshake",
        "Payload": {"ResponseCode": "Success", "ProtocolVersion": PLUGIN_PROTOCOL_VERSION},
      }))
      .await
  }

  async fn send(&mut self, message: Value) -> Result<(), ProviderError> {
    let mut bytes = Zeroizing::new(serde_json::to_vec(&message).map_err(|error| {
      ProviderError::protocol(
        self.path.display().to_string(),
        format!("failed to encode credential-provider request: {error}"),
      )
    })?);
    bytes.push(b'\n');
    self
      .stdin
      .write_all(&bytes)
      .await
      .map_err(|error| ProviderError::io(self.path.display().to_string(), format!("failed to write credential-provider request: {error}")))?;
    self
      .stdin
      .flush()
      .await
      .map_err(|error| ProviderError::io(self.path.display().to_string(), format!("failed to flush credential-provider request: {error}")))
  }

  async fn cancel_and_stop(&mut self, request_id: &str, method: &str) {
    let _ = self.send(json!({"RequestId": request_id, "Type": "Cancel", "Method": method})).await;
    let _ = timeout(CANCEL_GRACE, self.child.wait()).await;
    let _ = self.child.start_kill();
    let _ = self.child.wait().await;
  }

  async fn close(&mut self) {
    let request_id = self.allocate_request_id();
    let _ = self.send(json!({"RequestId": request_id, "Type": "Request", "Method": "Close"})).await;
    let _ = self.stdin.shutdown().await;
    if timeout(CLOSE_TIMEOUT, self.child.wait()).await.is_err() {
      let _ = self.child.start_kill();
      let _ = self.child.wait().await;
    }
  }

  fn allocate_request_id(&mut self) -> String {
    let request_id = self.next_request_id.to_string();
    self.next_request_id = self.next_request_id.saturating_add(1);
    request_id
  }
}

#[derive(Debug)]
struct SensitiveMessage {
  value: Value,
  request_id: String,
  kind: String,
  method: String,
}

impl SensitiveMessage {
  fn parse(bytes: &[u8], path: &Path) -> Result<Self, ProviderError> {
    let value: Value = serde_json::from_slice(bytes)
      .map_err(|error| ProviderError::protocol(path.display().to_string(), format!("credential provider emitted invalid JSON: {error}")))?;
    let request_id = object_string(&value, "RequestId", path)?.to_owned();
    let kind = object_string(&value, "Type", path)?.to_owned();
    let method = object_string(&value, "Method", path)?.to_owned();
    if !matches!(kind.as_str(), "Cancel" | "Fault" | "Progress" | "Request" | "Response") {
      return Err(ProviderError::protocol(
        path.display().to_string(),
        "credential provider emitted an unknown message type",
      ));
    }
    Ok(Self {
      value,
      request_id,
      kind,
      method,
    })
  }

  fn request_id(&self) -> &str {
    &self.request_id
  }

  fn kind(&self) -> &str {
    &self.kind
  }

  fn method(&self) -> &str {
    &self.method
  }

  fn payload(&self) -> &Map<String, Value> {
    match self.value.get("Payload").and_then(Value::as_object) {
      Some(payload) => payload,
      None => empty_object(),
    }
  }

  fn payload_mut(&mut self) -> Result<&mut Map<String, Value>, ProviderError> {
    self
      .value
      .get_mut("Payload")
      .and_then(Value::as_object_mut)
      .ok_or_else(|| ProviderError::protocol("credential provider", "credential-provider response omitted its object payload"))
  }
}

impl Drop for SensitiveMessage {
  fn drop(&mut self) {
    zeroize_json(&mut self.value);
  }
}

fn empty_object() -> &'static Map<String, Value> {
  static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
  EMPTY.get_or_init(Map::new)
}

fn object_string<'a>(value: &'a Value, key: &str, path: &Path) -> Result<&'a str, ProviderError> {
  value
    .get(key)
    .and_then(Value::as_str)
    .filter(|value| !value.is_empty())
    .ok_or_else(|| ProviderError::protocol(path.display().to_string(), format!("credential-provider message omitted {key}")))
}

fn payload_string<'a>(message: &'a SensitiveMessage, key: &str) -> Result<&'a str, ProviderError> {
  message
    .payload()
    .get(key)
    .and_then(Value::as_str)
    .ok_or_else(|| ProviderError::protocol("credential provider", format!("credential-provider response omitted {key}")))
}

fn payload_array_contains(message: &SensitiveMessage, key: &str, expected: &str) -> Result<bool, ProviderError> {
  let values = message
    .payload()
    .get(key)
    .and_then(Value::as_array)
    .ok_or_else(|| ProviderError::protocol("credential provider", format!("credential-provider response omitted {key}")))?;
  Ok(values.iter().any(|value| value.as_str() == Some(expected)))
}

fn require_success(message: &SensitiveMessage, path: &Path, operation: &str) -> Result<(), ProviderError> {
  if payload_string(message, "ResponseCode")? == "Success" {
    Ok(())
  } else {
    Err(ProviderError::protocol(
      path.display().to_string(),
      format!("credential provider rejected {operation}"),
    ))
  }
}

fn format_timespan(duration: Duration) -> String {
  let seconds = duration.as_secs();
  format!("{:02}:{:02}:{:02}", seconds / 3600, (seconds / 60) % 60, seconds % 60)
}

fn take_payload_secret(message: &mut SensitiveMessage, key: &str) -> Result<Zeroizing<String>, ProviderError> {
  let value = message.payload_mut()?.remove(key).unwrap_or(Value::Null);
  match value {
    Value::String(value) => Ok(Zeroizing::new(value)),
    Value::Null => Ok(Zeroizing::new(String::new())),
    mut value => {
      zeroize_json(&mut value);
      Err(ProviderError::protocol(
        "credential provider",
        format!("credential-provider response field {key} is not text"),
      ))
    },
  }
}

fn zeroize_json(value: &mut Value) {
  match value {
    Value::String(value) => value.zeroize(),
    Value::Array(values) => values.iter_mut().for_each(zeroize_json),
    Value::Object(values) => {
      let values = std::mem::take(values);
      for (mut key, mut value) in values {
        key.zeroize();
        zeroize_json(&mut value);
      }
    },
    Value::Null | Value::Bool(_) | Value::Number(_) => {},
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn cancellation_stops_before_provider_process_start() {
    let cancellation = PackageCancellation::new();
    cancellation.cancel();
    let settings = CredentialProviderSettings {
      paths: Arc::from([PathBuf::from("provider-must-not-start")]),
      context: PathBuf::from("."),
      handshake_timeout: Duration::from_secs(1),
      request_timeout: Duration::from_secs(1),
      interactive: false,
      cancellation: Some(cancellation),
      output: None,
    };

    let error = match settings.acquire("https://private.example.test/v3/index.json", false, None).await {
      Ok(_) => panic!("cancelled credential acquisition unexpectedly succeeded"),
      Err(error) => error,
    };

    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
  }

  #[tokio::test]
  async fn dll_only_provider_is_rejected_without_host_fallback() {
    let path = env::temp_dir().join(format!("dv-provider-test-{}.dll", std::process::id()));
    std::fs::write(&path, []).unwrap();

    let error = match ProviderSession::start(&path, Path::new("."), None, None).await {
      Ok(_) => panic!("DLL-only provider unexpectedly started"),
      Err(error) => error,
    };
    std::fs::remove_file(&path).unwrap();

    assert_eq!(error.kind(), ProviderErrorKind::Configuration);
    assert!(error.to_string().contains("never invokes dotnet"));
  }

  #[test]
  fn request_timeout_uses_nuget_timespan_shape() {
    assert_eq!(format_timespan(Duration::from_secs(30)), "00:00:30");
    assert_eq!(format_timespan(Duration::from_secs(3661)), "01:01:01");
  }

  #[test]
  fn credential_response_secrets_move_into_zeroizing_owners() {
    let mut message = SensitiveMessage::parse(
      br#"{"RequestId":"4","Type":"Response","Method":"GetAuthenticationCredentials","Payload":{"Username":"provider-user","Password":"provider-token","AuthenticationTypes":["Basic"],"ResponseCode":"Success"}}"#,
      Path::new("provider"),
    )
    .unwrap();

    let username = take_payload_secret(&mut message, "Username").unwrap();
    let password = take_payload_secret(&mut message, "Password").unwrap();

    assert_eq!(&*username, "provider-user");
    assert_eq!(&*password, "provider-token");
    assert!(!message.value.to_string().contains("provider-token"));
  }

  #[test]
  fn malformed_protocol_never_includes_provider_payload_in_errors() {
    let error = SensitiveMessage::parse(
      br#"{"RequestId":"1","Type":"Response","Method":"Handshake","Payload":{"Password":"must-not-leak"}} trailing"#,
      Path::new("provider"),
    )
    .unwrap_err();

    assert!(!error.to_string().contains("must-not-leak"));
  }
}
