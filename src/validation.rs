//! Validation strategies for payload safety.
//!
//! This module defines the `Validator` trait and core implementations that can be
//! composed with any `Framer`/`Deframer` via adapters. It mirrors the checksum
//! strategy pattern to preserve orthogonality and zero-cost opt-out.

use crate::error::{Error, Result};

/// A trait for message validation strategies.
///
/// Implementations validate a deframed payload (after any checksum verification)
/// to ensure safety before the application accesses it.
pub trait Validator: Send + Sync {
    /// Validates the payload according to the implementation's rules.
    ///
    /// Returns `Ok(())` if valid, or `Error::ValidationFailed` on failure.
    fn validate(&self, payload: &[u8]) -> Result<()>;

    /// Returns the name of this validator for diagnostics/monitoring.
    fn name(&self) -> &'static str;
}

/// A no-op validator for maximum performance.
///
/// Serves as the zero-cost abstraction path: when used, the compiler can inline
/// and eliminate the call entirely.
#[derive(Default, Clone, Copy)]
pub struct NoValidator;

impl Validator for NoValidator {
    #[inline(always)]
    fn validate(&self, _payload: &[u8]) -> Result<()> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "NoValidator"
    }
}

/// Structural validation for FlatBuffer integrity.
///
/// Uses the `flatbuffers` crate verifier to ensure the buffer describes a
/// structurally valid FlatBuffer table without schema knowledge.
///
/// Limitations:
/// - This validator is type-agnostic. It checks that the buffer has a valid
///   table/vtable layout and respects DoS-limiting options (depth, table count),
///   but it does not perform schema-specific, recursive field verification.
/// - For streams with a known root type that require full schema checks, prefer
///   composing this validator with a future `TypedValidator<T>` using
///   `CompositeValidator::add(...)`.
#[derive(Clone, Copy, Debug)]
pub struct StructuralValidator {
    max_depth: usize,
    max_tables: usize,
}

impl StructuralValidator {
    /// Creates a new `StructuralValidator` with conservative defaults.
    pub fn new() -> Self {
        Self {
            max_depth: 64,
            max_tables: 1_000_000,
        }
    }

    /// Creates a validator with explicit verification limits.
    pub fn with_limits(max_depth: usize, max_tables: usize) -> Self {
        Self {
            max_depth,
            max_tables,
        }
    }
}

impl Default for StructuralValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator for StructuralValidator {
    #[inline]
    fn validate(&self, payload: &[u8]) -> Result<()> {
        // Fast path trivial size sanity check; avoids constructing options for empty buffers.
        if payload.len() < 4 {
            return Err(Error::ValidationFailed {
                validator: self.name(),
                reason: "buffer too small for FlatBuffer".to_string(),
            });
        }

        let opts = flatbuffers::VerifierOptions {
            max_depth: self.max_depth,
            max_tables: self.max_tables,
            ..Default::default()
        };

        // Type-agnostic structural verification: manually compute the root table position
        // from the uoffset at start of the buffer and ask the verifier to visit the table.
        let mut verifier = flatbuffers::Verifier::new(&opts, payload);

        let root_rel =
            u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;

        // Validate table structure at computed position.
        verifier
            .visit_table(root_rel)
            .map(|tv| tv.finish())
            .map_err(|e| Error::ValidationFailed {
                validator: self.name(),
                reason: e.to_string(),
            })?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "StructuralValidator"
    }
}

/// Size-based validator for quick sanity checks.
#[derive(Clone, Copy, Debug)]
pub struct SizeValidator {
    min_size: usize,
    max_size: usize,
}

impl SizeValidator {
    pub fn new(min_size: usize, max_size: usize) -> Self {
        Self { min_size, max_size }
    }
}

impl Validator for SizeValidator {
    #[inline(always)]
    fn validate(&self, payload: &[u8]) -> Result<()> {
        let size = payload.len();
        if size < self.min_size {
            return Err(Error::ValidationFailed {
                validator: self.name(),
                reason: format!("payload size {size} is less than min {}", self.min_size),
            });
        }
        if size > self.max_size {
            return Err(Error::ValidationFailed {
                validator: self.name(),
                reason: format!("payload size {size} exceeds max {}", self.max_size),
            });
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SizeValidator"
    }
}

/// Compose multiple validators into a pipeline.
pub struct CompositeValidator {
    validators: Vec<Box<dyn Validator>>,
}

impl CompositeValidator {
    pub fn new() -> Self {
        Self {
            validators: Vec::new(),
        }
    }

    /// Adds a validator to the pipeline (AND semantics).
    pub fn add<V: Validator + 'static>(mut self, validator: V) -> Self {
        self.validators.push(Box::new(validator));
        self
    }
}

impl Default for CompositeValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Add<Box<dyn Validator>> for CompositeValidator {
    type Output = CompositeValidator;
    fn add(mut self, rhs: Box<dyn Validator>) -> Self::Output {
        self.validators.push(rhs);
        self
    }
}

impl Validator for CompositeValidator {
    fn validate(&self, payload: &[u8]) -> Result<()> {
        for v in &self.validators {
            v.validate(payload)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "CompositeValidator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flatbuffers::FlatBufferBuilder;

    #[test]
    fn no_validator_is_noop() {
        let v = NoValidator;
        assert!(v.validate(b"anything").is_ok());
    }

    #[test]
    fn size_validator_bounds() {
        let v = SizeValidator::new(3, 5);
        assert!(v.validate(b"abc").is_ok());
        assert!(matches!(
            v.validate(b"ab"),
            Err(Error::ValidationFailed { validator, .. }) if validator == "SizeValidator"
        ));
        assert!(matches!(
            v.validate(b"abcdef"),
            Err(Error::ValidationFailed { validator, .. }) if validator == "SizeValidator"
        ));
    }

    fn build_empty_table() -> Vec<u8> {
        let mut b = FlatBufferBuilder::new();
        let start = b.start_table();
        let root = b.end_table(start);
        b.finish(root, None);
        b.finished_data().to_vec()
    }

    #[test]
    fn structural_validator_rejects_tiny_buffer() {
        let sv = StructuralValidator::new();
        let small = [0u8; 2];
        assert!(matches!(
            sv.validate(&small),
            Err(Error::ValidationFailed { validator, .. }) if validator == "StructuralValidator"
        ));
    }

    #[test]
    fn structural_validator_accepts_valid_table() {
        let sv = StructuralValidator::new();
        let buf = build_empty_table();
        assert!(sv.validate(&buf).is_ok());
    }

    #[test]
    fn composite_validator_runs_all() {
        let buf = build_empty_table();
        let composite = CompositeValidator::new()
            .add(SizeValidator::new(1, 10_000))
            .add(StructuralValidator::new());
        assert!(composite.validate(&buf).is_ok());

        let bad = b"ab";
        let composite = CompositeValidator::new().add(SizeValidator::new(3, 10));
        assert!(matches!(
            composite.validate(bad),
            Err(Error::ValidationFailed { validator, .. }) if validator == "SizeValidator"
        ));
    }
}
