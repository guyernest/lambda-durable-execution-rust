//! Retry strategy trait and implementations.

use crate::types::Duration;
use std::fmt::Debug;

/// Decision on whether and how to retry an operation.
#[derive(Debug, Clone)]
pub struct RetryDecision {
    /// Whether the operation should be retried.
    pub should_retry: bool,

    /// Delay before the next retry attempt.
    pub delay: Option<Duration>,

    /// Reason for the decision (for logging/debugging).
    pub reason: Option<String>,
}

impl RetryDecision {
    /// Create a decision to retry with the specified delay.
    pub fn retry_after(delay: Duration) -> Self {
        Self {
            should_retry: true,
            delay: Some(delay),
            reason: None,
        }
    }

    /// Create a decision to retry immediately.
    pub fn retry_immediately() -> Self {
        Self {
            should_retry: true,
            delay: None,
            reason: None,
        }
    }

    /// Create a decision not to retry.
    pub fn no_retry() -> Self {
        Self {
            should_retry: false,
            delay: None,
            reason: None,
        }
    }

    /// Create a decision not to retry with a reason.
    pub fn no_retry_with_reason(reason: impl Into<String>) -> Self {
        Self {
            should_retry: false,
            delay: None,
            reason: Some(reason.into()),
        }
    }

    /// Add a reason to this decision.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Trait for implementing retry strategies.
///
/// Retry strategies determine whether a failed operation should be retried
/// and how long to wait before retrying.
///
/// # Examples
///
/// ```rust,no_run
/// use lambda_durable_execution_rust::retry::{RetryStrategy, RetryDecision, ExponentialBackoff};
/// use std::sync::Arc;
///
/// // Use the built-in exponential backoff strategy
/// let strategy: Arc<dyn RetryStrategy> = Arc::new(
///     ExponentialBackoff::builder()
///         .max_attempts(5)
///         .build()
/// );
///
/// // Or implement your own
/// #[derive(Debug)]
/// struct AlwaysRetry;
///
/// impl RetryStrategy for AlwaysRetry {
///     fn should_retry(
///         &self,
///         error: &(dyn std::error::Error + Send + Sync),
///         attempts_made: u32,
///     ) -> RetryDecision {
///         if attempts_made < 3 {
///             RetryDecision::retry_immediately()
///         } else {
///             RetryDecision::no_retry()
///         }
///     }
/// }
/// ```
pub trait RetryStrategy: Send + Sync + Debug {
    /// Determine if an operation should be retried after a failure.
    ///
    /// # Arguments
    /// * `error` - The error that caused the failure
    /// * `attempts_made` - Number of attempts already made (1 = first attempt failed)
    ///
    /// # Returns
    /// A `RetryDecision` indicating whether to retry and how long to wait.
    fn should_retry(
        &self,
        error: &(dyn std::error::Error + Send + Sync),
        attempts_made: u32,
    ) -> RetryDecision;
}

/// Jitter strategy for adding randomization to retry delays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JitterStrategy {
    /// No jitter - use exact calculated delay.
    None,

    /// Full jitter - random value between 0 and calculated delay.
    #[default]
    Full,

    /// Half jitter - random value between half and full calculated delay.
    Half,

    /// Equal jitter - calculated delay / 2 + random(0, delay / 2).
    Equal,
}

/// Exponential backoff retry strategy.
///
/// Implements exponential backoff with configurable parameters including
/// maximum attempts, initial delay, maximum delay, backoff rate, and jitter.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    /// Maximum number of retry attempts (total attempts = max_attempts).
    max_attempts: u32,

    /// Initial delay before first retry.
    initial_delay: Duration,

    /// Maximum delay between retries.
    max_delay: Duration,

    /// Multiplier for delay after each attempt.
    backoff_rate: f64,

    /// Jitter strategy for randomizing delays.
    jitter: JitterStrategy,

    /// Error patterns that are retryable (empty = all errors retryable).
    retryable_patterns: Vec<String>,
}

