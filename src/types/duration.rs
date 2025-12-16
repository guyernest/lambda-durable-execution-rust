//! Duration type for wait operations and retry delays.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Add;
use std::time::Duration as StdDuration;

/// Represents a duration for wait operations and retry delays.
///
/// This type provides a user-friendly way to specify durations using
/// named fields for days, hours, minutes, and seconds.
///
/// # Examples
///
/// ```rust
/// use lambda_durable_execution_rust::types::Duration;
///
/// // Create durations using convenience methods
/// let five_seconds = Duration::seconds(5);
/// let ten_minutes = Duration::minutes(10);
/// let one_hour = Duration::hours(1);
/// let one_day = Duration::days(1);
///
/// // Create complex durations using the builder
/// let complex = Duration::builder()
///     .hours(2)
///     .minutes(30)
///     .seconds(15)
///     .build();
///
/// // Convert to total seconds
/// assert_eq!(five_seconds.to_seconds(), 5);
/// assert_eq!(ten_minutes.to_seconds(), 600);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Duration {
    /// Number of days.
    #[serde(default)]
    pub days: u32,
    /// Number of hours.
    #[serde(default)]
    pub hours: u32,
    /// Number of minutes.
    #[serde(default)]
    pub minutes: u32,
    /// Number of seconds.
    #[serde(default)]
    pub seconds: u32,
}

impl Duration {
    /// Create a zero duration.
    pub const fn zero() -> Self {
        Self {
            days: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }

    /// Create a duration from seconds.
    pub const fn seconds(s: u32) -> Self {
        Self {
            days: 0,
            hours: 0,
            minutes: 0,
            seconds: s,
        }
    }

    /// Create a duration from minutes.
    pub const fn minutes(m: u32) -> Self {
        Self {
            days: 0,
            hours: 0,
            minutes: m,
            seconds: 0,
        }
    }

    /// Create a duration from hours.
    pub const fn hours(h: u32) -> Self {
        Self {
            days: 0,
            hours: h,
            minutes: 0,
            seconds: 0,
        }
    }

    /// Create a duration from days.
    pub const fn days(d: u32) -> Self {
        Self {
            days: d,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }

    /// Create a builder for constructing complex durations.
    pub fn builder() -> DurationBuilder {
        DurationBuilder::default()
    }

    /// Convert to total seconds.
    pub fn to_seconds(&self) -> u64 {
        (self.seconds as u64)
            + (self.minutes as u64 * 60)
            + (self.hours as u64 * 3600)
            + (self.days as u64 * 86400)
    }

    /// Convert to total seconds as i32, saturating at i32::MAX.
    ///
    /// This is useful when interfacing with APIs that expect i32 seconds.
    /// Values exceeding i32::MAX (~68 years) are clamped to i32::MAX.
    pub fn to_seconds_i32_saturating(&self) -> i32 {
        let secs = self.to_seconds();
        if secs > i32::MAX as u64 {
            i32::MAX
        } else {
            secs as i32
        }
    }

    /// Convert to total milliseconds.
    pub fn to_millis(&self) -> u64 {
        self.to_seconds().saturating_mul(1000)
    }

    /// Convert to a standard library Duration.
    pub fn to_std_duration(&self) -> StdDuration {
        StdDuration::from_secs(self.to_seconds())
    }

    /// Check if this duration is zero.
    pub fn is_zero(&self) -> bool {
        self.days == 0 && self.hours == 0 && self.minutes == 0 && self.seconds == 0
    }

    /// Calculate the timestamp when this duration elapses from now.
    pub fn deadline_from_now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now() + self.to_std_duration()
    }

    /// Parse from a string in ISO 8601 duration format (e.g., "PT1H30M").
    /// Supports a simplified subset: PTnHnMnS
    pub fn parse_iso8601(s: &str) -> Result<Self, ParseDurationError> {
        if !s.starts_with("PT") {
            return Err(ParseDurationError::InvalidFormat(
                "Duration must start with 'PT'".to_string(),
            ));
        }

        let s = &s[2..];
        let mut duration = Duration::zero();
        let mut num_buf = String::new();

        for c in s.chars() {
            if c.is_ascii_digit() {
                num_buf.push(c);
            } else {
                if num_buf.is_empty() {
                    return Err(ParseDurationError::InvalidFormat(
                        "Expected number before unit".to_string(),
                    ));
                }
                let num: u32 = num_buf
                    .parse()
                    .map_err(|_| ParseDurationError::InvalidFormat("Invalid number".to_string()))?;
                num_buf.clear();

                match c {
                    'D' => duration.days = num,
                    'H' => duration.hours = num,
                    'M' => duration.minutes = num,
                    'S' => duration.seconds = num,
                    _ => {
                        return Err(ParseDurationError::InvalidFormat(format!(
                            "Unknown unit: {}",
                            c
                        )))
                    }
                }
            }
        }

        Ok(duration)
    }

    /// Format as ISO 8601 duration string.
    pub fn to_iso8601(&self) -> String {
        let mut parts = Vec::new();

        if self.days > 0 {
            parts.push(format!("{}D", self.days));
        }
        if self.hours > 0 {
            parts.push(format!("{}H", self.hours));
        }
        if self.minutes > 0 {
            parts.push(format!("{}M", self.minutes));
        }
        if self.seconds > 0 || parts.is_empty() {
            parts.push(format!("{}S", self.seconds));
        }

        format!("PT{}", parts.join(""))
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();

        if self.days > 0 {
            parts.push(format!(
                "{} day{}",
                self.days,
                if self.days == 1 { "" } else { "s" }
            ));
        }
        if self.hours > 0 {
            parts.push(format!(
                "{} hour{}",
                self.hours,
                if self.hours == 1 { "" } else { "s" }
            ));
        }
        if self.minutes > 0 {
            parts.push(format!(
                "{} minute{}",
                self.minutes,
                if self.minutes == 1 { "" } else { "s" }
            ));
        }
        if self.seconds > 0 || parts.is_empty() {
            parts.push(format!(
                "{} second{}",
                self.seconds,
                if self.seconds == 1 { "" } else { "s" }
            ));
        }

        write!(f, "{}", parts.join(", "))
    }
}

impl Add for Duration {
    type Output = Duration;

