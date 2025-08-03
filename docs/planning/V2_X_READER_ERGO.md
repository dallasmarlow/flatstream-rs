Lesson from serde: Ergonomics via a "Data Model" Trait

serde is successful because #[derive(Serialize, Deserialize)] makes it incredibly easy to use. While flatstream-rs already has StreamSerialize, the reading path could be made more ergonomic.

Suggestion: Introduce a StreamDeserialize Trait

This trait wouldn't perform a costly deserialization, but rather provide a type-safe way to get the verified FlatBuffer root object.
Rust

// In src/traits.rs
pub trait StreamDeserialize<'a>: Sized {
    type Root: flatbuffers::Follow<'a>;

    /// Gets the root of the FlatBuffer from a payload slice.
    /// This is the place to put verification logic.
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root>;
}

This would allow you to create a new, highly ergonomic method on StreamReader:
Rust

// In src/reader.rs
impl<R: Read, D: Deframer> StreamReader<R, D> {
    pub fn process_typed<'a, T: StreamDeserialize<'a>, F>(&'a mut self, mut processor: F) -> Result<()>
    where
        F: FnMut(T::Root) -> Result<()>,
    {
        self.process_all(|payload| {
            let root = T::from_payload(payload)?;
            processor(root)
        })
    }
}

This would be a huge win for users, as they could process streams in a fully type-safe way without boilerplate.
