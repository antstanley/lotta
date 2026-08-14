//! Composition root for the Lotta server.
//!
//! This binary will assemble concrete adapters and own process lifecycle. Business logic belongs
//! in workspace library crates.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn composition_root_starts_empty() {}
}