impl ExponentialBackoff {
    /// Create a builder for configuring exponential backoff.
    pub fn builder() -> ExponentialBackoffBuilder {
        ExponentialBackoffBuilder::default()
    }

    /// Get the maximum number of attempts.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    fn calculate_delay(&self, attempts_made: u32) -> Duration {
        // Calculate base delay with exponential growth
        let base_delay_secs = self.initial_delay.to_seconds() as f64
            * self
                .backoff_rate
                .powi((attempts_made.saturating_sub(1)) as i32);

        // Cap at max delay
        let capped_delay_secs = base_delay_secs.min(self.max_delay.to_seconds() as f64);

        // Apply jitter
        let final_delay_secs = match self.jitter {
            JitterStrategy::None => capped_delay_secs,
            JitterStrategy::Full => {
                use rand::Rng;
                rand::rng().random_range(0.0..=capped_delay_secs)
            }
            JitterStrategy::Half => {
                use rand::Rng;
                let half = capped_delay_secs / 2.0;
                rand::rng().random_range(half..=capped_delay_secs)
            }
            JitterStrategy::Equal => {
                use rand::Rng;
                let half = capped_delay_secs / 2.0;
                half + rand::rng().random_range(0.0..=half)
            }
        };

        Duration::seconds(final_delay_secs.max(1.0).round() as u32)
    }

    fn is_error_retryable(&self, error: &(dyn std::error::Error + Send + Sync)) -> bool {
        if self.retryable_patterns.is_empty() {
            return true;
        }

        let error_str = error.to_string().to_lowercase();
        self.retryable_patterns
            .iter()
            .any(|pattern| error_str.contains(&pattern.to_lowercase()))
    }
}

impl RetryStrategy for ExponentialBackoff {
    fn should_retry(
        &self,
        error: &(dyn std::error::Error + Send + Sync),
        attempts_made: u32,
    ) -> RetryDecision {
        // Check if we've exceeded max attempts
        if attempts_made >= self.max_attempts {
            return RetryDecision::no_retry_with_reason(format!(
                "Max attempts ({}) exceeded",
                self.max_attempts
            ));
        }

        // Check if the error is retryable
        if !self.is_error_retryable(error) {
            return RetryDecision::no_retry_with_reason("Error not retryable");
        }

        // Calculate delay and return retry decision
        let delay = self.calculate_delay(attempts_made);
        RetryDecision::retry_after(delay).with_reason(format!(
            "Attempt {} of {}",
            attempts_made + 1,
            self.max_attempts
        ))
    }
}

/// Builder for ExponentialBackoff.
#[derive(Debug, Clone)]
pub struct ExponentialBackoffBuilder {
    max_attempts: u32,
    initial_delay: Duration,
    max_delay: Duration,
    backoff_rate: f64,
    jitter: JitterStrategy,
    retryable_patterns: Vec<String>,
}

impl Default for ExponentialBackoffBuilder {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::seconds(1),
            max_delay: Duration::minutes(5),
            backoff_rate: 2.0,
            jitter: JitterStrategy::Full,
            retryable_patterns: Vec::new(),
        }
    }
}

impl ExponentialBackoffBuilder {
    /// Set the maximum number of attempts.
    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// Set the initial delay before first retry.
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set the maximum delay between retries.
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Set the backoff rate (multiplier).
    pub fn backoff_rate(mut self, rate: f64) -> Self {
        self.backoff_rate = rate;
        self
    }

    /// Set the jitter strategy.
    pub fn jitter(mut self, jitter: JitterStrategy) -> Self {
        self.jitter = jitter;
        self
    }

    /// Add a retryable error pattern.
    pub fn retryable_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.retryable_patterns.push(pattern.into());
        self
    }

    /// Set multiple retryable error patterns.
    pub fn retryable_patterns(mut self, patterns: Vec<String>) -> Self {
        self.retryable_patterns = patterns;
        self
    }

    /// Build the ExponentialBackoff strategy.
    pub fn build(self) -> ExponentialBackoff {
        ExponentialBackoff {
            max_attempts: self.max_attempts,
            initial_delay: self.initial_delay,
            max_delay: self.max_delay,
            backoff_rate: self.backoff_rate,
            jitter: self.jitter,
            retryable_patterns: self.retryable_patterns,
        }
    }
}

