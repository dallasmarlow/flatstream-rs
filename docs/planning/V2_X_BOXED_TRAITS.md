# V2.X: Boxed Framer/Deframer Traits for Greater Flexibility

This document proposes optional "boxed" (type-erased) variants of the `Framer` and `Deframer` traits to improve flexibility, reduce generic bloat, and enable runtime composition, while preserving the library's zero-copy guarantees and existing ergonomics.

## Motivation

- Allow runtime selection/swapping of framing/deframing strategies (e.g., via configuration or plugins) without proliferating generic types.
- Reduce monomorphization/code-size when many adapter combinations are used across a large codebase.
- Provide simpler type signatures at API boundaries (FFI, dynamic registries) by erasing concrete types.
- Maintain all v2.5/2.6 zero-copy behaviors; only dynamic dispatch changes the call style.

## Design Goals

- Preserve existing generic API unchanged. Boxed support is additive and entirely optional.
- Keep zero-copy behavior: payloads remain `&[u8]` borrowed from internal buffers.
- Make dynamic dispatch explicit, with clear types/aliases and opt-in constructors/adapters.
- Keep observers and bounded adapters composable in both generic and boxed worlds.

## Problem: Object Safety of Current Traits

Current traits are not object-safe because they have generic methods:

```rust
pub trait Framer {
    fn frame_and_write<W: std::io::Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()>;
}

pub trait Deframer {
    fn read_and_deframe<R: std::io::Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>>;
}
```

Trait objects (`dyn Framer`, `dyn Deframer`) cannot use generic methods. We therefore introduce object-safe mirrors and bridging.

## Proposed API (Additive)

1) Object-safe mirror traits

```rust
pub trait FramerDyn: Send + Sync {
    fn frame_and_write_dyn(
        &self,
        writer: &mut dyn std::io::Write,
        payload: &[u8],
    ) -> Result<()>;
}

pub trait DeframerDyn: Send + Sync {
    fn read_and_deframe_dyn(
        &self,
        reader: &mut dyn std::io::Read,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>>;
}
```

2) Blanket bridges for all existing implementations

```rust
impl<T: Framer + Send + Sync> FramerDyn for T {
    fn frame_and_write_dyn(&self, writer: &mut dyn std::io::Write, payload: &[u8]) -> Result<()> {
        // Delegate via a monomorphic helper
        self.frame_and_write(writer, payload)
    }
}

impl<T: Deframer + Send + Sync> DeframerDyn for T {
    fn read_and_deframe_dyn(
        &self,
        reader: &mut dyn std::io::Read,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        self.read_and_deframe(reader, buffer)
    }
}
```

3) Type aliases for ergonomic use

```rust
pub type BoxFramer = Box<dyn FramerDyn + 'static>;
pub type BoxDeframer = Box<dyn DeframerDyn + 'static>;

pub type ArcFramer = std::sync::Arc<dyn FramerDyn + 'static>;
pub type ArcDeframer = std::sync::Arc<dyn DeframerDyn + 'static>;
```

4) Extension helpers

```rust
pub trait FramerBoxExt: Framer + Sized + Send + Sync + 'static {
    fn boxed(self) -> BoxFramer { Box::new(self) }
    fn shared(self) -> ArcFramer { std::sync::Arc::new(self) }
}
impl<T: Framer + Sized + Send + Sync + 'static> FramerBoxExt for T {}

pub trait DeframerBoxExt: Deframer + Sized + Send + Sync + 'static {
    fn boxed(self) -> BoxDeframer { Box::new(self) }
    fn shared(self) -> ArcDeframer { std::sync::Arc::new(self) }
}
impl<T: Deframer + Sized + Send + Sync + 'static> DeframerBoxExt for T {}
```

5) Adapters for seamless integration with existing `StreamReader`/`StreamWriter`

We provide tiny adapters that implement the generic traits by delegating to the boxed traits, allowing users to keep using the existing generic `StreamReader`/`StreamWriter` types without new constructors:

```rust
pub struct BoxedFramerAdapter(pub BoxFramer);
impl Framer for BoxedFramerAdapter {
    fn frame_and_write<W: std::io::Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        self.0.frame_and_write_dyn(writer, payload)
    }
}

pub struct BoxedDeframerAdapter(pub BoxDeframer);
impl Deframer for BoxedDeframerAdapter {
    fn read_and_deframe<R: std::io::Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        self.0.read_and_deframe_dyn(reader, buffer)
    }
}
```

With these, one can write:

