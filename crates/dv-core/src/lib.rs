//! Stable data contracts shared by dv commands and reporters.

mod diagnostic;
mod event;
mod reporter;
mod sdk;

pub use diagnostic::{ContextField, Diagnostic, DiagnosticCode, DiagnosticCodeError, Severity};
pub use event::{CacheOutcome, EVENT_SCHEMA_VERSION, Event, EventPayload, EventStreamError, Outcome, SdkInstallationEvent, validate_events};
pub use reporter::write_json_lines;
pub use sdk::{SdkError, SdkErrorKind, SdkInstallation, SdkInventory, SdkVersion, discover_sdks, discover_sdks_in_roots};
