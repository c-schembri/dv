use std::{
  env,
  fs::OpenOptions,
  io::{self, BufRead, Write},
  process::ExitCode,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const PLUGIN_HANDSHAKE_ID: &str = "10000000-0000-0000-0000-000000000001";
const PLUGIN_LOG_ID: &str = "10000000-0000-0000-0000-000000000002";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Incoming {
  request_id: String,
  #[serde(rename = "Type")]
  message_type: String,
  method: String,
  #[serde(default)]
  payload: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Outgoing<'a> {
  request_id: &'a str,
  #[serde(rename = "Type")]
  message_type: &'a str,
  method: &'a str,
  payload: Value,
}

fn main() -> ExitCode {
  match run() {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("fixture credential provider failed: {error}");
      ExitCode::FAILURE
    },
  }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
  let stdin = io::stdin();
  let mut input = stdin.lock();
  let stdout = io::stdout();
  let mut output = stdout.lock();
  send(
    &mut output,
    &Outgoing {
      request_id: PLUGIN_HANDSHAKE_ID,
      message_type: "Request",
      method: "Handshake",
      payload: json!({ "ProtocolVersion": "2.0.0", "MinimumProtocolVersion": "1.0.0" }),
    },
  )?;

  let mode = env::var("DV_TEST_PROVIDER_MODE").unwrap_or_else(|_| "success".into());
  let mut line = String::with_capacity(1024);
  loop {
    line.clear();
    if input.read_line(&mut line)? == 0 {
      return Ok(());
    }
    let message: Incoming = serde_json::from_str(&line)?;
    trace(&message)?;
    if message.message_type == "Cancel" {
      if mode == "ignore-cancel" {
        continue;
      }
      send(
        &mut output,
        &Outgoing {
          request_id: &message.request_id,
          message_type: "Cancel",
          method: &message.method,
          payload: Value::Null,
        },
      )?;
      return Ok(());
    }
    if message.message_type != "Request" {
      continue;
    }
    if matches!(mode.as_str(), "hang" | "ignore-cancel") && message.method == "GetAuthenticationCredentials" {
      continue;
    }
    if message.method == "GetAuthenticationCredentials"
      && let Ok(log_message) = env::var("DV_TEST_PROVIDER_LOG")
    {
      send(
        &mut output,
        &Outgoing {
          request_id: PLUGIN_LOG_ID,
          message_type: "Request",
          method: "Log",
          payload: json!({ "LogLevel": "Information", "Message": log_message }),
        },
      )?;
      line.clear();
      if input.read_line(&mut line)? == 0 {
        return Err("client exited before acknowledging provider log message".into());
      }
      let acknowledgement: Incoming = serde_json::from_str(&line)?;
      trace(&acknowledgement)?;
      if acknowledgement.request_id != PLUGIN_LOG_ID || acknowledgement.message_type != "Response" || acknowledgement.method != "Log" {
        return Err("client returned an invalid provider log acknowledgement".into());
      }
    }
    let payload = match message.method.as_str() {
      "Handshake" => json!({ "ResponseCode": "Success", "ProtocolVersion": "2.0.0" }),
      "Initialize" | "MonitorNuGetProcessExit" | "SetLogLevel" => json!({ "ResponseCode": "Success" }),
      "GetOperationClaims" => json!({ "Claims": ["Authentication"] }),
      "GetAuthenticationCredentials" if mode == "not-found" => json!({
        "Username": null,
        "Password": null,
        "Message": null,
        "AuthenticationTypes": [],
        "ResponseCode": "NotFound"
      }),
      "GetAuthenticationCredentials" => {
        let username = env::var("DV_TEST_PROVIDER_USERNAME").unwrap_or_else(|_| "fixture-user".into());
        let password = env::var("DV_TEST_PROVIDER_PASSWORD").unwrap_or_else(|_| "fixture-secret".into());
        json!({
          "Username": username,
          "Password": password,
          "Message": null,
          "AuthenticationTypes": ["Basic"],
          "ResponseCode": "Success"
        })
      },
      "Close" => return Ok(()),
      _ => json!({ "ResponseCode": "Error", "Message": "unsupported fixture method" }),
    };
    send(
      &mut output,
      &Outgoing {
        request_id: &message.request_id,
        message_type: "Response",
        method: &message.method,
        payload,
      },
    )?;
  }
}

fn send(output: &mut impl Write, message: &Outgoing<'_>) -> Result<(), Box<dyn std::error::Error>> {
  serde_json::to_writer(&mut *output, message)?;
  output.write_all(b"\n")?;
  output.flush()?;
  Ok(())
}

fn trace(message: &Incoming) -> Result<(), Box<dyn std::error::Error>> {
  let Some(path) = env::var_os("DV_TEST_PROVIDER_TRACE") else {
    return Ok(());
  };
  let mut output = OpenOptions::new().create(true).append(true).open(path)?;
  if message.method == "GetAuthenticationCredentials" && message.message_type == "Request" {
    let noninteractive = message.payload.get("IsNonInteractive").and_then(Value::as_bool).unwrap_or(false);
    let dialog = message.payload.get("CanShowDialog").and_then(Value::as_bool).unwrap_or(false);
    writeln!(output, "{} noninteractive={noninteractive} dialog={dialog}", message.method)?;
  } else {
    writeln!(output, "{} {}", message.message_type, message.method)?;
  }
  Ok(())
}
