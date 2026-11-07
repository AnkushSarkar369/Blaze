import {
  chmod,
  cp,
  mkdir,
  mkdtemp,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const targetDirectory = path.join(projectRoot, "BIN");
const archiveName = "ffmpeg-n9.0.2-48-gd4070a14ce-linux64-lgpl-shared-9.0.tar.xz";
const archiveUrl =
  "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-11-07-12-47/" +
  archiveName;

if (process.platform !== "linux" || process.arch !== "x64") {
  throw new Error(
    "The current bundled media-tool setup supports Linux x86_64 only.",
  );
}

await mkdir(targetDirectory, { recursive: true });

const environment = () => ({
  ...process.env,
  LD_LIBRARY_PATH: [
    path.join(targetDirectory, "lib"),
    targetDirectory,
    process.env.LD_LIBRARY_PATH,
  ].filter(Boolean).join(path.delimiter),
});

function validateTools() {
  const ffmpeg = path.join(targetDirectory, "ffmpeg");
  const ffprobe = path.join(targetDirectory, "ffprobe");

  execFileSync(ffmpeg, ["-version"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    env: environment(),
  });

  const license = execFileSync(ffmpeg, ["-L"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    env: environment(),
  });

  if (!/Lesser General Public License/i.test(license)) {
    throw new Error("The bundled FFmpeg build is not LGPL-licensed.");
  }

  execFileSync(ffprobe, ["-version"], {
    stdio: "ignore",
    env: environment(),
  });

  return execFileSync(ffmpeg, ["-version"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    env: environment(),
  }).split("\n")[0];
}

let version;
try {
  version = validateTools();
  console.log(`Using existing media tools: ${version}`);
} catch {
  console.log("Preparing Blaze's pinned FFmpeg/FFprobe build...");
  const temporaryDirectory = await mkdtemp(path.join(os.tmpdir(), "blaze-ffmpeg-"));

  try {
    const archivePath = path.join(temporaryDirectory, archiveName);
    const extractedDirectory = path.join(temporaryDirectory, "extracted");
    await mkdir(extractedDirectory, { recursive: true });

    execFileSync("curl", [
      "--fail",
      "--location",
      "--retry", "3",
      "--output", archivePath,
      archiveUrl,
    ], { stdio: "inherit" });

    execFileSync("tar", ["-xJf", archivePath, "-C", extractedDirectory], {
      stdio: "inherit",
    });

    async function findFile(directory, filename) {
      for (const entry of await readdir(directory, { withFileTypes: true })) {
        const candidate = path.join(directory, entry.name);
        if (entry.isFile() && entry.name === filename) return candidate;
        if (entry.isDirectory()) {
          const nested = await findFile(candidate, filename);
          if (nested) return nested;
        }
      }
      return null;
    }

    const ffmpegSource = await findFile(extractedDirectory, "ffmpeg");
    const ffprobeSource = await findFile(extractedDirectory, "ffprobe");
    if (!ffmpegSource || !ffprobeSource) {
      throw new Error("The downloaded archive does not contain FFmpeg and FFprobe.");
    }

    const sourceBin = path.dirname(ffmpegSource);
    const sourceRoot = path.dirname(sourceBin);
    const sourceLib = path.join(sourceRoot, "lib");

    await rm(targetDirectory, { recursive: true, force: true });
    await mkdir(path.join(targetDirectory, "lib"), { recursive: true });
    await cp(ffmpegSource, path.join(targetDirectory, "ffmpeg"));
    await cp(ffprobeSource, path.join(targetDirectory, "ffprobe"));
    await chmod(path.join(targetDirectory, "ffmpeg"), 0o755);
    await chmod(path.join(targetDirectory, "ffprobe"), 0o755);

    try {
      await cp(sourceLib, path.join(targetDirectory, "lib"), {
        recursive: true,
        dereference: true,
      });
    } catch (error) {
      throw new Error(`Failed to copy FFmpeg shared libraries from ${sourceLib}: ${error.message}`);
    }

    version = validateTools();
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

await writeFile(
  path.join(targetDirectory, "note.md"),
  `# Blaze media tools

${version}

Source: BtbN FFmpeg Builds
Pinned archive: ${archiveName}
License: LGPL shared build
Release: https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-11-07-12-47
FFmpeg licensing: https://ffmpeg.org/legal.html

The exact LGPL notices and corresponding source materials must accompany
redistribution of this build. See the upstream release for build details.
`,
);

console.log(`Media tools are ready in ${targetDirectory}`);
