use super::input::PROMPT_COMPILED_BYTES_MAX;
use lotta_runtime::RuntimeError;
use tokio_util::sync::CancellationToken;

pub(crate) struct BoundedString {
    pub(crate) value: String,
    limit: usize,
}

impl Default for BoundedString {
    fn default() -> Self {
        Self::with_limit(PROMPT_COMPILED_BYTES_MAX)
    }
}

impl BoundedString {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            value: String::new(),
            limit,
        }
    }

    pub(crate) fn push(&mut self, value: &str) -> Result<(), RuntimeError> {
        let required = checked_limit(self.value.len(), value.len(), self.limit)?;
        self.value
            .try_reserve(required - self.value.len())
            .map_err(|_| limit("compiled prompt bytes"))?;
        self.value.push_str(value);
        Ok(())
    }

    pub(crate) fn line(&mut self, value: &str) -> Result<(), RuntimeError> {
        if !self.value.is_empty() {
            self.push("\n")?;
        }
        self.push(value)
    }

    pub(crate) fn finish(self) -> String {
        self.value
    }
}

pub(crate) fn append_spaces(out: &mut BoundedString, count: usize) -> Result<(), RuntimeError> {
    for _ in 0..count {
        out.push(" ")?;
    }
    Ok(())
}

pub(crate) fn checked_add(
    left: usize,
    right: usize,
    context: &'static str,
) -> Result<usize, RuntimeError> {
    left.checked_add(right).ok_or_else(|| limit(context))
}

pub(crate) fn checked_budget(
    current: usize,
    added: usize,
    context: &'static str,
) -> Result<usize, RuntimeError> {
    let total = checked_add(current, added, context)?;
    if total > PROMPT_COMPILED_BYTES_MAX {
        Err(limit(context))
    } else {
        Ok(total)
    }
}

fn checked_limit(current: usize, added: usize, maximum: usize) -> Result<usize, RuntimeError> {
    let total = checked_add(current, added, "compiled prompt bytes")?;
    if total > maximum {
        Err(limit("compiled prompt bytes"))
    } else {
        Ok(total)
    }
}

pub(crate) fn check_cancelled(token: &CancellationToken) -> Result<(), RuntimeError> {
    if token.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

pub(crate) fn cancelled() -> RuntimeError {
    RuntimeError::Cancelled {
        context: "prompt compilation".into(),
    }
}

pub(crate) fn reserve_one<T>(
    values: &mut Vec<T>,
    context: &'static str,
) -> Result<(), RuntimeError> {
    values.try_reserve(1).map_err(|_| limit(context))
}

pub(crate) fn invalid(context: &'static str) -> RuntimeError {
    RuntimeError::InvalidData {
        context: context.into(),
    }
}

pub(crate) fn limit(context: &'static str) -> RuntimeError {
    RuntimeError::LimitExceeded {
        context: context.into(),
    }
}
