# Design Document: flatstream-rs v3.0 - Schema-Aware Streaming

Version: 1.0  
Status: Proposed  
Author: [Implementation Team]  

## 1. Intent and Motivation

This document outlines the proposed design for flatstream-rs v3.0, which introduces schema-aware streaming capabilities that leverage FlatBuffers' unique strengths. The current v2.5 implementation treats FlatBuffers as opaque byte blobs, missing opportunities for type safety, schema evolution, and field-level optimizations that make FlatBuffers compelling.

The core business need is to transform flatstream-rs from a generic streaming wrapper into a FlatBuffers-native solution that provides type-safe, schema-aware streaming while maintaining the high-performance characteristics that make the library valuable for telemetry and real-time data processing.

## 2. The Evolution Process: From Opaque Streams to Schema-Aware Intelligence

Our analysis has revealed several limitations in the current approach:

**v2.5 "Opaque" Limitations**: The current design treats FlatBuffers as black boxes, providing no compile-time guarantees about message types, no support for schema evolution, and no ability to perform field-level operations without full deserialization.

**v3.0 "Schema-Aware" Vision**: This design introduces first-class support for FlatBuffer schemas, enabling type-safe streaming, automatic schema validation, field-level filtering, and schema evolution support—all while maintaining zero-copy performance.

## 3. Core Design: Schema-Aware Architecture

### 3.1. Enhanced Stream Format with Schema Metadata

The v3.0 stream format evolves to include schema identification and versioning:

[Header Block]
├─ Magic Number (4 bytes): "FBST" (FlatBuffer STream)
├─ Format Version (2 bytes): 0x0300
├─ Schema Identifier (16 bytes): UUID or hash
└─ Schema Version (4 bytes): Semantic version
[Message Frame]
├─ Frame Length (4 bytes)
├─ Message Type (2 bytes): Table ID from schema
├─ Flags (1 byte): Compression, priority, etc.
├─ Checksum (0-8 bytes): Optional, algorithm-dependent
└─ Payload (N bytes): FlatBuffer message

### 3.2. Schema Registry Integration

```rust
// New trait for schema-aware serialization
pub trait SchemaAwareSerialize {
    /// Returns the schema identifier for this type
    fn schema_id() -> SchemaId;
    
    /// Returns the message type within the schema
    fn message_type() -> MessageType;
    
    /// Validates the message against its schema
    fn validate(&self) -> Result<()>;
    
    /// Serializes with schema metadata
    fn serialize_with_schema<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()>;
}

// Schema registry for runtime validation
pub struct SchemaRegistry {
    schemas: HashMap<SchemaId, Schema>,
    validators: HashMap<SchemaId, Box<dyn SchemaValidator>>,
}

impl SchemaRegistry {
    pub fn register_schema(&mut self, schema: Schema) -> Result<()> {
        // Register schema and create validator
    }
    
    pub fn validate_message(&self, schema_id: SchemaId, payload: &[u8]) -> Result<()> {
        // Runtime schema validation
    }
}
```

### 3.3. Type-Safe Stream Writer

```rust
pub struct SchemaAwareWriter<W: Write, F: Framer> {
    writer: W,
    framer: F,
    registry: Arc<SchemaRegistry>,
    schema_written: bool,
}

impl<W: Write, F: Framer> SchemaAwareWriter<W, F> {
    pub fn new(writer: W, framer: F, registry: Arc<SchemaRegistry>) -> Self {
        Self {
            writer,
            framer,
            registry,
            schema_written: false,
        }
    }
    
    /// Write a schema-aware message with compile-time type safety
    pub fn write_typed<T>(&mut self, message: &T) -> Result<()> 
    where 
        T: SchemaAwareSerialize + flatbuffers::Follow<'static>
    {
        // Write header on first message
        if !self.schema_written {
            self.write_header(T::schema_id())?;
            self.schema_written = true;
        }
        
        // Validate against schema
        message.validate()?;
        
        // Frame with message type metadata
        let frame = MessageFrame {
            message_type: T::message_type(),
            flags: 0,
        };
        
        // Serialize and write
        let mut builder = FlatBufferBuilder::new();
        message.serialize_with_schema(&mut builder)?;
        self.framer.frame_and_write_typed(
            &mut self.writer,
            &frame,
            builder.finished_data()
        )?;
        
        Ok(())
    }
}
```

### 3.4. Type-Safe Stream Reader with Field-Level Access

