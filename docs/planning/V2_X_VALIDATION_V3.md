Lesson from rkyv: Validation as a First-Class Citizen

rkyv's core safety promise is that you can validate an entire data structure with a single, fast check before accessing it. flatstream-rs could adopt a similar philosophy.

Suggestion: Introduce a Validator Trait

You can create a Validator trait that is composable with your Framer and Deframer, just like you did with Checksum.
Rust

// In a new `src/validation.rs` module
pub trait Validator {
    fn validate(&self, payload: &[u8]) -> Result<()>;
}

// A default that does nothing, ensuring zero overhead for those who don't need it.
pub struct NoValidator;
impl Validator for NoValidator {
    fn validate(&self, _payload: &[u8]) -> Result<()> { Ok(()) }
}

// An implementation that uses the FlatBuffers verifier
pub struct FlatBufferValidator;
impl Validator for FlatBufferValidator {
    fn validate(&self, payload: &[u8]) -> Result<()> {
        let opts = flatbuffers::VerifierOptions::default();
        flatbuffers::root_with_opts::<flatbuffers::Table>(&opts, payload)
            .map_err(|e| Error::ValidationFailed(e.to_string()))?;
        Ok(())
    }
}

You could then create a ValidatingDeframer that wraps another deframer and a validator. This would allow users to enforce data integrity at the stream level, preventing corrupted FlatBuffers from ever reaching their application logic. This perfectly aligns with the ideas in your V2_6_VALIDATION_V2.md document.
