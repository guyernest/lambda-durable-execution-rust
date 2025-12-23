/// Completion requirements for batch operations.
#[derive(Debug, Clone, Default)]
pub struct CompletionConfig {
    /// Minimum number of successful operations required.
    /// If set, the batch will complete early once this many succeed.
    pub min_successful: Option<usize>,

    /// Maximum number of tolerated failures before the batch fails.
    pub tolerated_failure_count: Option<usize>,

    /// Maximum percentage of tolerated failures (0-100) before the batch fails.
    pub tolerated_failure_percentage: Option<f64>,
}

impl CompletionConfig {
    /// Create a new default completion configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the minimum number of successful operations.
    pub fn with_min_successful(mut self, count: usize) -> Self {
        self.min_successful = Some(count);
        self
    }

    /// Set the tolerated failure count.
    pub fn with_tolerated_failures(mut self, count: usize) -> Self {
        self.tolerated_failure_count = Some(count);
        self
    }

    /// Set the tolerated failure percentage (0-100).
    pub fn with_tolerated_failure_percentage(mut self, percentage: f64) -> Self {
        self.tolerated_failure_percentage = Some(percentage);
        self
    }
}
