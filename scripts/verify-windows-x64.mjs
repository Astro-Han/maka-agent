import { access, mkdtemp, open, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import {
  assertMissing,
  assertPackagedResources,
  isolatedUserEnv,
  makePtyProbe,
  runCommand,
  sha256File,
  smokePackagedFilesystemWorker,
  smokePackagedRenderer,
} from './verify-packaged-app.mjs';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const desktopRoot = join(repoRoot, 'apps', 'desktop');
const executableName = 'Maka.exe';
const amd64Machine = 0x8664;
// conpty echoes the command and terminates lines with CRLF, so the probe keeps
// matching on a substring rather than the whole output.
const ptyProbe = makePtyProbe(process.env.ComSpec || 'cmd.exe', ['/c', 'echo', 'maka-node-pty-ok']);

function runCommandFromRepo(command, args, options = {}) {
  return runCommand(command, args, { cwd: repoRoot, ...options });
}

function runPowerShell(run, script) {
  return run('powershell', ['-NoProfile', '-NonInteractive', '-Command', script]);
}

// electron-builder writes the Windows product version resource in the four-part
// form Windows wants (app-builder-lib `AppInfo.getVersionInWeirdWindowsForm`),
// so 0.1.5 ships as 0.1.5.0 and the release version is its first three parts.
// The fourth part is a build number, which is 0 unless one is configured.
export function assertWindowsProductVersion(productVersion, expectedVersion) {
  const [expected] = expectedVersion.split('-');
  const parts = productVersion.trim().split('.');
  if (parts.length !== 4 || parts.slice(0, 3).join('.') !== expected || !/^\d+$/.test(parts[3])) {
    throw new Error(
      `Expected app version ${expected}.<build>, found ${productVersion.trim() || '<none>'}.`,
    );
  }
}

// The Windows build is unsigned, so the only architecture evidence in the
// artifact is the PE header of the executable itself.
export async function readPeMachine(path) {
  const file = await open(path, 'r');
  try {
    const header = Buffer.alloc(4);
    const { bytesRead } = await file.read(header, 0, 4, 0x3c);
    if (bytesRead !== 4) {
      throw new Error(`${path} is too small to be a PE image.`);
    }
    const peOffset = header.readUInt32LE(0);
    const signature = Buffer.alloc(6);
    const peRead = await file.read(signature, 0, 6, peOffset);
    if (peRead.bytesRead !== 6 || signature.toString('latin1', 0, 4) !== 'PE\0\0') {
      throw new Error(`${path} is not a PE image.`);
    }
    return signature.readUInt16LE(4);
  } finally {
    await file.close();
  }
}

export async function verifyPackagedWindowsApp(
  appDirectory,
  {
    run = runCommandFromRepo,
    requirePath = access,
    forbidPath = assertMissing,
    readMachine = readPeMachine,
    smokeRenderer = smokePackagedRenderer,
    smokeFilesystemWorker = smokePackagedFilesystemWorker,
    workingDirectory = appDirectory,
  } = {},
) {
  const desktopManifest = JSON.parse(await readFile(join(desktopRoot, 'package.json'), 'utf8'));
  const resources = join(appDirectory, 'resources');
  const executable = join(appDirectory, executableName);
  const filesystemWorker = join(resources, 'workers', 'filesystem-worker.js');
  const appAsar = join(resources, 'app.asar');

  await requirePath(executable);
  await assertPackagedResources(resources, { requirePath, forbidPath });

  const machine = await readMachine(executable);
  if (machine !== amd64Machine) {
    throw new Error(`${executableName} must be x64, found PE machine 0x${machine.toString(16)}.`);
  }

  const { stdout } = await runPowerShell(
    run,
    `(Get-Item -LiteralPath ${JSON.stringify(executable)}).VersionInfo.ProductVersion`,
  );
  assertWindowsProductVersion(stdout, desktopManifest.version);

  await run(executable, ['-e', ptyProbe, join(appAsar, 'package.json')], {
    env: {
      ELECTRON_RUN_AS_NODE: '1',
      ...isolatedUserEnv(join(workingDirectory, 'pty-home')),
    },
  });
  await smokeFilesystemWorker(executable, filesystemWorker, { workingDirectory, run });
  await smokeRenderer(executable, { workingDirectory });
}

// The NSIS installer carries no inspectable app structure, so the ZIP of the
// same build is what gets verified; installing the .exe is a checklist step.
export async function verifyWindowsX64Exe(
  inputPath,
  {
    platform = process.platform,
    run = runCommandFromRepo,
    verifyApp = verifyPackagedWindowsApp,
    checksum = sha256File,
  } = {},
) {
  if (platform !== 'win32') {
    throw new Error('Windows release verification requires Windows.');
  }
  if (!inputPath) {
    throw new Error('Usage: npm run verify:windows-x64 -- <path-to-exe>');
  }

  const exePath = resolve(inputPath);
  if (!exePath.endsWith('.exe')) {
    throw new Error(`Expected the NSIS installer .exe, found ${basename(exePath)}.`);
  }
  const zipPath = `${exePath.slice(0, -'.exe'.length)}.zip`;
  await access(exePath);
  await access(zipPath);

  const temporaryDirectory = await mkdtemp(join(tmpdir(), 'maka-release-verify-'));
  const extracted = join(temporaryDirectory, 'app');

  try {
    await runPowerShell(
      run,
      `Expand-Archive -LiteralPath ${JSON.stringify(zipPath)} -DestinationPath ${JSON.stringify(
        extracted,
      )} -Force`,
    );
    await verifyApp(extracted, { workingDirectory: temporaryDirectory });

    const checksums = [];
    for (const path of [exePath, zipPath]) {
      const sha256 = await checksum(path);
      const checksumPath = `${path}.sha256`;
      await writeFile(checksumPath, `${sha256}  ${basename(path)}\n`, 'utf8');
      checksums.push({ path, checksumPath, sha256 });
    }
    return { exePath, zipPath, checksums };
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = await verifyWindowsX64Exe(process.argv[2]);
  console.log(`Verified ${result.exePath}`);
  for (const { path, sha256 } of result.checksums) {
    console.log(`SHA-256 ${sha256}  ${basename(path)}`);
  }
}
