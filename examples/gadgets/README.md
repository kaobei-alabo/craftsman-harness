# Gadget examples

- `word-count`: pure typed computation, no capabilities.
- `workspace-stats`: workspace read through the host, no write or HTTP access.
- `review-flow`: data-only commands/agents/prompt; the migrated legacy plugin.
  It needs no component or compiler.

The two Rust packages build in this repository with the pinned stable toolchain:

```sh
cargo build --locked --release --target wasm32-wasip2 \
  --manifest-path examples/gadgets/word-count/Cargo.toml
```

To install an example elsewhere, first scaffold it with `craftsman gadgets new
<id>`, then copy its `src/lib.rs` and `gadget.toml` into that scaffold. Keep the
scaffold's generated Cargo manifest and lockfile: its dependency points at the
SDK embedded in your Craftsman binary, not this repository checkout. Run `craftsman
gadgets build <id>`. First-load approval is separate; an agent does not need to
poll `gadget_list` after the build because catalog discovery is deterministic.