    fn add(self, other: Duration) -> Duration {
        // Clamp to the maximum Duration representable by this type (normalized).
        //
        // This avoids truncation when converting back into u32 fields.
        const MAX_TOTAL_SECONDS: u64 = u32::MAX as u64 * 86_400 + 23 * 3_600 + 59 * 60 + 59;

        let total_seconds = self
            .to_seconds()
            .saturating_add(other.to_seconds())
            .min(MAX_TOTAL_SECONDS);

        Duration {
            days: (total_seconds / 86400) as u32,
            hours: ((total_seconds % 86400) / 3600) as u32,
            minutes: ((total_seconds % 3600) / 60) as u32,
            seconds: (total_seconds % 60) as u32,
        }
    }
}

impl From<StdDuration> for Duration {
    fn from(std: StdDuration) -> Self {
        const MAX_TOTAL_SECONDS: u64 = u32::MAX as u64 * 86_400 + 23 * 3_600 + 59 * 60 + 59;

        let total_secs = std.as_secs().min(MAX_TOTAL_SECONDS);
        Duration {
            days: (total_secs / 86400) as u32,
            hours: ((total_secs % 86400) / 3600) as u32,
            minutes: ((total_secs % 3600) / 60) as u32,
            seconds: (total_secs % 60) as u32,
        }
    }
}

impl From<Duration> for StdDuration {
    fn from(d: Duration) -> Self {
        StdDuration::from_secs(d.to_seconds())
    }
}

/// Builder for constructing Duration values.
#[derive(Debug, Clone, Default)]
pub struct DurationBuilder {
    days: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
}

impl DurationBuilder {
    /// Set the number of days.
    pub fn days(mut self, days: u32) -> Self {
        self.days = days;
        self
    }

