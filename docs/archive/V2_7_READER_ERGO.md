Design Document: A Type-Safe, Zero-Cost API for Deserializing FlatBuffer Streams1.0 Overview and MotivationThis document specifies the design and implementation of a new, high-level API for the flatstream-rs library. The proposal introduces a StreamDeserialize trait and a corresponding StreamReader::process_typed method. The primary objective is to enhance the ergonomics and type safety of the message reading path, creating a symmetrical counterpart to the existing StreamSerialize trait. This enhancement will be achieved as a zero-cost abstraction, preserving the library's core commitment to high performance and its zero-copy data handling principles.1.1 The Current State: Performance at the Cost of ErgonomicsThe flatstream-rs library is architected for maximum performance in streaming scenarios. The core components, StreamWriter and StreamReader, are built upon a composable, trait-based system that leverages static dispatch to eliminate runtime overhead.1 Extensive benchmarks demonstrate the library's high throughput, low latency, and efficient memory management, particularly when using the "expert mode" patterns for writing and the process_all method for reading.1The current high-performance reading path is exposed via the StreamReader::process_all method. This function iterates through a stream, deframes each message using the configured Deframer strategy, and passes a raw byte slice (&[u8]) of the payload to a user-provided closure.1 While this mechanism is highly efficient and guarantees zero-copy access to the message data, it places the burden of deserialization and type management squarely on the user.11.2 Problem Statement: Boilerplate, Type-Safety Gaps, and API AsymmetryThe current reading API, while performant, presents several challenges that impact developer experience and application robustness.1Repetitive Boilerplate: For any given stream of a known message type, the user must repeatedly invoke flatbuffers::get_root::<MyType>(payload)? inside their processing closure. This boilerplate code clutters the application's business logic and is repeated for every project that uses the library.1Type-Safety Gaps: The process_all method is generic over the payload content; it provides a &[u8] slice without any compile-time knowledge of the expected FlatBuffer type. The user is responsible for supplying the correct type parameter to get_root. If an incorrect type is provided, the error is only caught at runtime, resulting in a flatbuffers::InvalidFlatbuffer error. This is a correctness and robustness failure that a stronger type-system integration could prevent.1API Asymmetry: The library provides a clear "data model" trait for writing, StreamSerialize, which allows users to define how their types are serialized into a FlatBufferBuilder.1 This creates a clean and intuitive write path. However, there is no symmetrical trait for the read path. This asymmetry leads to an inconsistent developer experience, where writing is a guided, type-centric process, while reading is a manual, byte-slice-oriented one.1This proposal addresses an opportunity to improve the library's API design and developer experience. The existing codebase demonstrates a deep focus on performance and memory optimization, as evidenced by the sophisticated benchmark suites and the distinction between simple and expert modes.1 This feature represents a maturation of the library, evolving beyond pure performance to enhance usability and robustness, making it a more polished and productive tool for complex, production-grade systems where maintainability and safety are paramount.1.3 The Goal: A Symmetrical, Ergonomic, and Zero-Cost Reading PathThis proposal aims to achieve three primary objectives:Ergonomics: Abstract away the repetitive deserialization logic into a reusable, type-driven mechanism.Type Safety: Enforce the expected message type at the API boundary, leveraging the Rust compiler to catch potential type mismatches.Zero-Cost: Implement these improvements without introducing any runtime performance overhead, ensuring the new API is as fast as the existing manual best practice.To achieve this, the proposal introduces a StreamDeserialize trait that mirrors the existing StreamSerialize trait, creating a more cohesive and intuitive API for both reading and writing FlatBuffer streams.12.0 Proposed Design: The StreamDeserialize TraitThe cornerstone of this proposal is a new public trait, StreamDeserialize, to be defined in src/traits.rs. This trait will serve as the contract for converting a raw byte slice from a stream into a strongly-typed, verifiable FlatBuffer root object.2.1 API DefinitionThe trait will be defined with full documentation as follows 1:Rust/// A trait for types that can be deserialized from a FlatBuffer message payload.
///
/// This trait provides a type-safe and ergonomic way to access the root
/// of a FlatBuffer message from a raw byte slice (`&[u8]`). The primary
/// goal is to improve the reading path without introducing any performance
/// overhead, fully preserving the library's zero-copy principles.
///
/// Implementations of this trait are responsible for calling `flatbuffers::get_root`
/// and handling any FlatBuffer-specific verification.
pub trait StreamDeserialize<'a>: Sized {
    /// The associated type `Root` will be the generated FlatBuffer accessor struct
    /// (e.g., `MyEvent<'a>`). This type must implement `flatbuffers::Follow<'a>`
    /// to allow safe access to the FlatBuffer data.
    type Root: flatbuffers::Follow<'a>;

    /// Safely accesses the FlatBuffer root from a given payload slice.
    ///
    /// This method is responsible for performing the FlatBuffer verification
    /// and returning the strongly-typed root accessor object. It should leverage
    /// `flatbuffers::get_root` internally.
    ///
    /// # Arguments
    /// * `payload` - A borrowed slice of bytes containing the FlatBuffer message.
    ///
    /// # Returns
    /// * `Ok(Self::Root)` - The successfully accessed FlatBuffer root object.
    /// * `Err(flatstream::Error::FlatbuffersError)` - If the FlatBuffer is invalid.
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root>;
}
2.2 Design Rationale: Lifetimes, Associated Types, and Trait BoundsEach component of the StreamDeserialize trait definition is deliberately chosen to enforce safety and correctness while maintaining performance.The 'a Lifetime: The lifetime parameter 'a is the most critical element for ensuring memory safety. It creates a compile-time link between the lifetime of the input payload slice and the lifetime of the returned Root object. This guarantees that the FlatBuffer accessor struct cannot outlive the underlying byte buffer it refers to, preventing dangling pointers and use-after-free errors. This is the essence of Rust's "fearless concurrency" applied to zero-copy parsing.The Sized Bound: This is a standard trait bound in Rust, indicating that the type implementing the trait has a known size at compile time. It is required for the generic type T in the process_typed method, which will be passed by value.The type Root: flatbuffers::Follow<'a> Associated Type: This design choice ensures that only valid FlatBuffer accessor types can be used. The flatbuffers::Follow<'a> trait is implemented by all accessor structs generated by the FlatBuffers compiler (flatc). It provides the mechanism for safely traversing the underlying byte buffer in a zero-copy manner. By constraining the Root associated type with this bound, the API guarantees that the returned object is a legitimate, safe-to-use FlatBuffer view.The from_payload Method: This method serves as the single point of responsibility for the trait. Its purpose is to encapsulate the call to flatbuffers::get_root, which is the standard, verified entry point for reading a FlatBuffer. The method's signature, returning a Result<Self::Root>, ensures that any errors encountered during FlatBuffer verification (such as flatbuffers::InvalidFlatbuffer) are cleanly propagated through the library's unified Error type.1This design cleverly offloads the most complex and security-sensitive part of the operation—buffer verification—to the well-tested flatbuffers crate. flatstream-rs does not re-implement any verification logic; it composes existing, trusted functionality. This is a powerful design choice that reduces the maintenance burden and security surface area of flatstream-rs itself. As the core flatbuffers verifier improves, this library will benefit automatically.2.3 Example ImplementationTo make the abstract definition concrete, consider a user-defined FlatBuffer type, TelemetryEvent. The user, or a future derive macro, would implement the trait as follows 1:Rust// This implementation would be provided by the user or a derive macro.
// Assuming `TelemetryEvent<'a>` is the struct generated by `flatc`.

