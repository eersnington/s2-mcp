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

const launcherDirectory = path.join(outputDirectory, "s2-mcp")
fs.cpSync(packageDirectory, launcherDirectory, { recursive: true })
fs.copyFileSync("LICENSE", path.join(launcherDirectory, "LICENSE"))

for (const [platform, executable] of Object.entries({
  "darwin-arm64": "s2-mcp",
  "darwin-x64": "s2-mcp",
  "linux-x64": "s2-mcp",
  "win32-x64": "s2-mcp.exe",
})) {
  const source = path.join(nativeDirectory, platform, executable)
  if (!fs.existsSync(source)) throw new Error(`missing ${platform} release executable at ${source}`)
  const nativePackageDirectory = path.join(outputDirectory, `s2-mcp-${platform}`)
  const destination = path.join(nativePackageDirectory, "bin", executable)
  fs.mkdirSync(path.dirname(destination), { recursive: true })
  fs.copyFileSync(source, destination)
  if (platform !== "win32-x64") fs.chmodSync(destination, 0o755)
  fs.copyFileSync("LICENSE", path.join(nativePackageDirectory, "LICENSE"))
  fs.writeFileSync(
    path.join(nativePackageDirectory, "package.json"),
    `${JSON.stringify({
      name: `@eersnington/s2-mcp-${platform}`,
      version,
      description: `Native ${platform} executable for s2-mcp`,
      license: "MIT",
      repository: manifest.repository,
      os: [platform.split("-")[0]],
      cpu: [platform.split("-")[1]],
      files: ["bin", "LICENSE"],
      publishConfig: manifest.publishConfig,
    }, null, 2)}\n`,
  )
}
