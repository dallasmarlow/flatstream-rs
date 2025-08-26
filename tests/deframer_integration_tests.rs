// tests/deframer_integration_tests.rs

use flatstream::{
    DefaultFramer, Error, SafeTakeDeframer, StreamReader, StreamWriter, UnsafeDeframer,
};
use std::io::Cursor;

// Helper function to create test data.
// No changes needed here.
fn write_test_data(
    writer: &mut StreamWriter<Cursor<&mut Vec<u8>>, DefaultFramer>,
    messages: &[&str],
) {
    for msg in messages {
        writer.write(msg).unwrap();
    }
}

// Macro to run the same set of tests for any given deframer
macro_rules! test_deframer {
    ($deframer_name:ident, $deframer_expr:expr) => {
        mod $deframer_name {
            use super::*;

            #[test]
            fn test_correctness() {
                // Purpose: For a given deframer, confirm we can read back all framed messages
                // and that each payload corresponds to what was written (content check simplified).
                let mut buffer = Vec::new();
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                let messages = vec!["message 1", "another message", "final message"];
                write_test_data(&mut writer, &messages);

                let deframer = $deframer_expr;
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut read_payloads = Vec::<Vec<u8>>::new();

                // Collect all payloads first
                reader
                    .process_all(|payload| {
                        read_payloads.push(payload.to_vec());
                        Ok(())
                    })
                    .unwrap();

                assert_eq!(read_payloads.len(), messages.len());

                // Now, verify the content of each payload
                for (i, original_msg) in messages.iter().enumerate() {
                    let payload_str = String::from_utf8_lossy(&read_payloads[i]);
                    // This is a simplified check. In a real scenario, you would deserialize
                    // the FlatBuffer root and compare fields. For this test, we just
                    // confirm the original string exists within the payload.
                    assert!(payload_str.contains(original_msg));
                }
            }

            #[test]
            fn test_partial_stream_handling() {
                // Purpose: Truncating the stream should cause an UnexpectedEof when reading.
                let mut buffer = Vec::new();
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                write_test_data(&mut writer, &["a complete message"]);

                // Truncate the buffer to simulate a partial stream
                let truncated_len = buffer.len() - 3;
                buffer.truncate(truncated_len);

                let deframer = $deframer_expr;
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let result = reader.process_all(|_| Ok(()));

                assert!(matches!(result, Err(Error::UnexpectedEof)));
            }

            #[test]
            fn test_empty_stream() {
                // Purpose: Empty stream should be handled cleanly by process_all and read_message.
                let buffer = Vec::new();
                let deframer = $deframer_expr;
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let result = reader.process_all(|_| Ok(()));
                assert!(result.is_ok());

                // FIX: Re-create the deframer for the second reader because the first one was moved.
                let deframer2 = $deframer_expr;
                let mut reader2 = StreamReader::new(Cursor::new(&buffer), deframer2);
                assert!(reader2.read_message().unwrap().is_none());
            }

            #[test]
            fn test_corrupted_length_prefix() {
                // Purpose: Corrupting the length field to an absurdly large value should
                // lead to an UnexpectedEof when the deframer attempts to read past the buffer.
                let mut buffer = Vec::new();
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                write_test_data(&mut writer, &["a message"]);

                // Corrupt the length prefix to be huge, pointing beyond the buffer's end
                buffer[0] = 0xFF;
                buffer[1] = 0xFF;
                buffer[2] = 0xFF;
                buffer[3] = 0x7F; // Max positive i32

                let deframer = $deframer_expr;
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let result = reader.process_all(|_| Ok(()));

                // A corrupted length should result in an unexpected EOF when it tries to read
                assert!(matches!(result, Err(Error::UnexpectedEof)));
            }
        }
    };
}

// Run all tests for each of the new deframers
test_deframer!(safe_take_deframer, SafeTakeDeframer);
test_deframer!(unsafe_deframer, UnsafeDeframer);