```rust
let boxed_deframer = DefaultDeframer.boxed();
let adapter = BoxedDeframerAdapter(boxed_deframer);
let mut reader = StreamReader::new(io_reader, adapter);
```

6) Observer/Bounded adapters

No changes needed to generic adapters. For convenience, we can also provide boxed variants:

```rust
pub struct BoundedFramerBoxed { inner: BoxFramer, max_len: usize }
pub struct BoundedDeframerBoxed { inner: BoxDeframer, max_len: usize }
// ... implement FramerDyn / DeframerDyn by delegating and validating bounds first.

pub struct ObserverFramerBoxed<C: Fn(&[u8]) + Send + Sync + 'static> {
    inner: BoxFramer,
    callback: C,
}
pub struct ObserverDeframerBoxed<C: Fn(&[u8]) + Send + Sync + 'static> {
    inner: BoxDeframer,
    callback: C,
}
```

These keep callback and bound logic while enabling an entirely boxed composition stack when desired.

## StreamReader/StreamWriter API

This proposal is additive. We can:

- Keep `StreamReader<R, D: Deframer>` and `StreamWriter<'a, W, F: Framer, A>` unchanged.
- Optionally expose convenience constructors:

```rust
impl<R: std::io::Read> StreamReader<R, BoxedDeframerAdapter> {
    pub fn new_boxed(reader: R, deframer: BoxDeframer) -> Self {
        Self::new(reader, BoxedDeframerAdapter(deframer))
    }
}

impl<'a, W: std::io::Write> StreamWriter<'a, W, BoxedFramerAdapter> {
    pub fn new_boxed(writer: W, framer: BoxFramer) -> Self {
        Self::new(writer, BoxedFramerAdapter(framer))
    }
}
```

These avoid an explicit adapter name at call sites when preferred.

## Performance Considerations

- Dynamic dispatch adds one vtable call per message on the read/write path.
- Zero-copy is unaffected: payloads remain borrowed slices; no additional copying.
- Expected overhead in typical IO-bound workloads: ~0–3%. Microbenchmarks recommended for hot loops.
- Potential compile-time and binary-size improvements when many generic combinations are used (less monomorphization).

## Backward Compatibility

- Entirely opt-in; no breaking changes.
- Generic APIs, adapters, and examples remain valid.
- Boxed traits are additive and can live behind a cargo feature (e.g., `boxed`) initially.

## Migration and Usage Examples

```rust
// Generic to boxed: runtime selection
let framer: BoxFramer = if use_checksum {
    ChecksumFramer::new(XxHash64::new()).boxed()
} else {
    DefaultFramer.boxed()
};

let mut writer = StreamWriter::new_boxed(io_writer, framer);

// Mix generic and boxed: wrap a boxed deframer for use with existing reader
let deframer = BoxedDeframerAdapter(DefaultDeframer.boxed());
let mut reader = StreamReader::new(io_reader, deframer);
```

## Alternatives Considered

- "AnyFramer/AnyDeframer" newtypes only, without distinct object-safe traits: would still require object-safe methods internally; the explicit `*Dyn` traits make the design clearer.
- Making existing traits object-safe by changing method signatures: would be a breaking API change, so rejected for v2.x.

## Open Questions

- Names: `FramerDyn`/`DeframerDyn` vs `FramerObject`/`DeframerObject`.
- Feature gating: default-on vs `boxed` feature. Suggest default-on (low risk), but `boxed` feature is also fine.
- Threading bounds: require `Send + Sync`? Suggest yes for generality; we can also offer non-`Send` variants.

## Implementation Plan (Incremental)

1. Add `FramerDyn`/`DeframerDyn` traits and blanket impls.
2. Add type aliases `BoxFramer`/`BoxDeframer` and `Arc*` variants.
3. Add `FramerBoxExt`/`DeframerBoxExt` with `.boxed()`/`.shared()` helpers.
4. Add `BoxedFramerAdapter`/`BoxedDeframerAdapter` and optional `StreamReader::new_boxed`/`StreamWriter::new_boxed`.
5. Provide boxed variants of key adapters as convenience (optional).
6. Add examples and microbenchmarks comparing generic vs boxed throughput.

## Testing and Verification

- Unit tests exercising boxed framers/deframers in read/write paths, including observers and bounds.
- Ensure zero-copy assertions remain (payload slices identical to internal buffer ranges).
- Benchmarks: measure per-message overhead across payload sizes and IO backends.

## Conclusion

Boxed framer/deframer support provides a pragmatic, opt-in flexibility layer with minimal runtime overhead and no impact on zero-copy guarantees. It complements the existing generic design by enabling dynamic composition and cleaner API boundaries when needed.

