# Connect AI agents to S2.dev

`s2-mcp` connects AI agents to [S2](https://s2.dev), a durable stream store, through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io/). Agents can manage basins and streams, append records, and read stored or live data.

## Install

Run the published package without a global installation:

```sh
npx --yes s2-mcp
```

Or clone the repository and install from source:

```sh
git clone https://github.com/eersnington/s2-mcp.git
cd s2-mcp
just install 
# or 
cargo install --path crates/s2-mcp --locked --force
```

Install [`just`](https://just.systems/) to to be able to run this workspace commands easily.

```sh
s2-mcp --version
```

For local development use `just mcp` instead of `s2-mcp`.

## Configure an MCP client

Configure the MCP to your preferred agent client like Codex, Claude Code, OpenCode, Pi, etc. Add two separate MCP servers for cloud and development:


```json
{
  "s2-cloud": {
    "command": "npx",
    "args": ["--yes", "s2-mcp"]
  },
  "s2-dev": {
    "command": "npx",
    "args": ["--yes", "s2-mcp", "--dev"]
  }
}
```

Or if you have it installed from source:

```json
{
  "s2-cloud": {
    "command": "s2-mcp"
  },
  "s2-dev": {
    "command": "s2-mcp",
    "args": ["--dev"]
  }
}
```

Cloud mode reads credentials saved by the [S2 CLI](https://s2.dev/docs/cli/configuration). 

Dev mode starts a temporary S2 Lite container when needed and reuses it for the MCP process lifetime. Docker or another compatible runtime like OrbStack or Apple Containers is required.

## Modes

Code Mode is the default. The agent searches the S2 API, then runs a TypeScript program:

```typescript
async function run() {
  return await S2.listStreams({ basin: "example-basin" });
}
```

If your agent client has it's own code mode executor, or if you use a specialized MCP executor, then you can use tools mode instead which exposes all S2 operations as MCP tools and disable code mode.

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

- **Default** (`s2-mcp`): uses S2 Cloud defaults. Reads the S2 CLI config. Honors `S2_ACCESS_TOKEN`.
- **Dev** (`s2-mcp --dev`): starts a temporary S2 Lite container when needed and reuses it for the process lifetime. Requires Docker or a compatible runtime.
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
