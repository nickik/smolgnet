# x4c / P4 `no_std` plan

## Goal

Keep the P4 program as the source of truth while allowing a smolgnet build to use x4c-generated GDP parsing under `#![no_std]` with `alloc`.

The x4c compiler and `p4-macro` are build-time tools and may continue to require a hosted Rust environment. The runtime code emitted by x4c is what must become `core`/`alloc` compatible.

## Current status

The pinned Oxide x4c/p4rs revision is hosted-only. The current smolgnet `p4-gdp` feature therefore implies `std`. Handwritten GDP remains available for all `no_std` builds.

This is an upstream/runtime limitation rather than a GDP limitation.

## Concrete blockers in the pinned x4c runtime

1. `p4rs` imports `std::fmt`, `std::net`, `std::hash`, and standard collections directly.
2. `p4rs::packet_out`, table entries, and the pipeline API use `Vec`/`String` without an `alloc` abstraction.
3. `p4rs` unconditionally includes USDT tracing.
4. x4c Rust codegen emits `std::sync::Arc` for table actions.
5. generated SoftNPU pipeline construction calls `usdt::register_probes()`.
6. generated code uses `Box`, `Vec`, `vec!`, and `format!` as hosted-prelude assumptions.
7. some helper APIs are tied to `std::net::IpAddr` even though GDP does not need them.

## Preferred implementation

Add an `alloc` runtime mode to x4c/p4rs rather than copying generated GDP Rust into smolgnet.

### p4rs

- add `#![cfg_attr(not(feature = "std"), no_std)]`
- add `extern crate alloc` for the alloc runtime
- make `std` a default feature
- use `core::fmt`, `core::hash`, and other `core` equivalents
- use `alloc::{boxed::Box, string::String, sync::Arc, vec::Vec}`
- gate `std::net` conversion helpers behind `feature = "std"`
- gate USDT support behind a hosted tracing feature
- replace `std::collections` in the data-plane table implementation with an alloc-capable representation or provide a small-table implementation for `no_std`

### x4c Rust codegen

Generated code must not name `std` directly when targeting the alloc runtime.

- emit `alloc::sync::Arc` rather than `std::sync::Arc`
- emit/import `alloc::boxed::Box`, `alloc::vec::Vec`, `alloc::string::String`
- make tracing/probe emission optional
- do not call `usdt::register_probes()` in an alloc-only target
- allow generated packet parser code to be built without the dynamic C pipeline constructor if the target does not need it

## Useful intermediate target: parser-only alloc backend

For smolgnet endpoints we do not need the complete SoftNPU forwarding runtime merely to decode GDP. A useful first upstream x4c target is therefore:

```text
P4 parser
   |
x4c build-time compiler
   |
generated Rust parser
   |
core + alloc only
```

It should still be generated from `gdp.p4`; we should not maintain a second handwritten generated-parser snapshot.

The full table/forwarding SoftNPU backend can gain `no_std + alloc` afterward.

## Acceptance tests

A no-std P4 backend is complete only when all of these pass:

```sh
cargo check --no-default-features --features alloc,p4-gdp
cargo test --features p4-gdp
cargo test --features p4-gdp-compare
```

For a target that cannot execute hosted tests, compile a small `no_std` fixture that parses known global and local GDP vectors and link it for a bare target such as `thumbv7em-none-eabihf` or an equivalent target available in CI.

The same GDP differential corpus must continue to pass in hosted mode so the no-std runtime does not create a second interpretation of the protocol.

## Non-goals

- x4c itself does not need to become `no_std`; it is a compiler.
- smolgnet will not vendor generated GDP Rust as the normative implementation.
- GDP wire semantics will not change to accommodate x4c runtime limitations.
