#!/usr/bin/env node

/**
 * Installs a packed npm package and checks that its CLI starts correctly.
 * Used by the `smoke-npm` job in `.github/workflows/release.yml`.
 */

import { execFileSync, spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

const [archive, version] = process.argv.slice(2)
if (!archive || !version) {
  throw new Error("usage: node scripts/smoke-npm-package.mjs <archive> <version>")
}

const npm = process.platform === "win32" ? "npm.cmd" : "npm"
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "s2-mcp-npm-smoke-"))
try {
  execFileSync(npm, ["install", "--ignore-scripts", "--no-save", "--prefix", directory, archive], {
    stdio: "inherit",
  })
  const launcher = path.join(directory, "node_modules", "s2-mcp", "bin", "s2-mcp.mjs")
  const result = spawnSync(process.execPath, [launcher, "--version"], { encoding: "utf8" })
  if (result.error) throw result.error
  if (result.status !== 0) {
    throw new Error(`packaged s2-mcp --version failed: ${result.stderr}`)
  }
  const expectedVersion = `s2-mcp ${version}`
  if (result.stdout.trim() !== expectedVersion) {
    throw new Error(`packaged s2-mcp reported ${JSON.stringify(result.stdout.trim())}, expected ${expectedVersion}`)
  }
} finally {
  fs.rmSync(directory, { force: true, recursive: true })
}
