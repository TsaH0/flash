//! The `flash` binary's logic.
//!
//! [`skeleton`] is the Stage 3 walking skeleton: deliberately crude,
//! throwaway UI over the real core APIs. It is rewired onto the headless
//! editor in Stage 4 and replaced by the real terminal editor in Stage 6.

#![forbid(unsafe_code)]

pub mod latency;
pub mod skeleton;
