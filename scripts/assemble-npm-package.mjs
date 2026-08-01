#!/usr/bin/env node

/**
 * Builds the npm package directory with the release binaries.
 * Used by the `package-npm` job in `.github/workflows/release.yml`.
 */

import fs from "node:fs"
import path from "node:path"

const [version, nativeDirectory, outputDirectory] = process.argv.slice(2)
if (!version || !nativeDirectory || !outputDirectory) {
  throw new Error("usage: node scripts/assemble-npm-package.mjs <version> <native-directory> <output-directory>")
}

const packageDirectory = "npm/s2-mcp"
const manifest = JSON.parse(fs.readFileSync(path.join(packageDirectory, "package.json"), "utf8"))
if (manifest.version !== version) {
  throw new Error(`${manifest.name} version ${manifest.version} does not match ${version}`)
}

fs.cpSync(packageDirectory, outputDirectory, {
  filter(source) {
    return path.basename(source) !== "prebuilds"
  },
  recursive: true,
})
fs.copyFileSync("LICENSE", path.join(outputDirectory, "LICENSE"))

for (const [platform, executable] of Object.entries({
  "darwin-arm64": "s2-mcp",
  "darwin-x64": "s2-mcp",
  "linux-x64": "s2-mcp",
  "win32-x64": "s2-mcp.exe",
})) {
  const source = path.join(nativeDirectory, platform, executable)
  if (!fs.existsSync(source)) throw new Error(`missing ${platform} release executable at ${source}`)
  const destination = path.join(outputDirectory, "prebuilds", platform, executable)
  fs.mkdirSync(path.dirname(destination), { recursive: true })
  fs.copyFileSync(source, destination)
  if (platform !== "win32-x64") fs.chmodSync(destination, 0o755)
}
