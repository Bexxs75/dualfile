//! A password that cannot leak through formatting and is overwritten when dropped.

use std::fmt;
use zeroize::Zeroizing;

/// A secret text (a password). `Debug` and `Display` show `***`, there is no `Serialize`, and
/// the memory is overwritten on drop (`zeroize`). Read it with `expose`, only where it is
/// sent to the server.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(Zeroizing::new(text.into()))
    }

    /// The secret text. Never put the result into an error, a log line or an event.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Replaces every occurrence of the secret in `text` by `***` (a server that echoes the
    /// password in a reply must not get it into an error message).
    pub fn scrub(&self, text: &str) -> String {
        if self.0.is_empty() {
            text.to_owned()
        } else {
            text.replace(self.0.as_str(), "***")
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_never_shows_the_text() {
        let secret = Secret::new("hunter2-marker");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(format!("{secret}"), "***");
        assert_eq!(
            format!("{:#?}", Some(&secret)),
            "Some(\n    Secret(***),\n)"
        );
        assert_eq!(secret.expose(), "hunter2-marker");
        assert_eq!(secret.scrub("pw hunter2-marker!"), "pw ***!");
        assert_eq!(Secret::new("").scrub("abc"), "abc");
    }
}
