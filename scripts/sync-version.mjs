#!/usr/bin/env node

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function writeJson(filePath, data) {
  writeFileSync(filePath, `${JSON.stringify(data, null, 2)}\n`, "utf8");
}

function updateTauriVersion(tauriConfigPath, version) {
  const config = readJson(tauriConfigPath);
  if (config.version === version) {
    return false;
  }

  config.version = version;
  writeJson(tauriConfigPath, config);
  return true;
}

function updateCargoVersion(cargoTomlPath, version) {
  const cargoToml = readFileSync(cargoTomlPath, "utf8");
  const updated = cargoToml.replace(
    /^version\s*=\s*"[^"]*"\s*$/m,
    `version = "${version}"`
  );

  if (updated === cargoToml) {
    return false;
  }

  writeFileSync(cargoTomlPath, updated, "utf8");
  return true;
}

function updatePackageLockVersion(packageLockPath, version) {
  if (!existsSync(packageLockPath)) {
    return false;
  }

  const packageLock = readFileSync(packageLockPath, "utf8");
  let updated = packageLock;

  // Top-level lock file version.
  updated = updated.replace(
    /("version"\s*:\s*")([^"]+)(")/,
    `$1${version}$3`
  );

  // Root package entry under packages[""].
  updated = updated.replace(
    /("packages"\s*:\s*\{\s*""\s*:\s*\{\s*"name"\s*:\s*"[^"]+",\s*"version"\s*:\s*")([^"]+)(")/s,
    `$1${version}$3`
  );

  if (updated === packageLock) {
    return false;
  }

  writeFileSync(packageLockPath, updated, "utf8");
  return true;
}

function main() {
  const root = process.cwd();
  const packageJsonPath = resolve(root, "package.json");
  const tauriConfigPath = resolve(root, "src-tauri", "tauri.conf.json");
  const cargoTomlPath = resolve(root, "src-tauri", "Cargo.toml");
  const packageLockPath = resolve(root, "package-lock.json");

  const pkg = readJson(packageJsonPath);
  const version = pkg.version;

  if (typeof version !== "string" || version.trim() === "") {
    console.error("Error: package.json version is missing.");
    process.exit(1);
  }

  const changedFiles = [];

  if (updateTauriVersion(tauriConfigPath, version)) {
    changedFiles.push("src-tauri/tauri.conf.json");
  }

  if (updateCargoVersion(cargoTomlPath, version)) {
    changedFiles.push("src-tauri/Cargo.toml");
  }

  if (updatePackageLockVersion(packageLockPath, version)) {
    changedFiles.push("package-lock.json");
  }

  if (changedFiles.length === 0) {
    console.log(`Version already synced: ${version}`);
    return;
  }

  console.log(`Synced version ${version} to:`);
  for (const file of changedFiles) {
    console.log(`- ${file}`);
  }
}

main();