impl<'a> StreamDeserialize<'a> for TelemetryEvent<'a> {
    type Root = TelemetryEvent<'a>;

    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        // The implementation simply delegates to the trusted flatbuffers::get_root function.
        // It performs verification and returns the accessor struct on success.
        // The error is mapped into the library's error enum for consistency.
        flatbuffers::get_root::<Self::Root>(payload)
           .map_err(flatstream::Error::FlatbuffersError)
    }
}
3.0 Integration with StreamReader: The process_typed MethodThe StreamDeserialize trait is made accessible to users through a new method, process_typed, added to the StreamReader struct. This method will become the primary, recommended API for reading streams of a known message type.3.1 API DefinitionThe new method will be added to the impl<R: Read, D: Deframer> StreamReader<R, D> block in src/reader.rs. It will be fully documented to guide users toward its adoption.1Rustimpl<R: Read, D: Deframer> StreamReader<R, D> {
    /// Processes all messages in the stream, automatically deserializing them
    /// into a strongly-typed FlatBuffer root object.
    ///
    /// This method combines the high-performance, zero-copy `process_all`
    /// with the type-safe deserialization provided by the `StreamDeserialize` trait.
    /// It removes boilerplate and adds compile-time type safety to the reading path.
    ///
    /// # Type Parameters
    /// * `T`: A type that implements `StreamDeserialize<'a>`, representing the
    ///        expected FlatBuffer root type (e.g., `MyEvent`).
    /// * `F`: A closure that processes the strongly-typed FlatBuffer root object.
    ///
    /// # Arguments
    /// * `processor` - A closure that receives the deserialized FlatBuffer root object.
    ///                 It should return `Ok(())` to continue processing or an error to stop.
    pub fn process_typed<'a, T, F>(&'a mut self, mut processor: F) -> Result<()>
    where
        T: StreamDeserialize<'a>,
        F: FnMut(T::Root) -> Result<()>,
    {
        self.process_all(|payload| {
            let root = T::from_payload(payload)?;
            processor(root)
        })
    }
}
The lifetime parameter 'a on process_typed is a crucial element of the design. It constrains the entire operation, ensuring that the StreamReader itself (&'a mut self) lives at least as long as the payload slices it produces. This lifetime is then passed to T: StreamDeserialize<'a>, which in turn ensures that the resulting T::Root accessor is also tied to 'a. This transitive chain of lifetime constraints allows the Rust compiler to statically verify the memory safety of the entire zero-copy data flow, from the reader's internal buffer to the final accessor object used in the closure.3.2 Interaction with process_allThe implementation of process_typed is intentionally simple: it is a direct wrapper around the existing process_all method.1 This design choice is deliberate and provides several key benefits:Code Reuse: It avoids duplicating the complex logic for iterating through the stream, handling EOF conditions, and managing the internal buffer.Inherited Robustness: process_typed automatically inherits all the performance characteristics and battle-tested robustness of the underlying process_all method. Any future improvements to process_all will transparently benefit process_typed.Clear Separation of Concerns: The responsibility of process_all remains to extract raw message payloads. The responsibility of process_typed is to add a type-safe deserialization layer on top of that.3.3 Ergonomics: A Before-and-After ComparisonThe improvement in developer experience is best illustrated with a direct comparison.1Before (Manual process_all):Rustreader.process_all(|payload| {
    let event = flatbuffers::get_root::<TelemetryEvent>(payload)?;
    println!("Timestamp: {}", event.timestamp());
    Ok(())
})?;
After (Typed process_typed):Rust// Assuming `TelemetryEvent` implements `StreamDeserialize`
reader.process_typed::<TelemetryEvent, _>(|event| {
    println!("Timestamp: {}", event.timestamp());
    Ok(())
})?;
The new API is cleaner, more concise, and less error-prone. The benefits are summarized below.AspectCurrent Path (process_all)Proposed Path (process_typed)Code VerbosityHigh; requires manual flatbuffers::get_root call per message.Low; deserialization is abstracted into the method call.Type SafetyRuntime; user must provide the correct type to get_root.Compile-time enforced by StreamDeserialize trait bound on T.Error PotentialHigher; risk of using the wrong FlatBuffer type for a given stream.Lower; type is fixed per process_typed call, reducing ambiguity.Developer FocusOn both stream mechanics and data deserialization.Primarily on business logic within the closure.API SymmetryAsymmetrical; StreamSerialize exists, but no read equivalent.Symmetrical; StreamDeserialize mirrors StreamSerialize.4.0 Performance and Architectural AnalysisA core requirement of this proposal is that the new API must be a "zero-cost abstraction." This section provides the technical justification for this claim and analyzes the feature's integration with the existing library architecture.4.1 Upholding the Zero-Copy GuaranteeThe proposed design does not alter the fundamental zero-copy nature of the library. The data flow remains unchanged:The StreamReader, via its Deframer, reads a complete message frame into its single, reusable internal buffer.1The process_all method provides a borrowed slice (&[u8]) of this internal buffer to the closure.The T::from_payload method calls flatbuffers::get_root, which performs verification and returns a lightweight accessor struct. This accessor struct does not contain the data; it contains references (pointers) into the original payload slice.At no point is the message data copied into a separate user-space struct, which is the hallmark of traditional deserialization libraries like serde. The library's existing zero-copy verification tests will continue to pass, as this fundamental mechanism is preserved.14.2 Justification of the "Zero-Cost Abstraction" ClaimThe claim that process_typed introduces no performance overhead compared to a manually optimized process_all loop is substantiated by two key principles of the Rust compiler: monomorphization and the equivalence of runtime operations.14.2.1 The Role of MonomorphizationIn Rust, generic functions are not typically implemented using dynamic dispatch (like virtual functions in C++). Instead, the compiler performs monomorphization: it creates a specialized, concrete version of the generic function for each type it is used with.When a user calls reader.process_typed::<TelemetryEvent, _>(...), the compiler generates a new version of process_typed specifically for TelemetryEvent. Inside this specialized function, the generic type T is replaced with the concrete type TelemetryEvent. Consequently, the call to T::from_payload(payload) becomes a direct, static function call to TelemetryEvent::from_payload(payload). There are no vtable lookups or other forms of runtime indirection. The compiler can then aggressively inline this call, resulting in machine code that is as efficient as if the code were written manually.14.2.2 Equivalence of WorkThe sequence of operations performed at runtime is identical for both the manual and the new typed approach.Manual process_all path:StreamReader deframes a message into its buffer.The process_all closure is called with a payload: &[u8].The user's code calls flatbuffers::get_root::<TelemetryEvent>(payload).The user's code processes the resulting TelemetryEvent root.New process_typed path:StreamReader deframes a message into its buffer.The process_all closure (internal to process_typed) is called with a payload: &[u8].The process_typed implementation calls TelemetryEvent::from_payload(payload) (which itself calls flatbuffers::get_root).The user's code processes the resulting TelemetryEvent root.The work performed is identical; it has simply been moved from the user's closure into a trait method. The abstraction exists only at the source code level and is compiled away, fulfilling the definition of a zero-cost abstraction.4.3 Architectural Cohesion with Existing Framer and Deframer StrategiesThe proposed deserialization layer is architecturally orthogonal to the existing framing and deframing layer. The Deframer trait's responsibility is to parse the byte stream and extract the raw FlatBuffer payload (&[u8]). This includes handling length prefixes and, in the case of ChecksumDeframer, verifying data integrity.1The new process_typed method operates on the payload after the Deframer has successfully completed its job. This clean separation of concerns ensures that the new feature works seamlessly with all existing and future Deframer implementations (DefaultDeframer, ChecksumDeframer, UnsafeDeframer, SafeTakeDeframer) without any modification.1 The performance of the system remains primarily bottlenecked by I/O and the efficiency of the chosen Deframer, as demonstrated by the performance differences between deframer implementations.1 The logic within process_typed occurs after this critical path and, being zero-cost, has a negligible impact on overall system throughput.5.0 Detailed Implementation PlanThis section provides a concrete, actionable plan for the implementing engineer, translating the design into specific code changes.5.1 File Manifest and ModificationsThe implementation will involve modifications to three existing files and the creation of new example and test files.1src/traits.rs: This file will be modified to add the StreamDeserialize trait definition as specified in section 2.1. Full documentation comments must be included.src/reader.rs: The impl block for StreamReader will be modified to add the process_typed method as specified in section 3.1. Full documentation comments and a doc-test example are required.src/lib.rs: The new trait will be publicly exported by adding the line: pub use traits::StreamDeserialize;.examples/typed_reading.rs: A new example file will be created to demonstrate the end-to-end usage of the new feature, from writing data to reading it back with process_typed.tests/stream_deserialize_integration_tests.rs: A new integration test file will be created to house the comprehensive verification suite for this feature.5.2 Error Handling IntegrationThe error handling for this feature integrates cleanly with the library's existing Error enum.The flatbuffers crate returns a flatbuffers::InvalidFlatbuffer error when get_root fails verification.The from_payload implementation is responsible for mapping this error into the library's Error::FlatbuffersError variant, which already has a #[from] attribute for this purpose.1Inside process_typed, the call let root = T::from_payload(payload)?; uses the ? operator. This will automatically propagate the Error::FlatbuffersError up the call stack, terminating the process_all loop and returning the error to the user. No special error handling logic is required in process_typed itself.6.0 Comprehensive Verification StrategyTo validate the feature's correctness, robustness, and performance, a multi-faceted verification strategy is required, encompassing unit tests, integration tests, and performance benchmarks.16.1 Unit Testing the StreamDeserialize ImplementationUnit tests will be created to validate the contract of the StreamDeserialize trait itself, using a mock FlatBuffer type.Valid Payload: Test that a correctly serialized FlatBuffer payload can be successfully deserialized, and that the resulting root object's data is correct.Invalid Payload: Test that a malformed byte slice (e.g., random bytes, truncated data) results in an Err(Error::FlatbuffersError).Empty Payload: Test that an empty byte slice correctly returns an error.Incorrect Root Type: Test that attempting to deserialize a payload for TypeA using TypeB::from_payload correctly returns an error from the FlatBuffers verifier.6.2 Integration Testing process_typedIntegration tests will verify the behavior of process_typed in the context of the full StreamReader.Correctness Across All Deframers: A test suite, potentially parameterized by a macro, will run a standard write-then-read test using process_typed. This test will be executed against every available Deframer (DefaultDeframer, ChecksumDeframer with each checksum type, SafeTakeDeframer, UnsafeDeframer) to ensure universal compatibility.1Error Condition Tests:Checksum Mismatch: Write a message using ChecksumFramer, manually corrupt the checksum bytes in the stream, and assert that a call to process_typed returns Err(Error::ChecksumMismatch).Invalid Payload: Write a valid frame (length and optional checksum) but with a non-FlatBuffer payload. Assert that process_typed correctly propagates an Err(Error::FlatbuffersError).Unexpected EOF: Write a truncated message frame and assert that process_typed returns Err(Error::UnexpectedEof).Processor Error Propagation: Write a stream of valid messages and use a processor closure that returns an Err for a specific message. Assert that the error is correctly propagated out of process_typed and that processing stops at that point.6.3 Performance Benchmark: Validating the Zero-Cost ClaimA new benchmark will be added to the benches/ directory to empirically prove the "zero-cost" claim.1Setup: A large number (e.g., 10,000) of identical, serialized FlatBuffer messages will be prepared in an in-memory buffer to isolate the benchmark from I/O costs.Baseline Case: A benchmark will measure the performance of the current best practice:Rust// In criterion benchmark
reader.process_all(|payload| {
    let _root = black_box(flatbuffers::get_root::<MyType>(payload)?);
    Ok(())
})
New Method Case: A second benchmark will measure the performance of the new API:Rust// In criterion benchmark
reader.process_typed::<MyType, _>(|root| {
    let _root = black_box(root);
    Ok(())
})
The use of black_box prevents the compiler from optimizing away the operation entirely.Analysis: The benchmark results for mean execution time, throughput (messages/sec), and latency (ns/message) for both cases will be compared. The expected outcome is that the results are statistically indistinguishable, providing empirical evidence for the zero-cost nature of the abstraction.7.0 Future Work: The Path to #The introduction of the StreamDeserialize trait is not just an immediate ergonomic improvement; it is a strategic architectural decision that paves the way for future enhancements, most notably a derive macro.7.1 How This Design Enables a derive MacroProcedural macros in Rust, specifically derive macros, operate on the abstract syntax tree of a struct definition. They allow for the automatic generation of code, such as trait implementations. The StreamDeserialize trait provides a clear, stable, and simple contract that is an ideal target for code generation.1 A # macro could be implemented to automatically generate the impl<'a> StreamDeserialize<'a> for MyType<'a> block, completely eliminating the need for users to write this boilerplate manually.7.2 Conceptual Macro ExpansionThe ultimate user experience would be as follows.User Code with a Future derive Macro:Rustuse flatstream::StreamDeserialize;

