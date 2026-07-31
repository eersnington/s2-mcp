# Connect AI agents to S2

`s2-mcp` connects AI agents to [S2](https://s2.dev), a durable stream store, through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io/). Agents can manage basins and streams, append records, and read stored or live data.

## Install

Install Rust 1.89 or newer and [`just`](https://just.systems/), then install `s2-mcp` from the workspace:

```sh
just install
```

This runs `cargo install` and adds the `s2-mcp` binary to Cargo's global binary directory, normally `~/.cargo/bin`. Make sure that directory is in your `PATH`:

```sh
s2-mcp --version
```

Run `just install` again after pulling changes to replace the installed binary.

## Configure an MCP client

Configure Cloud and development as separate MCP servers:

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

Cloud mode reads credentials saved by the S2 CLI. Development mode starts an ephemeral S2 Lite container and requires Docker or another compatible container runtime.

## Choose a mode

In Code Mode, the agent searches the S2 API and writes a TypeScript program. Code Mode is the default:

```typescript
async function run() {
  return await S2.listStreams({ basin: "example-basin" });
}
```

In Tools Mode, the client receives one MCP tool for each S2 operation:

```sh
s2-mcp --mode tools
```

Both modes enforce the same access policy.

## Limit access

Pass flags when you start the server:

```sh
s2-mcp --readonly
s2-mcp --basin example-basin
s2-mcp --allow-destructive
```

`--readonly` hides mutating operations. `--basin` restricts access to one basin. Destructive operations remain hidden unless you pass `--allow-destructive`.

Run `s2-mcp --help` for all options. When developing locally, `just mcp --help` runs the workspace version without installing it first.

## Choose a connection

`s2-mcp` uses S2 Cloud defaults and ignores endpoint environment variables. Development connections are explicit:

```sh
s2-mcp --dev
s2-mcp --dev --endpoint http://127.0.0.1:8080
s2-mcp --dev --from-env
```

Managed `--dev` data is ephemeral. `--endpoint` connects both APIs to one existing server. `--from-env` requires both `S2_ACCOUNT_ENDPOINT` and `S2_BASIN_ENDPOINT`; it never falls back to Cloud.

## Read more

- [Server configuration and development](crates/s2-mcp/README.md)
- [Code Mode runtime and limits](crates/s2-mcp-codemode/README.md)

## License

MIT
