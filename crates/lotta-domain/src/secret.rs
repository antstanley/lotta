use std::fmt;

/// A value whose formatting is always redacted.
pub struct Secret<T> {
    value: T,
}

impl<T> Secret<T> {
    /// Wraps a secret value.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self { value }
    }

    /// Provides controlled access to the value through a caller-supplied operation.
    ///
    /// The higher-ranked callback prevents its result from borrowing the protected value. Callers
    /// remain responsible for ensuring owned operation results do not disclose secret material.
    pub fn expose_secret<R>(&self, operation: impl for<'a> FnOnce(&'a T) -> R) -> R {
        operation(&self.value)
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.value;
        formatter.write_str("[REDACTED]")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.value;
        formatter.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PanicsIfFormatted;

    impl fmt::Debug for PanicsIfFormatted {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("inner Debug reached")
        }
    }

    impl fmt::Display for PanicsIfFormatted {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("inner Display reached")
        }
    }

    pub(super) fn assert_redacts() {
        let literal = "sk-live-123";
        let secret = Secret::new(literal);
        assert_eq!(secret.expose_secret(|value| value.len()), literal.len());
        assert!(!format!("{secret:?}").contains(literal));
        assert!(!format!("{secret}").contains(literal));
        assert_eq!(
            format!("{:?}", Secret::new(PanicsIfFormatted)),
            "[REDACTED]"
        );
        assert_eq!(format!("{}", Secret::new(PanicsIfFormatted)), "[REDACTED]");
    }
}

#[cfg(test)]
#[test]
fn redacts() {
    tests::assert_redacts();
}
