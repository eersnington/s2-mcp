# Configure and develop s2-mcp

The `s2-mcp` crate contains the MCP server, S2 operations, access policy, CLI, and Code Mode subprocess coordinator.

## Configure S2

The server reads credentials saved by the [S2 CLI](https://s2.dev/docs/cli/configuration). You can override the saved configuration with these environment variables:

```text
S2_ACCESS_TOKEN
S2_ACCOUNT_ENDPOINT
S2_BASIN_ENDPOINT
S2_ENCRYPTION_KEY
S2_COMPRESSION
S2_SSL_NO_VERIFY
```

Set both endpoint variables to use custom endpoints. `S2_ENCRYPTION_KEY` applies to encrypted append and read operations. `S2_COMPRESSION` accepts `none`, `gzip`, or `zstd`.

## Run development checks

Run the project recipes from the workspace root:

```sh
just fmt
just check
just clippy
just test
just deny
```

The production timeout test takes about 30 seconds, so the regular test recipe skips it. Run it explicitly:

```sh
cargo test --locked -p s2-mcp --test protocol \
  executor_terminates_infinite_program_at_production_deadline \
  -- --ignored --exact
```

The S2 Lite test requires `s2-lite` in `PATH`:

```sh
cargo test --locked -p s2-mcp --test s2_lite -- --ignored
```

## Embed the server

Call the library entry point from a Rust binary:

```rust
s2_mcp::serve(options, configuration).await?;
```

Code Mode starts the current executable with the hidden `__execute` command. An embedding binary must route that command to `s2_mcp::run_executor_child()`. Tools Mode does not use a child process.
