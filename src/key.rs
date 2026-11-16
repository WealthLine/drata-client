//! The Drata API key.

use std::fmt;

/// A Drata API key, passed in by the caller.
///
/// The library never reads a key from a vault, a file or the environment; the
/// caller decides where it comes from and hands it over here. The key never
/// appears in `Debug` output, and the type has no `Display`, so it cannot reach
/// a log line or an error by being formatted.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key the caller has already read.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key itself, for a [`Transport`](crate::Transport) to put in the
    /// `Authorization` header. Nothing else should call it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::ApiKey;

    #[test]
    fn debug_does_not_show_the_key() {
        let key = ApiKey::new("sk-live-0123456789");
        assert_eq!(
            format!("{key:?}"),
            "ApiKey(<redacted>)",
            "Debug should print a placeholder and never the key"
        );
    }
}
