#!/usr/bin/env node

import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"

const executables = {
  "darwin-arm64": "../prebuilds/darwin-arm64/s2-mcp",
  "darwin-x64": "../prebuilds/darwin-x64/s2-mcp",
  "linux-x64": "../prebuilds/linux-x64/s2-mcp",
  "win32-x64": "../prebuilds/win32-x64/s2-mcp.exe",
}

const platform = `${process.platform}-${process.arch}`
const executable = executables[platform]
if (!executable) {
  console.error(
    `s2-mcp does not support ${platform}. Supported platforms: ${Object.keys(executables).join(", ")}.`,
  )
  process.exitCode = 1
} else {
  const child = spawn(fileURLToPath(new URL(executable, import.meta.url)), process.argv.slice(2), {
    env: process.env,
    stdio: "inherit",
    windowsHide: true,
  })

  child.on("error", (error) => {
    console.error(`s2-mcp could not start its ${platform} executable: ${error.message}`)
    process.exitCode = 1
  })
  child.on("exit", (code, signal) => {
    if (signal) process.kill(process.pid, signal)
    else process.exitCode = code ?? 1
  })
}
