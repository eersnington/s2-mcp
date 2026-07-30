# Understand the Code Mode runtime

The `s2-mcp-codemode` crate generates the TypeScript API, transpiles submitted code, and runs it in an isolated V8 child process.

## Runtime behavior

Each `execute` request starts a child process with an empty inherited environment. Cancellation or timeout terminates the child.

Code Mode rejects static and dynamic imports. It does not expose the host process, environment, filesystem, network, or WebAssembly.

TypeScript is parsed and transpiled without semantic type checking. Rust validates each S2 call before sending it to `s2-sdk`.

## Execution limits

Each request has these limits:

| Limit | Value |
| --- | ---: |
| Execution time | 30s |
| Source | 64 KiB |
| S2 calls | 32 |
| Concurrent S2 calls | 8 |
| Serialized output | 256 KiB |
| JSON nesting depth | 32 |
| V8 heap | 128 MiB |
