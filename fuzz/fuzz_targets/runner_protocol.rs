//! Raw-byte hardening for the isolated-runner framing boundary.
//!
//! The same arbitrary bytes are attempted as a request and as a response:
//! accepted values must round-trip through the canonical writer, while all
//! malformed/truncated/oversized frames are ordinary rejection paths.

#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use rsscript_runner_protocol::{
    read_request, read_response, write_request, write_response,
};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 * 1024 {
        return;
    }

    if let Ok((request, bundle)) = read_request(Cursor::new(data)) {
        let mut canonical = Vec::new();
        write_request(&mut canonical, &request, &bundle)
            .expect("a validated runner request must serialize");
        read_request(Cursor::new(canonical))
            .expect("a canonical runner request must remain valid");
    }

    if let Ok(response) = read_response(Cursor::new(data)) {
        let mut canonical = Vec::new();
        write_response(&mut canonical, &response)
            .expect("a validated runner response must serialize");
        read_response(Cursor::new(canonical))
            .expect("a canonical runner response must remain valid");
    }
});
