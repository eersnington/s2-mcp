# Connect AI agents to S2

`s2-mcp` connects AI agents to [S2](https://s2.dev), a durable stream store, through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io/). Agents can manage basins and streams, append records, and read stored or live data.

## Install

Install Rust 1.89 or newer and [`just`](https://just.systems/), then install from the workspace:

```sh
just install
```

This runs `cargo install` and places `s2-mcp` in Cargo’s global binary directory, normally `~/.cargo/bin`. Confirm that directory is on your `PATH`:

```sh
s2-mcp --version
```

Run `just install` again after you pull changes to replace the installed binary. For local development without installing, use `just mcp` instead of `s2-mcp`.

## Configure an MCP client

Add Cloud and development as separate MCP servers:

```json
{
  "s2": {
    "command": "s2-mcp"
  },
  "s2-dev": {
    "command": "s2-mcp",
    "args": ["--dev"]
  }
}
```

Cloud mode reads credentials saved by the [S2 CLI](https://s2.dev/docs/cli/configuration). Managed development starts a temporary S2 Lite container when needed and reuses it for the MCP process lifetime. Docker or another compatible runtime is required.

`s2-mcp` is an MCP stdio server. Launch it from an MCP client. Running it in a terminal prints help and exits.

## Modes

Code Mode is the default. The agent searches the S2 API, then runs a TypeScript program:

```typescript
async function run() {
  return await S2.listStreams({ basin: "example-basin" });
}
```

Tools Mode exposes one MCP tool per S2 operation. Both modes enforce the same access policy. Set the mode in your MCP client config:

```json
{
  "s2": {
    "command": "s2-mcp",
    "args": ["--mode", "tools"]
  }
}
```

## CLI reference

```text
s2-mcp [OPTIONS]
```

### Connections

Choose a connection when the process starts. It cannot switch later.

- **Cloud** (`s2-mcp`): uses S2 Cloud defaults. Reads the S2 CLI config. Honors `S2_ACCESS_TOKEN`. Ignores endpoint environment variables.
- **Managed development** (`s2-mcp --dev`): starts a temporary S2 Lite container when needed and reuses it for the process lifetime. Requires Docker or a compatible runtime.
- **Existing endpoint** (`s2-mcp --dev --endpoint URL`): uses one URL for account and basin APIs. Does not start a container.
- **Environment** (`s2-mcp --dev --from-env`): requires both `S2_ACCOUNT_ENDPOINT` and `S2_BASIN_ENDPOINT`. A partial pair fails. Never falls back to Cloud.

`--endpoint` and `--from-env` require `--dev` and conflict with each other.

### Options

- **`--mode <MODE>`**: `code` (default) or `tools`
- **`--readonly`**: hide operations that mutate state
- **`--basin <BASIN>`**: restrict access to one basin
- **`--allow-destructive`**: advertise destructive operations; has no effect with `--readonly`
- **`--dev`**: use an isolated development connection
- **`--endpoint <URL>`**: with `--dev`, connect both APIs to one existing server
- **`--from-env`**: with `--dev`, read endpoints from the environment
- **`--log-file <PATH>`**: write diagnostic logs to a file
- **`-h`, `--help`**: print help
- **`-V`, `--version`**: print version

### Examples

Read-only Cloud access:

```json
{
  "s2": {
    "command": "s2-mcp",
    "args": ["--readonly"]
  }
}
```

Tools Mode against one basin:

```json
{
  "s2": {
    "command": "s2-mcp",
    "args": ["--mode", "tools", "--basin", "example-basin"]
  }
}
```

Development against a local S2 Lite you already run:

```json
{
  "s2-dev": {
    "command": "s2-mcp",
    "args": ["--dev", "--endpoint", "http://127.0.0.1:8080"]
  }
}
```

Development from environment variables:

```sh
S2_ACCOUNT_ENDPOINT=http://account.internal:8080 \
S2_BASIN_ENDPOINT=http://basin.internal:8081 \
s2-mcp --dev --from-env
```

## Read more

- [Server configuration and development](crates/s2-mcp/README.md)
- [Code Mode runtime and limits](crates/s2-mcp-codemode/README.md)

## License

MIT
