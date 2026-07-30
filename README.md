# Connect AI agents to S2

`s2-mcp` connects AI agents to [S2](https://s2.dev), a durable stream store, through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io/). Agents can manage basins and streams, append records, and read stored or live data.

## Start the server

Install Rust 1.89 or newer and [`just`](https://just.systems/).

Set your S2 access token and start the server:

```sh
export S2_ACCESS_TOKEN=your_access_token_here
just mcp
```

Build a binary for your MCP client:

```sh
just build
```

Configure the client to run `target/debug/s2-mcp` by its absolute path.

## Choose a mode

In Code Mode, the agent searches the S2 API and writes a TypeScript program. Code Mode is the default:

```typescript
async function run() {
  return await S2.listStreams({ basin: "example-basin" });
}
```

In Tools Mode, the client receives one MCP tool for each S2 operation:

```sh
just mcp --mode tools
```

Both modes enforce the same access policy.

## Limit access

Pass flags when you start the server:

```sh
just mcp --readonly
just mcp --basin example-basin
just mcp --allow-destructive
```

`--readonly` hides mutating operations. `--basin` restricts access to one basin. Destructive operations remain hidden unless you pass `--allow-destructive`.

Run `just mcp --help` for all options.

## Read more

- [Server configuration and development](crates/s2-mcp/README.md)
- [Code Mode runtime and limits](crates/s2-mcp-codemode/README.md)

## License

MIT
