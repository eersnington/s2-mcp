#!/usr/bin/env node

import { spawn } from "node:child_process"
import { createRequire } from "node:module"

const nativePackages = {
  "darwin-arm64": "@eersnington/s2-mcp-darwin-arm64/bin/s2-mcp",
  "darwin-x64": "@eersnington/s2-mcp-darwin-x64/bin/s2-mcp",
  "linux-x64": "@eersnington/s2-mcp-linux-x64/bin/s2-mcp",
  "win32-x64": "@eersnington/s2-mcp-win32-x64/bin/s2-mcp.exe",
}

const platform = `${process.platform}-${process.arch}`
const nativePackage = nativePackages[platform]
if (!nativePackage) {
  console.error(
    `s2-mcp does not support ${platform}. Supported platforms: ${Object.keys(nativePackages).join(", ")}.`,
  )
  process.exitCode = 1
} else {
  let executable
  try {
    executable = createRequire(import.meta.url).resolve(nativePackage)
  } catch {
    console.error(
      `s2-mcp could not find its ${platform} executable. Reinstall s2-mcp without --no-optional so npm can install the native package.`,
    )
    process.exit(1)
  }

  const child = spawn(executable, process.argv.slice(2), {
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