## Illustrative Example Code (for a future examples/boxed_usage_example.rs)

The following example code demonstrates how the boxed API would be used once implemented. This example assumes a cargo feature `boxed` enables the boxed traits, aliases, adapters, and the optional convenience constructors described above.

```rust
// examples/boxed_usage_example.rs
// Run with:
//   cargo run --example boxed_usage_example --features boxed

#![cfg(feature = "boxed")]

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    DefaultDeframer, DefaultFramer, Result, StreamReader, StreamWriter, StreamSerialize,
};

// Hypothetical boxed API, per this proposal.
use flatstream::{
    ArcDeframer, ArcFramer, BoxDeframer, BoxFramer, BoxedDeframerAdapter, BoxedFramerAdapter,
    DeframerBoxExt, FramerBoxExt,
};

use std::io::Cursor;

fn build_message(builder: &mut FlatBufferBuilder) {
    let data = builder.create_string("hello boxed world");
    builder.finish(data, None);
}

fn main() -> Result<()> {
    // 1) Runtime selection of a framer, then boxed usage with convenience constructor
    let use_checksum = false; // Toggle at runtime

    let framer: BoxFramer = if use_checksum {
        #[cfg(feature = "xxhash")]
        {
            use flatstream::{framing::ChecksumFramer, XxHash64};
            ChecksumFramer::new(XxHash64::new()).boxed()
        }

        #[cfg(not(feature = "xxhash"))]
        {
            DefaultFramer.boxed()
        }
    } else {
        DefaultFramer.boxed()
    };

    let mut out = Vec::new();
    let writer = Cursor::new(&mut out);

    // If convenience ctor is provided:
    // let mut writer = StreamWriter::new_boxed(writer, framer);
    // Otherwise, use the adapter to keep generic StreamWriter:
    let mut writer = StreamWriter::new(writer, BoxedFramerAdapter(framer));

    let mut fb = FlatBufferBuilder::new();
    build_message(&mut fb);
    writer.write_finished(&mut fb)?;
    writer.flush()?;

    // 2) Mixing generic and boxed on the read side via adapter
    let boxed_deframer: BoxDeframer = DefaultDeframer.boxed();
    let deframer_adapter = BoxedDeframerAdapter(boxed_deframer);
    let mut reader = StreamReader::new(Cursor::new(out), deframer_adapter);
    reader.process_all(|payload| {
        println!("received boxed payload: {} bytes", payload.len());
        Ok(())
    })?;

    // 3) Shared (Arc) variants for multi-thread fanout scenarios
    let _shared_framer: ArcFramer = DefaultFramer.shared();
    let _shared_deframer: ArcDeframer = DefaultDeframer.shared();

    Ok(())
}
```

If boxed adapters for bounds/observers are also included, a follow-up example could look like this:

```rust
#![cfg(feature = "boxed")]

use flatstream::{
    BoundedDeframerBoxed, BoundedFramerBoxed, BoxDeframer, BoxFramer, DeframerBoxExt,
    FramerBoxExt, ObserverDeframerBoxed, ObserverFramerBoxed, Result, StreamReader, StreamWriter,
};
use std::cell::Cell;
use std::io::Cursor;

fn main() -> Result<()> {
    // Bounded + observed write path (boxed)
    let seen = Cell::new(0usize);
    let framer: BoxFramer = ObserverFramerBoxed::new(
        BoundedFramerBoxed::new(flatstream::DefaultFramer.boxed(), 1 << 20),
        |p: &[u8]| seen.set(seen.get() + p.len()),
    )
    .boxed();

    let mut out = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut out), flatstream::BoxedFramerAdapter(framer));
    writer.write(&"boxed adapters")?;
    writer.flush()?;

    // Bounded + observed read path (boxed)
    let count = Cell::new(0usize);
    let deframer: BoxDeframer = ObserverDeframerBoxed::new(
        BoundedDeframerBoxed::new(flatstream::DefaultDeframer.boxed(), 1 << 20),
        |_p: &[u8]| count.set(count.get() + 1),
    )
    .boxed();

    let mut reader = StreamReader::new(Cursor::new(out), flatstream::BoxedDeframerAdapter(deframer));
    reader.process_all(|_p| Ok(()))?;

    println!("observed: bytes={}, messages={}", seen.get(), count.get());
    Ok(())
}
```

These examples are intentionally scoped to a `boxed` cargo feature and serve as ready-to-copy templates for an `examples/boxed_usage_example.rs` (and optionally `examples/boxed_adapters_example.rs`) once the implementation lands.