/// A simple retry strategy that retries a fixed number of times with no delay.
#[derive(Debug, Clone)]
pub struct FixedRetry {
    max_attempts: u32,
}

impl FixedRetry {
    /// Create a new fixed retry strategy.
    pub fn new(max_attempts: u32) -> Self {
        Self { max_attempts }
    }
}

impl RetryStrategy for FixedRetry {
    fn should_retry(
        &self,
        _error: &(dyn std::error::Error + Send + Sync),
        attempts_made: u32,
    ) -> RetryDecision {
        if attempts_made < self.max_attempts {
            RetryDecision::retry_immediately()
        } else {
            RetryDecision::no_retry()
        }
    }
}

/// A retry strategy that never retries.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRetry;

impl NoRetry {
    /// Create a new no-retry strategy.
    pub fn new() -> Self {
        Self
    }
}

impl RetryStrategy for NoRetry {
    fn should_retry(
        &self,
        _error: &(dyn std::error::Error + Send + Sync),
        _attempts_made: u32,
    ) -> RetryDecision {
        RetryDecision::no_retry()
    }
}

/// A retry strategy with constant delay between attempts.
#[derive(Debug, Clone)]
pub struct ConstantDelay {
    max_attempts: u32,
    delay: Duration,
}

impl ConstantDelay {
    /// Create a new constant delay retry strategy.
    pub fn new(max_attempts: u32, delay: Duration) -> Self {
        Self {
            max_attempts,
            delay,
        }
    }
}