```rust
pub struct SchemaAwareReader<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
    registry: Arc<SchemaRegistry>,
    schema_id: Option<SchemaId>,
    buffer: Vec<u8>,
}

impl<R: Read, D: Deframer> SchemaAwareReader<R, D> {
    /// Process messages with compile-time type checking
    pub fn process_typed<T, F>(&mut self, mut processor: F) -> Result<()>
    where
        T: SchemaAwareSerialize + flatbuffers::Follow<'static>,
        F: FnMut(&T) -> Result<()>,
    {
        // Verify schema compatibility
        self.verify_schema::<T>()?;
        
        loop {
            match self.read_typed_message::<T>()? {
                Some(message) => processor(&message)?,
                None => break,
            }
        }
        Ok(())
    }
    
    /// Field-level filtering without full deserialization
    pub fn filter_by_field<T, V, F>(
        &mut self,
        field_accessor: fn(&T) -> V,
        predicate: F,
    ) -> Result<Vec<T>>
    where
        T: SchemaAwareSerialize + flatbuffers::Follow<'static>,
        V: PartialEq,
        F: Fn(&V) -> bool,
    {
        let mut results = Vec::new();
        
        self.process_typed::<T, _>(|message| {
            // Zero-copy field access
            let field_value = field_accessor(message);
            if predicate(&field_value) {
                results.push(message.clone());
            }
            Ok(())
        })?;
        
        Ok(results)
    }
}
```

## 4. Implementation Examples

### 4.1. Schema Definition (.fbs file)

```fbs
// telemetry.fbs
namespace telemetry;

table TelemetryEvent {
    device_id: uint64;
    timestamp: uint64;
    temperature: float;
    pressure: float;
    battery_level: float;
}

table AlertEvent {
    device_id: uint64;
    timestamp: uint64;
    alert_type: string;
    severity: uint8;
}

root_type TelemetryEvent;
```

### 4.2. Generated Rust Code with Schema-Aware Traits

```rust
// Generated by flatc with schema-aware extensions
impl SchemaAwareSerialize for TelemetryEvent<'_> {
    fn schema_id() -> SchemaId {
        SchemaId::from_bytes(b"telemetry.fbs:v1.0.0")
    }
    
    fn message_type() -> MessageType {
        MessageType::TelemetryEvent
    }
    
    fn validate(&self) -> Result<()> {
        // Auto-generated validation logic
        if self.device_id() == 0 {
            return Err(Error::ValidationError("device_id cannot be zero"));
        }
        Ok(())
    }
    
    fn serialize_with_schema<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        // Standard FlatBuffer serialization
        let event = telemetry::TelemetryEvent::create(builder, &self.args());
        builder.finish(event, None);
        Ok(())
    }
}
```

### 4.3. User Experience: Type-Safe Streaming

```rust
fn main() -> Result<()> {
    // Schema registry setup
    let registry = Arc::new(SchemaRegistry::from_fbs_files(&[
        "schemas/telemetry.fbs",
        "schemas/alerts.fbs",
    ])?);
    
    // Writing with compile-time type safety
    let file = File::create("telemetry_typed.fbst")?;
    let mut writer = SchemaAwareWriter::new(file, DefaultFramer, registry.clone());
    
    // Type-safe writes - compiler ensures correct types
    let telemetry = TelemetryEvent {
        device_id: 12345,
        timestamp: SystemTime::now().as_nanos(),
        temperature: 23.5,
        pressure: 1013.25,
        battery_level: 87.3,
    };
    writer.write_typed(&telemetry)?;
    
    // Reading with field-level filtering
    let file = File::open("telemetry_typed.fbst")?;
    let mut reader = SchemaAwareReader::new(file, DefaultDeframer, registry);
    
    // Filter by temperature without deserializing other fields
    let hot_events = reader.filter_by_field(
        |event| event.temperature(),
        |temp| *temp > 30.0
    )?;
    
    println!("Found {} events with temperature > 30°C", hot_events.len());
    Ok(())
}
```

## 5. Advanced Features

### 5.1. Schema Evolution Support

```rust
pub struct SchemaEvolution {
    /// Automatic field mapping between schema versions
    pub fn migrate_message<T1, T2>(
        &self,
        old_message: &T1,
        new_schema: &Schema,
    ) -> Result<T2>
    where
        T1: SchemaAwareSerialize,
        T2: SchemaAwareSerialize,
    {
        // Automatic field mapping based on schema compatibility rules
        // - New fields get default values
        // - Removed fields are ignored
        // - Type promotions handled automatically
    }
}

// Usage: Seamless reading of old format with new code
reader.process_typed_with_migration::<TelemetryEventV2, _>(|event| {
    // Old messages automatically migrated to new schema
    println!("GPS coords: {:?}", event.gps_coordinates()); // New field
    Ok(())
})?;
```

### 5.2. Streaming Analytics with Zero-Copy Field Access

```rust
pub struct StreamingAnalytics<R: Read, D: Deframer> {
    reader: SchemaAwareReader<R, D>,
}

impl<R: Read, D: Deframer> StreamingAnalytics<R, D> {
    /// Compute statistics on specific fields without full deserialization
    pub fn field_statistics<T, V>(
        &mut self,
        field_accessor: fn(&T) -> V,
    ) -> Result<FieldStats<V>>
    where
        T: SchemaAwareSerialize + flatbuffers::Follow<'static>,
        V: Copy + PartialOrd + Into<f64>,
    {
        let mut stats = FieldStats::new();
        
        self.reader.process_typed::<T, _>(|message| {
            // Zero-copy field access
            let value = field_accessor(message);
            stats.update(value);
            Ok(())
        })?;
        
        Ok(stats)
    }
    
    /// Time-windowed aggregation
    pub fn windowed_aggregation<T, V, A>(
        &mut self,
        window_size: Duration,
        timestamp_accessor: fn(&T) -> u64,
        value_accessor: fn(&T) -> V,
        aggregator: A,
    ) -> Result<Vec<WindowResult<V>>>
    where
        T: SchemaAwareSerialize + flatbuffers::Follow<'static>,
        V: Copy,
        A: Fn(&[V]) -> V,
    {
        // Efficient windowed processing without buffering entire messages
    }
}
```