    /// Set the number of hours.
    pub fn hours(mut self, hours: u32) -> Self {
        self.hours = hours;
        self
    }

    /// Set the number of minutes.
    pub fn minutes(mut self, minutes: u32) -> Self {
        self.minutes = minutes;
        self
    }

    /// Set the number of seconds.
    pub fn seconds(mut self, seconds: u32) -> Self {
        self.seconds = seconds;
        self
    }

    /// Build the Duration.
    pub fn build(self) -> Duration {
        Duration {
            days: self.days,
            hours: self.hours,
            minutes: self.minutes,
            seconds: self.seconds,
        }
    }
}

/// Error type for duration parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseDurationError {
    /// Invalid format encountered.
    InvalidFormat(String),
}

impl fmt::Display for ParseDurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseDurationError::InvalidFormat(msg) => write!(f, "Invalid duration format: {}", msg),
        }
    }
}

impl std::error::Error for ParseDurationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_seconds() {
        let d = Duration::seconds(30);
        assert_eq!(d.to_seconds(), 30);
        assert_eq!(d.to_millis(), 30000);
    }

    #[test]
    fn test_duration_minutes() {
        let d = Duration::minutes(5);
        assert_eq!(d.to_seconds(), 300);
    }

    #[test]
    fn test_duration_hours() {
        let d = Duration::hours(2);
        assert_eq!(d.to_seconds(), 7200);
    }

    #[test]
    fn test_duration_days() {
        let d = Duration::days(1);
        assert_eq!(d.to_seconds(), 86400);
    }

    #[test]
    fn test_duration_builder() {
        let d = Duration::builder()
            .days(1)
            .hours(2)
            .minutes(30)
            .seconds(15)
            .build();

        let expected = 86400 + 7200 + 1800 + 15;
        assert_eq!(d.to_seconds(), expected);
    }

    #[test]
    fn test_duration_add() {
        let d1 = Duration::hours(1);
        let d2 = Duration::minutes(30);
        let sum = d1 + d2;

        assert_eq!(sum.to_seconds(), 5400);
    }

    #[test]
    fn test_duration_add_saturates_at_max() {
        let max = Duration::builder()
            .days(u32::MAX)
            .hours(23)
            .minutes(59)
            .seconds(59)
            .build();

        let sum = max + Duration::seconds(1);
        assert_eq!(sum, max);
    }

    #[test]
    fn test_duration_display() {
        let d = Duration::builder().hours(1).minutes(30).build();
        assert_eq!(d.to_string(), "1 hour, 30 minutes");

        let zero = Duration::zero();
        assert_eq!(zero.to_string(), "0 seconds");
    }

    #[test]
    fn test_duration_iso8601() {
        let d = Duration::builder().hours(1).minutes(30).seconds(45).build();
        assert_eq!(d.to_iso8601(), "PT1H30M45S");

        let parsed = Duration::parse_iso8601("PT1H30M45S").unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn test_duration_serialization() {
        let d = Duration::builder().hours(2).minutes(15).build();

        let json = serde_json::to_string(&d).unwrap();
        let deserialized: Duration = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, d);
    }

    #[test]
    fn test_std_duration_conversion() {
        let d = Duration::minutes(5);
        let std_d: StdDuration = d.into();
        assert_eq!(std_d.as_secs(), 300);

        let back: Duration = std_d.into();
        assert_eq!(back.to_seconds(), 300);
    }

    #[test]
    fn test_std_duration_conversion_saturates() {
        const MAX_TOTAL_SECONDS: u64 = u32::MAX as u64 * 86_400 + 23 * 3_600 + 59 * 60 + 59;

        let std_d = StdDuration::from_secs(MAX_TOTAL_SECONDS + 1);
        let d: Duration = std_d.into();
        assert_eq!(d.to_seconds(), MAX_TOTAL_SECONDS);
    }
}
