#!/usr/bin/env node

/**
 * Checks that the release tag, Rust package, and npm packages share one version.
 * Used by the `validate` job in `.github/workflows/release.yml`.
 */

import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"

const [tag] = process.argv.slice(2)
if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag ?? "")) {
  throw new Error("release tag must use the vX.Y.Z format")
}

const version = tag.slice(1)
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
  }),
)
const server = metadata.packages.find((packageMetadata) => packageMetadata.name === "s2-mcp")
if (!server) throw new Error("Cargo workspace does not contain the s2-mcp package")
if (server.version !== version) {
  throw new Error(`release tag ${tag} does not match s2-mcp version ${server.version}`)
}

for (const entry of fs.readdirSync("npm")) {
  const manifestPath = path.join("npm", entry, "package.json")
  if (!fs.existsSync(manifestPath)) continue
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"))
  if (manifest.version !== version) {
    throw new Error(`${manifest.name} version ${manifest.version} does not match ${version}`)
  }
  for (const [dependency, dependencyVersion] of Object.entries(manifest.optionalDependencies ?? {})) {
    if (dependencyVersion !== version) {
      throw new Error(`${manifest.name} optional dependency ${dependency} version ${dependencyVersion} does not match ${version}`)
    }
  }
}