### 5.3. Multi-Schema Streams

```rust
// Support for heterogeneous message types in a single stream
pub enum StreamMessage {
    Telemetry(TelemetryEvent),
    Alert(AlertEvent),
    Diagnostic(DiagnosticEvent),
}

impl SchemaAwareReader {
    /// Process multiple message types with pattern matching
    pub fn process_multi<F>(&mut self, mut processor: F) -> Result<()>
    where
        F: FnMut(StreamMessage) -> Result<()>,
    {
        loop {
            let message_type = self.peek_message_type()?;
            
            match message_type {
                MessageType::TelemetryEvent => {
                    let event = self.read_typed::<TelemetryEvent>()?;
                    processor(StreamMessage::Telemetry(event))?;
                }
                MessageType::AlertEvent => {
                    let alert = self.read_typed::<AlertEvent>()?;
                    processor(StreamMessage::Alert(alert))?;
                }
                // ... other types
                None => break,
            }
        }
        Ok(())
    }
}
```

## 6. Performance Considerations

### 6.1. Zero-Copy Field Access

The schema-aware design maintains zero-copy performance through:
- Direct field accessors that read from the underlying buffer
- No intermediate deserialization for filtered fields
- Lazy parsing of complex fields (strings, vectors)

### 6.2. Compile-Time Optimizations

- Generic specialization for known message types
- Inlined field accessors for hot paths
- Schema validation only on debug builds (optional)

### 6.3. Benchmark Targets

| Operation | v2.5 Baseline | v3.0 Target | Notes |
|-----------|---------------|-------------|-------|
| Type-safe write | 1.8µs | 1.9µs | +5% for type safety |
| Type-safe read | 482ns | 500ns | +4% for validation |
| Field filtering | N/A | 100ns | New capability |
| Schema migration | N/A | 1µs | New capability |

## 7. Migration Path from v2.5

### 7.1. Backward Compatibility Mode

```rust
// Support reading v2.5 streams
pub struct HybridReader<R: Read, D: Deframer> {
    inner: SchemaAwareReader<R, D>,
    legacy_mode: bool,
}

impl<R: Read, D: Deframer> HybridReader<R, D> {
    pub fn new(reader: R, deframer: D) -> Result<Self> {
        let mut hybrid = Self {
            inner: SchemaAwareReader::new(reader, deframer, registry),
            legacy_mode: false,
        };
        
        // Auto-detect format
        hybrid.legacy_mode = !hybrid.detect_v3_header()?;
        Ok(hybrid)
    }
}
```

### 7.2. Migration Tool

```bash
# Command-line tool to upgrade v2.5 streams
flatstream-migrate \
    --input legacy.bin \
    --output modern.fbst \
    --schema telemetry.fbs \
    --validate
```

## 8. Benefits for Official FlatBuffers Repository Inclusion

This schema-aware design addresses the previous concerns about inclusion:

1. **FlatBuffers-Native**: Deeply integrates with FlatBuffer schemas and tooling
2. **Cross-Language Potential**: Schema-based approach enables consistent implementations
3. **Unique Value**: Provides streaming capabilities that complement core FlatBuffers
4. **Type Safety**: Compile-time guarantees that align with FlatBuffers philosophy
5. **Evolution Support**: Leverages FlatBuffers' schema evolution capabilities

## 9. Implementation Phases

### Phase 1: Core Schema Integration (2 weeks)
- Schema registry implementation
- Enhanced stream format with headers
- Basic type-safe read/write APIs

### Phase 2: Advanced Features (3 weeks)
- Field-level filtering
- Schema evolution support
- Multi-schema streams

### Phase 3: Performance Optimization (1 week)
- Benchmark validation
- Zero-copy optimizations
- Compile-time specializations

### Phase 4: Tooling and Migration (1 week)
- Schema compiler integration
- Migration tools
- Documentation and examples

## 10. Success Criteria

The v3.0 implementation will be considered successful when:

✅ **Type Safety**: Compile-time guarantees for message types  
✅ **Schema Evolution**: Seamless handling of schema changes  
✅ **Performance**: <5% overhead compared to v2.5 for basic operations  
✅ **Field Operations**: Sub-microsecond field-level filtering  
✅ **Developer Experience**: Simpler, safer API than v2.5  
✅ **FlatBuffers Integration**: Native support for .fbs schemas  

---

*The v3.0 "Schema-Aware" design transforms flatstream-rs from a generic streaming wrapper into a FlatBuffers-native solution that provides unique value worthy of inclusion in the official repository.*
