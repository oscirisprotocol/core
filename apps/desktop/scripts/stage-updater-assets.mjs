import { copyFile, mkdir, readdir } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

function argument(name) {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) {
    throw new Error(`missing ${name}`);
  }
  return process.argv[index + 1];
}

async function filesIn(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await filesIn(entryPath)));
    } else {
      files.push(entryPath);
    }
  }
  return files;
}

const platform = argument("--platform");
const bundleRoot = path.resolve(argument("--bundle-root"));
const output = path.resolve(argument("--output"));
const files = await filesIn(bundleRoot);

const specifications = {
  "darwin-aarch64": {
    matches: (file) => file.endsWith(".app.tar.gz"),
    output: "osciris-node-darwin-aarch64.app.tar.gz",
  },
  "linux-x86_64": {
    matches: (file) => file.endsWith(".AppImage"),
    output: "osciris-node-linux-x86_64.AppImage",
  },
  "windows-x86_64": {
    matches: (file) => file.endsWith("-setup.exe"),
    output: "osciris-node-windows-x86_64-setup.exe",
  },
};

const specification = specifications[platform];
if (!specification) {
  throw new Error(`unsupported updater platform: ${platform}`);
}

const candidates = files.filter(
  (file) => specification.matches(file) && !file.endsWith(".sig"),
);
if (candidates.length !== 1) {
  throw new Error(
    `expected one ${platform} updater bundle, found ${candidates.length}: ${candidates.join(", ")}`,
  );
}

const source = candidates[0];
const signature = `${source}.sig`;
if (!files.includes(signature)) {
  throw new Error(`missing updater signature: ${signature}`);
}

await mkdir(output, { recursive: true });
await copyFile(source, path.join(output, specification.output));
await copyFile(signature, path.join(output, `${specification.output}.sig`));

console.log(`staged ${platform} updater bundle`);
