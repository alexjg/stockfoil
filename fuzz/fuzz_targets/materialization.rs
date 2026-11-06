//! Stateful planner fuzzing against a naive whole-history participation model.
#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| stockfoil_fuzz::planner::exercise(data));
