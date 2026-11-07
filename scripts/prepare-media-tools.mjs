import { chmod, cp, mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourceDirectory = path.join(homedir(), "bin");
const targetDirectory = path.join(projectRoot, "BIN");
const windows = process.platform === "win32";
const executableNames = windows ? ["ffmpeg.exe", "ffprobe.exe"] : ["ffmpeg", "ffprobe"];

await rm(targetDirectory, { recursive: true, force: true });
await mkdir(targetDirectory, { recursive: true });

for (const name of executableNames) {
  const source = path.join(sourceDirectory, name);
  try {
    await cp(source, path.join(targetDirectory, name));
  } catch {
    throw new Error(`Missing ${source}. Put FFmpeg and FFprobe in ~/bin before packaging Blaze.`);
  }
  if (!windows) await chmod(path.join(targetDirectory, name), 0o755);
}

if (process.platform === "linux" || process.platform === "darwin") {
  const sourceLibraries = path.join(sourceDirectory, "lib");
  const targetLibraries = path.join(targetDirectory, "lib");
  try {
    const entries = await readdir(sourceLibraries, { withFileTypes: true });
    await mkdir(targetLibraries, { recursive: true });
    for (const entry of entries) {
      if (entry.name.includes(".so") || entry.name.endsWith(".dylib")) {
        await cp(
          path.join(sourceLibraries, entry.name),
          path.join(targetLibraries, entry.name),
        );
      }
    }
  } catch {
    // Static builds do not need a separate library directory.
  }
}

if (windows) {
  const entries = await readdir(sourceDirectory, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.isFile() && entry.name.toLowerCase().endsWith(".dll")) {
      await cp(path.join(sourceDirectory, entry.name), path.join(targetDirectory, entry.name));
    }
  }
}

const toolEnvironment = { ...process.env };
if (process.platform === "linux") {
  toolEnvironment.LD_LIBRARY_PATH = [
    path.join(targetDirectory, "lib"),
    targetDirectory,
    process.env.LD_LIBRARY_PATH,
  ].filter(Boolean).join(path.delimiter);
}
if (process.platform === "darwin") {
  toolEnvironment.DYLD_LIBRARY_PATH = [
    path.join(targetDirectory, "lib"),
    targetDirectory,
    process.env.DYLD_LIBRARY_PATH,
  ].filter(Boolean).join(path.delimiter);
}
if (windows) {
  toolEnvironment.PATH = [targetDirectory, process.env.PATH].filter(Boolean).join(path.delimiter);
}

let version;
try {
  const ffmpeg = path.join(targetDirectory, executableNames[0]);
  version = execFileSync(ffmpeg, ["-version"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    env: toolEnvironment,
  }).split("\n")[0];

  const license = execFileSync(ffmpeg, ["-L"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    env: toolEnvironment,
  });
  if (!/Lesser General Public License/i.test(license)) {
    throw new Error("The selected FFmpeg build is not an LGPL build.");
  }

  execFileSync(path.join(targetDirectory, executableNames[1]), ["-version"], {
    stdio: "ignore",
    env: toolEnvironment,
  });
} catch (error) {
  throw new Error(`FFmpeg/FFprobe validation failed: ${error.message}`);
}

await writeFile(
  path.join(targetDirectory, "note.md"),
  `# Blaze media tools

${version}

Build family: BtbN FFmpeg Builds, LGPL shared distribution.
Linux x86_64 release archive: ffmpeg-n9.0.2-48-gd4070a14ce-linux64-lgpl-shared-9.0.tar.xz
Windows x86_64 release archive: ffmpeg-n9.0.2-48-gd4070a14ce-win64-lgpl-shared-9.0.zip

The matching shared libraries are included alongside the executables.
Build details: https://github.com/BtbN/FFmpeg-Builds/releases
FFmpeg licensing: https://ffmpeg.org/legal.html

Distributors must include the applicable LGPL notices and corresponding
source materials for the exact FFmpeg build they ship.
`,
);

console.log(`Prepared portable media tools in ${targetDirectory}`);
