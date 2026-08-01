# Configure and develop s2-mcp

The `s2-mcp` crate contains the MCP server, S2 operations, access policy, CLI, and Code Mode subprocess coordinator.

## Configure S2

Cloud mode reads credentials saved by the [S2 CLI](https://s2.dev/docs/cli/configuration). `S2_ACCESS_TOKEN` may override the saved token. Endpoint environment variables never change a Cloud connection.

Use explicit development mode for S2 Lite or custom deployments:

```sh
s2-mcp --dev
s2-mcp --dev --endpoint http://127.0.0.1:8080
S2_ACCOUNT_ENDPOINT=http://account.internal:8080 \
S2_BASIN_ENDPOINT=http://basin.internal:8081 \
s2-mcp --dev --from-env
```

Managed development starts a temporary S2 Lite container through Testcontainers when needed, then keeps it for the MCP process lifetime. Docker or another compatible runtime is required. `--endpoint` uses one server for both APIs. `--from-env` requires both endpoint variables and never falls back to Cloud. `S2_ACCESS_TOKEN` supplies a development token when the endpoint requires one.

`S2_ENCRYPTION_KEY`, `S2_COMPRESSION`, and `S2_SSL_NO_VERIFY` configure Cloud connections. `S2_COMPRESSION` accepts `none`, `gzip`, or `zstd`.

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

The S2 Lite test requires Docker or another compatible container runtime:

```sh
cargo test --locked -p s2-mcp --test s2_lite -- --ignored
```

## Embed the server

Call the library entry point from a Rust binary:

```rust
s2_mcp::serve(options, configuration).await?;
```

Code Mode starts the current executable with the hidden `__execute` command. An embedding binary must route that command to `s2_mcp::run_executor_child()`. Tools Mode does not use a child process. The child limits accidental host access and contains runtime failures; it is not an operating-system security sandbox.
