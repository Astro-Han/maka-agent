import { spawn } from 'node:child_process';
import { access, readFile, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const desktopRoot = join(repoRoot, 'apps', 'desktop');
const releaseDirectory = join(desktopRoot, 'release');
const electronDistributionDirectory = join(repoRoot, 'node_modules', 'electron', 'dist');
const requiredElectronLicensePaths = [
  join(electronDistributionDirectory, 'LICENSE'),
  join(electronDistributionDirectory, 'LICENSES.chromium.html'),
];

export function runCommand(
  command,
  args,
  { spawnProcess = spawn, platform = process.platform } = {},
) {
  return new Promise((resolve, reject) => {
    const child = spawnProcess(command, args, {
      cwd: repoRoot,
      env: process.env,
      stdio: 'inherit',
      // On Windows npm is npm.cmd, and a bare `npm` never resolves: libuv's
      // process launcher tries only .com and .exe and ignores PATHEXT, so the
      // spawn fails with ENOENT before any packaging happens. Going through the
      // shell is what makes .cmd reachable. Every command here is a repository
      // constant, so shell quoting is not a concern.
      shell: platform === 'win32',
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(
        new Error(
          `${command} ${args.join(' ')} failed with ${
            signal ? `signal ${signal}` : `exit code ${code}`
          }`,
        ),
      );
    });
  });
}

export async function packageWindowsX64({
  platform = process.platform,
  arch = process.arch,
  run = runCommand,
  remove = rm,
  assertFile = access,
} = {}) {
  if (platform !== 'win32' || arch !== 'x64') {
    throw new Error('Release packaging requires a Windows x64 host.');
  }

  const manifest = JSON.parse(await readFile(join(desktopRoot, 'package.json'), 'utf8'));
  const exePath = join(releaseDirectory, `Maka-${manifest.version}-win-x64.exe`);
  const zipPath = join(releaseDirectory, `Maka-${manifest.version}-win-x64.zip`);
  const updateMetadataPath = join(releaseDirectory, 'latest.yml');

  for (const path of requiredElectronLicensePaths) {
    await assertFile(path);
  }

  await run('npm', ['run', 'clean']);
  await run('npm', ['run', 'build']);
  await run('npm', ['run', 'check:release']);
  await remove(releaseDirectory, { recursive: true, force: true });
  await run('npm', ['--workspace', '@maka/desktop', 'run', 'package:windows-x64']);
  await assertFile(exePath);
  await assertFile(zipPath);
  await assertFile(updateMetadataPath);
  await remove(join(releaseDirectory, 'win-unpacked'), { recursive: true, force: true });

  return exePath;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const exePath = await packageWindowsX64();
  console.log(`Created ${exePath}`);
}
