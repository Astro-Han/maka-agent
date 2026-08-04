import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const { packageWindowsX64 } = await import(new URL('package-windows-x64.mjs', import.meta.url));
const { readPeMachine, verifyPackagedWindowsApp, verifyWindowsX64Exe } = await import(
  new URL('verify-windows-x64.mjs', import.meta.url)
);

const desktopManifest = JSON.parse(
  await readFile(new URL('../apps/desktop/package.json', import.meta.url), 'utf8'),
);

function peImage(machine) {
  const image = Buffer.alloc(0x100);
  image.writeUInt8(0x4d, 0);
  image.writeUInt8(0x5a, 1);
  image.writeUInt32LE(0x80, 0x3c);
  image.write('PE\0\0', 0x80, 'latin1');
  image.writeUInt16LE(machine, 0x84);
  return image;
}

function packagedAppOptions(overrides = {}) {
  return {
    run: async (command) => {
      if (command === 'powershell') return { stdout: `${desktopManifest.version}\n`, stderr: '' };
      return { stdout: '', stderr: '' };
    },
    requirePath: async () => {},
    forbidPath: async () => {},
    readMachine: async () => 0x8664,
    smokeRenderer: async () => {},
    smokeFilesystemWorker: async () => {},
    ...overrides,
  };
}

test('Windows packaging fails closed on hosts that cannot produce the release', async () => {
  await assert.rejects(packageWindowsX64({ platform: 'darwin', arch: 'x64' }), /Windows x64 host/);
  await assert.rejects(packageWindowsX64({ platform: 'win32', arch: 'arm64' }), /Windows x64 host/);
});

test('Windows verification fails closed on the host, the input, and the architecture', async () => {
  await assert.rejects(
    verifyWindowsX64Exe('C:/Maka.exe', { platform: 'darwin' }),
    /requires Windows/,
  );
  await assert.rejects(verifyWindowsX64Exe('', { platform: 'win32' }), /Usage:/);
  await assert.rejects(verifyWindowsX64Exe('C:/Maka.zip', { platform: 'win32' }), /NSIS installer/);

  await assert.rejects(
    verifyPackagedWindowsApp('C:/app', packagedAppOptions({ readMachine: async () => 0x014c })),
    /must be x64/,
  );
});

test('a packaged Windows app whose version does not match the manifest is rejected', async () => {
  await assert.rejects(
    verifyPackagedWindowsApp(
      'C:/app',
      packagedAppOptions({
        run: async (command) => {
          if (command === 'powershell') return { stdout: '0.0.0-not-the-release\n', stderr: '' };
          return { stdout: '', stderr: '' };
        },
      }),
    ),
    /Expected app version/,
  );
});

test('the packaged Windows app is checked for every unsigned helper that could still be in a tree', async () => {
  // Same reason as the macOS check: `apps/desktop/resources/bin` is gitignored,
  // so a helper a developer prepared once is still on their machine and would
  // be packaged without anything noticing.
  const forbidden = [];
  await assert.rejects(
    verifyPackagedWindowsApp(
      'C:/app',
      packagedAppOptions({
        forbidPath: async (path) => {
          forbidden.push(path);
        },
        // Everything before the architecture check has to pass, so the failure
        // that ends this run is the one after the forbids.
        readMachine: async () => 0x014c,
      }),
    ),
    /must be x64/,
  );
  for (const helper of ['cua-driver', 'maka-cu', 'officecli']) {
    assert.ok(
      forbidden.some((path) => path.endsWith(helper)),
      `${helper} is not among the paths the packaged app is checked against`,
    );
  }
});

test('the PE machine is read from the executable header', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-pe-'));
  try {
    const amd64 = join(directory, 'amd64.exe');
    const i386 = join(directory, 'i386.exe');
    const notPe = join(directory, 'not-pe.exe');
    await writeFile(amd64, peImage(0x8664));
    await writeFile(i386, peImage(0x014c));
    await writeFile(notPe, Buffer.alloc(0x100));

    assert.equal(await readPeMachine(amd64), 0x8664);
    assert.equal(await readPeMachine(i386), 0x014c);
    await assert.rejects(readPeMachine(notPe), /not a PE image/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
