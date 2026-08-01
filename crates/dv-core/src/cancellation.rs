use std::{
  mem::{align_of, size_of},
  sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
  },
  time::{Duration, Instant},
};

use tokio::sync::Notify;

const RUNNING: u8 = 0;
const CANCELLING: u8 = 1;
const FORCED: u8 = 2;
const NO_REQUEST: u64 = u64::MAX;
const CHILD_GRACE: Duration = Duration::from_secs(2);

/// One command-lifetime cooperative cancellation signal.
///
/// One reference-counted allocation is shared by the process signal handler,
/// async workers, and future child launchers. Signal-written atomics begin an
/// isolated cache-aligned record so they do not share the Arc count line.
#[derive(Clone, Debug)]
pub struct CancellationToken {
  state: Arc<CancellationState>,
}

#[repr(C, align(64))]
#[derive(Debug)]
struct CancellationState {
  requested_elapsed_us: AtomicU64,
  phase: AtomicU8,
  epoch: Instant,
  notify: Notify,
}

const _: () = assert!(size_of::<CancellationToken>() == size_of::<usize>());
const _: () = assert!(align_of::<CancellationToken>() == align_of::<usize>());
const _: () = assert!(size_of::<CancellationState>().is_multiple_of(crate::BENCHMARK_CACHE_LINE_BYTES));
const _: () = assert!(size_of::<CancellationState>() <= 2 * crate::BENCHMARK_CACHE_LINE_BYTES);
const _: () = assert!(align_of::<CancellationState>() == crate::BENCHMARK_CACHE_LINE_BYTES);

impl CancellationToken {
  /// Creates one unset token with the fixed child-shutdown deadline contract.
  pub fn new() -> Self {
    Self {
      state: Arc::new(CancellationState {
        requested_elapsed_us: AtomicU64::new(NO_REQUEST),
        phase: AtomicU8::new(RUNNING),
        epoch: Instant::now(),
        notify: Notify::new(),
      }),
    }
  }

  /// Records one signal. The first requests cooperation; the second forces it.
  pub fn request(&self) {
    if !self.record_first_request() {
      self.state.phase.fetch_max(FORCED, Ordering::Release);
    }
    self.state.notify.notify_waiters();
  }

  fn record_first_request(&self) -> bool {
    let elapsed_us = self.state.epoch.elapsed().as_micros().min(u128::from(u64::MAX - 1)) as u64;
    let first = self
      .state
      .requested_elapsed_us
      .compare_exchange(NO_REQUEST, elapsed_us, Ordering::AcqRel, Ordering::Acquire)
      .is_ok();
    if first {
      self.state.phase.fetch_max(CANCELLING, Ordering::Release);
    }
    first
  }

  /// Requests cooperative cancellation without escalating an existing request.
  pub fn cancel(&self) {
    self.record_first_request();
    self.state.notify.notify_waiters();
  }

  /// Whether at least one cancellation signal has arrived.
  pub fn is_cancelled(&self) -> bool {
    self.state.phase.load(Ordering::Acquire) >= CANCELLING
  }

  /// Whether a second signal requires immediate child termination.
  pub fn is_forced(&self) -> bool {
    self.state.phase.load(Ordering::Acquire) >= FORCED
  }

  /// Fixed maximum time afforded to a cooperative child shutdown.
  pub const fn child_grace(&self) -> Duration {
    Self::default_child_grace()
  }

  /// Process-wide child grace used even when a caller has no token handle.
  pub const fn default_child_grace() -> Duration {
    CHILD_GRACE
  }

  /// Absolute monotonic deadline derived from the first cancellation signal.
  pub fn child_deadline(&self) -> Option<Instant> {
    let elapsed_us = self.state.requested_elapsed_us.load(Ordering::Acquire);
    if elapsed_us == NO_REQUEST {
      return None;
    }
    self
      .state
      .epoch
      .checked_add(Duration::from_micros(elapsed_us))
      .and_then(|requested| requested.checked_add(CHILD_GRACE))
  }

  /// Remaining cooperative child-shutdown time, bounded by the first signal.
  pub fn remaining_child_grace(&self) -> Duration {
    if self.is_forced() {
      return Duration::ZERO;
    }
    self
      .child_deadline()
      .map_or(CHILD_GRACE, |deadline| deadline.saturating_duration_since(Instant::now()))
  }

  pub(crate) async fn cancelled(&self) {
    self.wait_for(CANCELLING).await;
  }

  pub(crate) async fn forced(&self) {
    self.wait_for(FORCED).await;
  }

  async fn wait_for(&self, expected: u8) {
    loop {
      if self.state.phase.load(Ordering::Acquire) >= expected {
        return;
      }
      let notified = self.state.notify.notified();
      if self.state.phase.load(Ordering::Acquire) >= expected {
        return;
      }
      notified.await;
    }
  }
}

impl Default for CancellationToken {
  fn default() -> Self {
    Self::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn cancellation_is_monotonic_and_the_second_request_escalates() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
    assert!(!token.is_forced());
    assert_eq!(token.child_grace(), Duration::from_secs(2));
    assert_eq!(token.remaining_child_grace(), Duration::from_secs(2));
    assert!(token.child_deadline().is_none());

    token.request();
    token.cancelled().await;
    assert!(token.is_cancelled());
    assert!(!token.is_forced());
    assert!(token.child_deadline().is_some());
    assert!(token.remaining_child_grace() <= Duration::from_secs(2));

    let deadline = token.child_deadline();
    std::thread::sleep(Duration::from_millis(1));
    assert_eq!(token.child_deadline(), deadline);
    assert!(token.remaining_child_grace() < Duration::from_secs(2));

    token.request();
    token.forced().await;
    assert!(token.is_forced());
    assert_eq!(token.remaining_child_grace(), Duration::ZERO);

    token.request();
    assert!(token.is_forced());
  }
}
