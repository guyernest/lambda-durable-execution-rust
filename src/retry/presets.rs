//! Preset retry strategies for common use cases.
//!
//! This module provides pre-configured retry strategies optimized for common
//! scenarios. Use these to quickly add appropriate retry behavior without
//! manually configuring [`ExponentialBackoff`](super::ExponentialBackoff).
//!
//! # Available Presets
//!
//! | Preset | Max Attempts | Initial Delay | Use Case |
//! |--------|--------------|---------------|----------|
//! | [`default`] | 6 | 5s | General purpose (matches JS SDK) |
//! | [`aggressive`] | 6 | 1s | Transient failures, low-latency needs |
//! | [`conservative`] | 10 | 5s | Important operations, idempotent calls |
//! | [`network`] | 5 | 2s | HTTP/API calls with network error filtering |
//! | [`database`] | 4 | 1s | Database operations with lock/deadlock handling |
//! | [`rate_limited`] | 8 | 5s | APIs with rate limiting (429 responses) |
//! | [`single`] | 2 | 1s | Quick single retry |
//! | [`none`] | 1 | - | No retries |
//!
//! # Example
//!
//! ```rust,no_run
//! use lambda_durable_execution_rust::prelude::*;
//! use lambda_durable_execution_rust::retry::presets;
//!
//! // Use default retry for general operations
//! let config = StepConfig::<()>::new().with_retry_strategy(presets::default());
//!
//! // Use network preset for API calls
//! let api_config = StepConfig::<()>::new().with_retry_strategy(presets::network());
//!
//! // Use conservative preset for critical operations
//! let critical_config = StepConfig::<()>::new().with_retry_strategy(presets::conservative());
//!
//! let _ = (config, api_config, critical_config);
//! ```
//!
//! # Aliased Functions
//!
//! For more readable code, these aliases are available:
//!
//! - [`default_exponential_backoff`] - Alias for [`default`]
//! - [`aggressive_retry`] - Alias for [`aggressive`]
//! - [`patient_retry`] - Alias for [`conservative`]

use crate::retry::{ExponentialBackoff, JitterStrategy, NoRetry, RetryStrategy};
use crate::types::Duration;
use std::sync::Arc;

/// Default retry strategy with reasonable defaults.
///
/// Mirrors the JS SDK `retryPresets.default`:
/// - Max attempts: 6 (1 initial + 5 retries)
/// - Initial delay: 5 seconds
/// - Max delay: 60 seconds
/// - Backoff rate: 2.0
/// - Jitter: Full
pub fn default() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(6)
            .initial_delay(Duration::seconds(5))
            .max_delay(Duration::seconds(60))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::Full)
            .build(),
    )
}

/// Aggressive retry strategy for transient failures.
///
/// - Max attempts: 6
/// - Initial delay: 500ms
/// - Max delay: 30 seconds
/// - Backoff rate: 1.5
/// - Jitter: Full
pub fn aggressive() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(6)
            .initial_delay(Duration::seconds(1)) // Minimum 1 second
            .max_delay(Duration::seconds(30))
            .backoff_rate(1.5)
            .jitter(JitterStrategy::Full)
            .build(),
    )
}

/// Conservative retry strategy for idempotent operations.
///
/// - Max attempts: 10
/// - Initial delay: 5 seconds
/// - Max delay: 5 minutes
/// - Backoff rate: 2.0
/// - Jitter: Half
pub fn conservative() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(10)
            .initial_delay(Duration::seconds(5))
            .max_delay(Duration::minutes(5))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::Half)
            .build(),
    )
}

/// Network-optimized retry strategy for API calls.
///
/// - Max attempts: 5
/// - Initial delay: 2 seconds
/// - Max delay: 1 minute
/// - Backoff rate: 2.0
/// - Jitter: Full
/// - Retryable patterns: timeout, connection, network
pub fn network() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(5)
            .initial_delay(Duration::seconds(2))
            .max_delay(Duration::minutes(1))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::Full)
            .retryable_pattern("timeout")
            .retryable_pattern("connection")
            .retryable_pattern("network")
            .retryable_pattern("reset")
            .retryable_pattern("refused")
            .build(),
    )
}