impl RetryStrategy for ConstantDelay {
    fn should_retry(
        &self,
        _error: &(dyn std::error::Error + Send + Sync),
        attempts_made: u32,
    ) -> RetryDecision {
        if attempts_made < self.max_attempts {
            RetryDecision::retry_after(self.delay)
        } else {
            RetryDecision::no_retry()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_exponential_backoff_basic() {
        let strategy = ExponentialBackoff::builder()
            .max_attempts(3)
            .jitter(JitterStrategy::None)
            .build();

        let error = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");

        // First attempt
        let decision = strategy.should_retry(&error, 1);
        assert!(decision.should_retry);

        // Second attempt
        let decision = strategy.should_retry(&error, 2);
        assert!(decision.should_retry);

        // Third attempt (max reached)
        let decision = strategy.should_retry(&error, 3);
        assert!(!decision.should_retry);
    }

    #[test]
    fn test_exponential_backoff_delay_growth() {
        let strategy = ExponentialBackoff::builder()
            .max_attempts(5)
            .initial_delay(Duration::seconds(1))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::None)
            .build();

        let error = io::Error::new(io::ErrorKind::TimedOut, "timeout");

        let d1 = strategy.should_retry(&error, 1).delay.unwrap().to_seconds();
        let d2 = strategy.should_retry(&error, 2).delay.unwrap().to_seconds();
        let d3 = strategy.should_retry(&error, 3).delay.unwrap().to_seconds();

        // Delays should grow exponentially
        assert_eq!(d1, 1);
        assert_eq!(d2, 2);
        assert_eq!(d3, 4);
    }

    #[test]
    fn test_exponential_backoff_max_delay() {
        let strategy = ExponentialBackoff::builder()
            .max_attempts(10)
            .initial_delay(Duration::seconds(10))
            .max_delay(Duration::seconds(30))
            .backoff_rate(2.0)
            .jitter(JitterStrategy::None)
            .build();

        let error = io::Error::new(io::ErrorKind::TimedOut, "timeout");

        // After several attempts, delay should cap at max_delay
        let decision = strategy.should_retry(&error, 5);
        assert!(decision.delay.unwrap().to_seconds() <= 30);
    }

    #[test]
    fn test_exponential_backoff_retryable_patterns() {
        let strategy = ExponentialBackoff::builder()
            .max_attempts(3)
            .retryable_pattern("timeout")
            .retryable_pattern("connection")
            .build();

        let timeout_error = io::Error::new(io::ErrorKind::TimedOut, "operation timeout");
        let auth_error = io::Error::new(io::ErrorKind::PermissionDenied, "authentication failed");

        assert!(strategy.should_retry(&timeout_error, 1).should_retry);
        assert!(!strategy.should_retry(&auth_error, 1).should_retry);
    }

    #[test]
    fn test_exponential_backoff_jitter_ranges() {
        let error = io::Error::new(io::ErrorKind::TimedOut, "timeout");

        let jitter_none = ExponentialBackoff::builder()
            .max_attempts(3)
            .initial_delay(Duration::seconds(10))
            .max_delay(Duration::seconds(10))
            .jitter(JitterStrategy::None)
            .build();

        let jitter_full = ExponentialBackoff::builder()
            .max_attempts(3)
            .initial_delay(Duration::seconds(10))
            .max_delay(Duration::seconds(10))
            .jitter(JitterStrategy::Full)
            .build();
        let jitter_half = ExponentialBackoff::builder()
            .max_attempts(3)
            .initial_delay(Duration::seconds(10))
            .max_delay(Duration::seconds(10))
            .jitter(JitterStrategy::Half)
            .build();
        let jitter_equal = ExponentialBackoff::builder()
            .max_attempts(3)
            .initial_delay(Duration::seconds(10))
            .max_delay(Duration::seconds(10))
            .jitter(JitterStrategy::Equal)
            .build();

        let none_delay = jitter_none
            .should_retry(&error, 1)
            .delay
            .unwrap()
            .to_seconds();
        assert_eq!(none_delay, 10);

        for _ in 0..50 {
            let full = jitter_full
                .should_retry(&error, 1)
                .delay
                .unwrap()
                .to_seconds();
            assert!((1..=10).contains(&full));

            let half = jitter_half
                .should_retry(&error, 1)
                .delay
                .unwrap()
                .to_seconds();
            assert!((5..=10).contains(&half));

            let equal = jitter_equal
                .should_retry(&error, 1)
                .delay
                .unwrap()
                .to_seconds();
            assert!((5..=10).contains(&equal));
        }
    }

    #[test]
    fn test_fixed_retry() {
        let strategy = FixedRetry::new(2);
        let error = io::Error::other("error");

        assert!(strategy.should_retry(&error, 1).should_retry);
        assert!(!strategy.should_retry(&error, 2).should_retry);
    }

    #[test]
    fn test_no_retry() {
        let strategy = NoRetry::new();
        let error = io::Error::other("error");

        assert!(!strategy.should_retry(&error, 1).should_retry);
    }

    #[test]
    fn test_constant_delay() {
        let strategy = ConstantDelay::new(3, Duration::seconds(5));
        let error = io::Error::other("error");

        let decision = strategy.should_retry(&error, 1);
        assert!(decision.should_retry);
        assert_eq!(decision.delay.unwrap().to_seconds(), 5);

        let decision = strategy.should_retry(&error, 2);
        assert!(decision.should_retry);
        assert_eq!(decision.delay.unwrap().to_seconds(), 5);

        assert!(!strategy.should_retry(&error, 3).should_retry);
    }

    #[test]
    fn test_retry_decision_builders() {
        let decision = RetryDecision::retry_after(Duration::seconds(3));
        assert!(decision.should_retry);
        assert_eq!(decision.delay.unwrap().to_seconds(), 3);

        let decision = RetryDecision::retry_immediately();
        assert!(decision.should_retry);
        assert!(decision.delay.is_none());

        let decision = RetryDecision::no_retry_with_reason("nope");
        assert!(!decision.should_retry);
        assert_eq!(decision.reason.as_deref(), Some("nope"));

        let decision = RetryDecision::no_retry().with_reason("later");
        assert_eq!(decision.reason.as_deref(), Some("later"));
    }
}
