#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: usize,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 1 }
    }
}

impl RetryPolicy {
    pub fn classify_failure(&self, message: &str) -> FailureClass {
        FailureClass::classify(message)
    }

    pub fn retry_decision(&self, attempts: usize, message: &str) -> RetryDecision {
        let class = self.classify_failure(message);
        let max_attempts = self.max_attempts.max(1);
        let retryable = class.retryable() && attempts < max_attempts;
        let next_delay_ms = if retryable {
            retry_backoff_ms(class, attempts)
        } else {
            0
        };
        RetryDecision {
            class,
            retryable,
            next_delay_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Transient,
    Permission,
    MissingFile,
    CorruptFile,
    OfflineVolume,
    Permanent,
}

impl FailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Permission => "permission",
            Self::MissingFile => "missing-file",
            Self::CorruptFile => "corrupt-file",
            Self::OfflineVolume => "offline-volume",
            Self::Permanent => "permanent",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(self, Self::Transient | Self::OfflineVolume)
    }

    fn classify(message: &str) -> Self {
        let message = message.to_ascii_lowercase();
        if contains_any(
            &message,
            &[
                "offline",
                "not mounted",
                "unmounted",
                "ejected",
                "network is down",
                "network is unreachable",
                "device not configured",
                "stale file handle",
            ],
        ) {
            Self::OfflineVolume
        } else if contains_any(&message, &["permission", "denied", "not permitted", "tcc"]) {
            Self::Permission
        } else if contains_any(&message, &["missing", "not found", "no such file"]) {
            Self::MissingFile
        } else if contains_any(&message, &["corrupt", "checksum", "crc", "malformed"]) {
            Self::CorruptFile
        } else if contains_any(
            &message,
            &[
                "temporary",
                "transient",
                "timed out",
                "timeout",
                "busy",
                "again",
                "resource temporarily unavailable",
                "interrupted system call",
                "source does not exist",
            ],
        ) {
            Self::Transient
        } else {
            Self::Permanent
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryDecision {
    pub class: FailureClass,
    pub retryable: bool,
    pub next_delay_ms: u64,
}

fn retry_backoff_ms(class: FailureClass, attempts: usize) -> u64 {
    let base_ms = match class {
        FailureClass::Transient => 25,
        FailureClass::OfflineVolume => 250,
        FailureClass::Permission
        | FailureClass::MissingFile
        | FailureClass::CorruptFile
        | FailureClass::Permanent => 0,
    };
    if base_ms == 0 {
        return 0;
    }
    let exponent = attempts.saturating_sub(1).min(8) as u32;
    base_ms * 2_u64.pow(exponent)
}

fn contains_any(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| message.contains(needle))
}
