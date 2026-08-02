#!/usr/bin/env node

import fs from "node:fs"
import path from "node:path"

const [executable, platform, output] = process.argv.slice(2)
if (!executable) {
  throw new Error("usage: node scripts/check-binary-size.mjs <executable> [platform] [output.json]")
}

const size = fs.statSync(executable).size
const measurement = {
  platform: platform ?? executable,
  bytes: size,
  megabytes: size / 1_000_000,
  mebibytes: size / 1024 / 1024,
}
console.log(
  `${measurement.platform}: ${size.toLocaleString("en-US")} bytes (${measurement.megabytes.toFixed(2)} MB, ${measurement.mebibytes.toFixed(2)} MiB)`,
)
if (output) {
  fs.mkdirSync(path.dirname(output), { recursive: true })
  fs.writeFileSync(output, `${JSON.stringify(measurement, null, 2)}\n`)
}