// User only needs to add the derive attribute to their flatc-generated struct.
#
pub struct TelemetryEvent<'a> {
    //... fields generated by flatc
}
Conceptual Macro Expansion (what the compiler would see):Rust// The derive macro would generate this implementation automatically.
impl<'a> StreamDeserialize<'a> for TelemetryEvent<'a> {
    type Root = TelemetryEvent<'a>;

    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::get_root::<Self::Root>(payload)
           .map_err(flatstream::Error::FlatbuffersError)
    }
}
This demonstrates that the current proposal is a necessary and well-planned foundational step. By establishing the StreamDeserialize trait now, this work enables a future where flatstream-rs can offer an ergonomic experience for reading that is on par with the simplicity of the serde ecosystem, further solidifying its position as a best-in-class library for high-performance FlatBuffer streaming in Rust.

---

Topic: A new StreamDeserialize trait for ergonomic, type-safe, and zero-copy reading of FlatBuffers messages.
Core Idea:
Introduce a new trait, StreamDeserialize, as a counterpart to the existing StreamSerialize trait. This new trait will abstract the process of safely accessing the root of a FlatBuffer message from a raw byte slice (&[u8]). The primary goal is to improve the ergonomics and type safety of the reading path without introducing any performance overhead, fully preserving the library's zero-copy principles.
Key Concepts to Include in the Design Document:
1. Motivation & Problem Statement:
The current reading path (process_all) is highly performant but requires boilerplate and manual type management within the user's closure.
The user is responsible for calling flatbuffers::get_root::<MyType>(payload) for every message. This is repetitive and can lead to errors if the wrong type is used for a given stream.
The library lacks a symmetrical "data model" trait for reading that mirrors the StreamSerialize trait for writing.
2. Proposed Solution: The StreamDeserialize Trait:
Define a new public trait in src/traits.rs.
The trait should be generic over a lifetime 'a to correctly handle the borrowing of the payload slice.
Trait Definition:
pub trait StreamDeserialize<'a>: Sized {
    // The associated type `Root` will be the generated FlatBuffer accessor struct (e.g., `MyEvent<'a>`).
    type Root: flatbuffers::Follow<'a>;

    /// Safely accesses the FlatBuffer root from a given payload slice.
    /// This method is responsible for verification.
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root>;
}