/// Database-optimized retry strategy.
///
/// - Max attempts: 4
/// - Initial delay: 1 second
/// - Max delay: 30 seconds
/// - Backoff rate: 2.0
/// - Jitter: Equal
/// - Retryable patterns: lock, deadlock, conflict, busy
pub fn database() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(4)
            .initial_delay(Duration::seconds(1))
            .max_delay(Duration::seconds(30))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::Equal)
            .retryable_pattern("lock")
            .retryable_pattern("deadlock")
            .retryable_pattern("conflict")
            .retryable_pattern("busy")
            .retryable_pattern("too many connections")
            .build(),
    )
}

/// Rate-limit aware retry strategy.
///
/// - Max attempts: 8
/// - Initial delay: 5 seconds
/// - Max delay: 2 minutes
/// - Backoff rate: 2.0
/// - Jitter: Full
/// - Retryable patterns: rate limit, throttle, 429
pub fn rate_limited() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(8)
            .initial_delay(Duration::seconds(5))
            .max_delay(Duration::minutes(2))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::Full)
            .retryable_pattern("rate limit")
            .retryable_pattern("throttle")
            .retryable_pattern("429")
            .retryable_pattern("too many requests")
            .build(),
    )
}

/// Single retry - one immediate retry attempt.
///
/// - Max attempts: 2
/// - No delay
pub fn single() -> Arc<dyn RetryStrategy> {
    Arc::new(
        ExponentialBackoff::builder()
            .max_attempts(2)
            .initial_delay(Duration::seconds(1))
            .jitter(JitterStrategy::None)
            .build(),
    )
}

/// No retry - fail immediately.
pub fn none() -> Arc<dyn RetryStrategy> {
    Arc::new(NoRetry::new())
}

// ============================================================================
// Aliases for more readable code
// ============================================================================

/// Alias for [`default`] - clearer naming for exponential backoff.
///
/// # Example
///
/// ```rust,no_run
/// # use lambda_durable_execution_rust::prelude::*;
/// # use lambda_durable_execution_rust::retry::presets;
/// let config = StepConfig::<()>::new().with_retry_strategy(presets::default_exponential_backoff());
/// ```
pub fn default_exponential_backoff() -> Arc<dyn RetryStrategy> {
    default()
}

/// Alias for [`aggressive`] - shorter delays, more retries.
///
/// Use for operations where quick retries are preferred over longer waits.
pub fn aggressive_retry() -> Arc<dyn RetryStrategy> {
    aggressive()
}

/// Alias for [`conservative`] - longer delays, many retries.
///
/// Use for important operations where you can afford to wait longer.
pub fn patient_retry() -> Arc<dyn RetryStrategy> {
    conservative()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_default_preset() {
        let strategy = default();
        let error = io::Error::other("error");

        assert!(strategy.should_retry(&error, 1).should_retry);
        assert!(strategy.should_retry(&error, 5).should_retry);
        assert!(!strategy.should_retry(&error, 6).should_retry);
    }

    #[test]
    fn test_network_preset_filters_errors() {
        let strategy = network();

        let timeout = io::Error::new(io::ErrorKind::TimedOut, "connection timeout");
        let auth = io::Error::new(io::ErrorKind::PermissionDenied, "unauthorized");

        assert!(strategy.should_retry(&timeout, 1).should_retry);
        assert!(!strategy.should_retry(&auth, 1).should_retry);
    }

    #[test]
    fn test_none_preset() {
        let strategy = none();
        let error = io::Error::other("error");

        assert!(!strategy.should_retry(&error, 1).should_retry);
    }
}
