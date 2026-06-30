import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const desktopDirectory = resolve(scriptDirectory, "..");
const repositoryRoot = resolve(desktopDirectory, "../..");
const release = process.argv.includes("--release");
const profile = release ? "release" : "debug";
const extension = process.platform === "win32" ? ".exe" : "";
const targetTriple = execFileSync("rustc", ["--print", "host-tuple"], {
  encoding: "utf8",
}).trim();

if (!targetTriple) {
  throw new Error("rustc did not report a host target triple");
}

const cargoArguments = [
  "build",
  "--locked",
  "-p",
  "osciris-daemon",
  "--bin",
  "osciris-daemon",
];
if (release) {
  cargoArguments.push("--release");
}

execFileSync("cargo", cargoArguments, {
  cwd: repositoryRoot,
  stdio: "inherit",
});

const source = join(
  repositoryRoot,
  "target",
  profile,
  `osciris-daemon${extension}`,
);
const destinationDirectory = join(desktopDirectory, "src-tauri", "binaries");
const destination = join(
  destinationDirectory,
  `osciris-daemon-${targetTriple}${extension}`,
);

mkdirSync(destinationDirectory, { recursive: true });
copyFileSync(source, destination);
console.log(`Prepared ${destination}`);
