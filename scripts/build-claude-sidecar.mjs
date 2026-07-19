import { cp, mkdir, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";

import { stageClaudeSdkPlatformAssets } from "./claude-sidecar-staging.mjs";
import { ensureClaudeSdkPnpmLayout } from "./ensure-claude-sdk-pnpm-layout.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const entryPoint = path.join(
  repoRoot,
  "src-tauri",
  "sidecar",
  "claude-agent-sdk-server.mjs",
);
const outFile = path.join(
  repoRoot,
  "src-tauri",
  "sidecar-dist",
  "claude-agent-sdk-server.mjs",
);
const outDir = path.dirname(outFile);
const sdkDistNodeModulesDir = path.join(outDir, "node_modules");
const sdkDistPackageDir = path.join(
  sdkDistNodeModulesDir,
  "@anthropic-ai",
  "claude-agent-sdk",
);
const linuxSdkArchiveFile = path.join(outDir, "claude-sdk-node_modules.tar.gz");
const execFileAsync = promisify(execFile);

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: repoRoot,
      stdio: "inherit",
      ...options,
    });

    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(
        new Error(
          signal
            ? `${command} ${args.join(" ")} exited with signal ${signal}`
            : `${command} ${args.join(" ")} exited with code ${code}`,
        ),
      );
    });
  });
}

async function removeGeneratedSidecarPath(targetPath, options) {
  const relativePath = path.relative(repoRoot, targetPath);
  const { stdout } = await execFileAsync("git", ["ls-files", "--", relativePath], {
    cwd: repoRoot,
    windowsHide: true,
  });

  if (stdout.trim()) {
    throw new Error(
      `Refusing to remove tracked sidecar content: ${relativePath}. Only generated sidecar content may be cleaned.`,
    );
  }

  await rm(targetPath, options);
}

async function archiveLinuxSdkNodeModules() {
  const targetPlatform = process.env.PANES_CLAUDE_SDK_PLATFORM ?? process.platform;
  if (targetPlatform !== "linux") {
    return;
  }

  await removeGeneratedSidecarPath(linuxSdkArchiveFile, { force: true });
  await run("tar", ["-czf", path.basename(linuxSdkArchiveFile), "node_modules"], {
    cwd: outDir,
  });
  await removeGeneratedSidecarPath(sdkDistNodeModulesDir, {
    recursive: true,
    force: true,
  });
  console.log("Archived Claude SDK node_modules for Linux runtime staging.");
}

const { sdkPackageDir } = await ensureClaudeSdkPnpmLayout(repoRoot);

await removeGeneratedSidecarPath(sdkDistNodeModulesDir, {
  recursive: true,
  force: true,
});
await removeGeneratedSidecarPath(linuxSdkArchiveFile, { force: true });
await mkdir(outDir, { recursive: true });

await cp(entryPoint, outFile, {
  force: true,
});

await cp(sdkPackageDir, sdkDistNodeModulesDir, {
  recursive: true,
  dereference: true,
  force: true,
});

await stageClaudeSdkPlatformAssets({
  sdkDistNodeModulesDir,
  sdkDistPackageDir,
  targetPlatform: process.env.PANES_CLAUDE_SDK_PLATFORM ?? process.platform,
  targetArch: process.env.PANES_CLAUDE_SDK_ARCH ?? process.arch,
  targetLibc: process.env.PANES_CLAUDE_SDK_LIBC ?? "glibc",
});
await archiveLinuxSdkNodeModules();

const output = await readFile(outFile, "utf8");
if (!output.includes('import("@anthropic-ai/claude-agent-sdk")')) {
  throw new Error(
    "Claude sidecar staging no longer imports @anthropic-ai/claude-agent-sdk from node_modules as expected.",
  );
}
