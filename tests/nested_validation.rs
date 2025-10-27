use flatbuffers::FlatBufferBuilder;
use flatstream::{TableRootValidator, Validator};

fn build_nested_empty_tables_bytes(depth: usize) -> Vec<u8> {
    assert!(depth >= 1, "depth must be >= 1");
    let mut b = FlatBufferBuilder::new();

    // Build from leaf to root
    let mut current: Option<flatbuffers::WIPOffset<flatbuffers::Table<'_>>> = None;
    for _ in 0..depth {
        let start = b.start_table();
        if let Some(child) = current {
            // Mirrored from generated code: first field vtable offset is 4 (e.g., VT_* = 4).
            // We use that slot to store a child table offset to create nesting.
            b.push_slot_always::<flatbuffers::WIPOffset<_>>(4, child);
        }
        let this_table = b.end_table(start);
        let as_table: flatbuffers::WIPOffset<flatbuffers::Table<'_>> =
            flatbuffers::WIPOffset::new(this_table.value());
        current = Some(as_table);
    }

    let root = current.expect("depth>=1 ensures a root table");
    b.finish(root, None);
    b.finished_data().to_vec()
}

#[test]
fn table_root_validator_accepts_nested_when_limits_high() {
    let buf = build_nested_empty_tables_bytes(32);
    let v = TableRootValidator::with_limits(128, 1_000_000);
    assert!(v.validate(&buf).is_ok());
}

#[test]
fn table_root_validator_does_not_traverse_children_under_low_limits() {
    // TableRootValidator is type-agnostic; it validates the root table structure
    // but does not traverse nested tables without schema information. Therefore,
    // even very strict limits do not cause rejection for nested payloads here.
    let buf = build_nested_empty_tables_bytes(32);
    let v = TableRootValidator::with_limits(2, 2);
    assert!(v.validate(&buf).is_ok());
}


