#![no_main]

use libfuzzer_sys::fuzz_target;

// Builds a text and change sets from the input and checks every ChangeSet
// invariant; see flash_core::fuzzing::changeset_ops.
fuzz_target!(|data: &[u8]| {
    flash_core::fuzzing::changeset_ops(data);
});