3. Integration with StreamReader:
A new method, process_typed, should be added to StreamReader.
This method will be generic over a type T that implements StreamDeserialize.
It will internally call the existing process_all and use T::from_payload to handle the verification and type conversion, passing the strongly-typed root object to the user's closure.
StreamReader Method Definition:
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


4. Performance Analysis (Crucial Point):
This is a Zero-Cost Abstraction. The design does not introduce a traditional, costly deserialization step like serde.
Zero-Copy Guarantee: The from_payload method's implementation will use flatbuffers::get_root. This function is the cornerstone of FlatBuffers' zero-copy design. It performs verification and returns a lightweight accessor struct that holds a reference to the original payload slice. No data is copied or moved into a new struct.
No New Overhead: The work performed by from_payload (i.e., the verification step in get_root) is identical to the work users must already perform manually in the process_all closure. This design simply moves that work into a reusable, type-safe trait implementation. The performance will be identical to the current manual best practice.
5. Ergonomics & User Experience:
Provide a clear "before" and "after" comparison.
Before (Manual):
reader.process_all(|payload| {
    let event = flatbuffers::get_root::<TelemetryEvent>(payload)?;
    println!("Timestamp: {}", event.timestamp());
    Ok(())
})?;


After (Typed):
// Assuming `TelemetryEvent` implements `StreamDeserialize`
reader.process_typed::<TelemetryEvent, _>(|event| {
    println!("Timestamp: {}", event.timestamp());
    Ok(())
})?;


Benefits: Removes boilerplate, adds compile-time type safety, and makes the processing logic cleaner and less error-prone.
6. Future Potential:
Mention that this design lays the groundwork for a potential #[derive(StreamDeserialize)] macro in the future, which would provide an ergonomic experience on par with serde for the FlatBuffers ecosystem.
This summary provides all the necessary technical details, justifications, and examples for an AI to generate a comprehensive design document for the StreamDeserialize feature.


—
Original Idea Notes
—
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

