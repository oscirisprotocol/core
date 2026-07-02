import { copyFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
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

const version = argument("--version").replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`invalid semantic version: ${version}`);
}

const tag = argument("--tag");
const repository = argument("--repository");
const input = path.resolve(argument("--input"));
const output = path.resolve(argument("--output"));
const tauriConfig = path.resolve(argument("--tauri-config"));
const files = await filesIn(input);
const configuredVersion = JSON.parse(await readFile(tauriConfig, "utf8")).version;
if (configuredVersion !== version) {
  throw new Error(
    `release version ${version} does not match Tauri version ${configuredVersion}`,
  );
}

const platforms = {
  "darwin-aarch64": "osciris-node-darwin-aarch64.app.tar.gz",
  "linux-x86_64": "osciris-node-linux-x86_64.AppImage",
  "windows-x86_64": "osciris-node-windows-x86_64-setup.exe",
};

await mkdir(output, { recursive: true });
const manifestPlatforms = {};
for (const [platform, filename] of Object.entries(platforms)) {
  const matches = files.filter((file) => path.basename(file) === filename);
  const signatures = files.filter(
    (file) => path.basename(file) === `${filename}.sig`,
  );
  if (matches.length !== 1 || signatures.length !== 1) {
    throw new Error(
      `expected exactly one bundle and signature for ${platform}; found ${matches.length} bundle(s), ${signatures.length} signature(s)`,
    );
  }

  await copyFile(matches[0], path.join(output, filename));
  const signature = (await readFile(signatures[0], "utf8")).trim();
  if (!signature) {
    throw new Error(`empty updater signature for ${platform}`);
  }

  manifestPlatforms[platform] = {
    signature,
    url: `https://github.com/${repository}/releases/download/${tag}/${filename}`,
  };
}

const manifest = {
  version,
  notes: `OSCIRIS Node ${tag}`,
  pub_date: new Date().toISOString(),
  platforms: manifestPlatforms,
};

await writeFile(
  path.join(output, "latest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
console.log(`generated signed updater manifest for ${tag}`);
