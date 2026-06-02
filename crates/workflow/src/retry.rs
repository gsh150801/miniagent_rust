use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub retry_on_error: bool,
}

impl RetryPolicy {
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
            retry_on_error: true,
        }
    }

    pub fn exponential(max_retries: usize, initial: Duration) -> Self {
        Self {
            max_retries,
            initial_delay: initial,
            max_delay: Duration::from_secs(120),
            multiplier: 2.0,
            retry_on_error: true,
        }
    }

    pub fn delay_for_attempt(&self, attempt: usize) -> Duration {
        let delay = self.initial_delay.as_millis() as f64
            * self.multiplier.powi(attempt as i32);
        let capped = delay.min(self.max_delay.as_millis() as f64);
        Duration::from_millis(capped as u64)
    }

    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            retry_on_error: false,
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(3)
    }
}
