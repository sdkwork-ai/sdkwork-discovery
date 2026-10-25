#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { copyFile, mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const APP_ID = 'sdkwork-discovery';
const BINARY_NAME = 'sdkwork-discovery-service-host';
const SERVER_PROFILE = 'server';
const SUPPORTED_FORMAT = 'tar.gz';

const scriptPath = fileURLToPath(import.meta.url);
const appRoot = path.resolve(path.dirname(scriptPath), '..');

async function main() {
  const { command, options } = parseArgs(process.argv.slice(2));
  const context = await createPackageContext(options);

  if (command === 'package') {
    await packageServer(context);
    return;
  }
  if (command === 'validate') {
    await validateArchive(context);
    return;
  }
  if (command === 'help') {
    printHelp();
    return;
  }
  throw new Error(`Unsupported command: ${command}`);
}

function printHelp() {
  console.log(`Usage: node scripts/package-server.mjs <package|validate> [options]

Options:
  --version <value>       Package version. Defaults to SDKWORK_PACKAGE_VERSION or package.json.
  --platform <value>      Package platform. Defaults to SDKWORK_PACKAGE_PLATFORM or host platform.
  --arch <value>          Package architecture. Defaults to SDKWORK_PACKAGE_ARCHITECTURE or host arch.
  --format <value>        Package format. Only tar.gz is supported.
  --package-id <value>    Package id. Defaults to SDKWORK_PACKAGE_ID or canonical id.
  --rust-target <value>   Optional cargo --target triple for cross-builds.
  --skip-build            Reuse an existing release binary.
`);
}

function parseArgs(argv) {
  const hasExplicitCommand = argv[0] && !argv[0].startsWith('-');
  const command = hasExplicitCommand ? argv[0] : 'package';
  const options = {};
  const start = hasExplicitCommand ? 1 : 0;

  for (let index = start; index < argv.length; index += 1) {
    const arg = argv[index];
    switch (arg) {
      case '--version':
        options.version = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--platform':
        options.platform = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--arch':
      case '--architecture':
        options.architecture = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--format':
        options.format = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--package-id':
        options.packageId = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--rust-target':
        options.rustTarget = requireValue(argv, index, arg);
        index += 1;
        break;
      case '--skip-build':
        options.skipBuild = true;
        break;
      case '--help':
      case '-h':
        options.help = true;
        break;
      default:
        throw new Error(`Unsupported option: ${arg}`);
    }
  }

  return {
    command: options.help ? 'help' : command,
    options,
  };
}

function requireValue(argv, index, flag) {
  const value = argv[index + 1];
  if (value === undefined || value.startsWith('--')) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

async function createPackageContext(options) {
  const packageJson = JSON.parse(await readFile(path.join(appRoot, 'package.json'), 'utf8'));
  const hostPlatform = normalizeHostPlatform(process.platform);
  const hostArchitecture = normalizeHostArchitecture(process.arch);
  const platform = normalizePackagePlatform(
    options.platform ?? process.env.SDKWORK_PACKAGE_PLATFORM ?? hostPlatform
  );
  const architecture = normalizePackageArchitecture(
    options.architecture ?? process.env.SDKWORK_PACKAGE_ARCHITECTURE ?? hostArchitecture
  );
  const format = options.format ?? process.env.SDKWORK_PACKAGE_FORMAT ?? SUPPORTED_FORMAT;
  const version = normalizeVersion(
    options.version ?? process.env.SDKWORK_PACKAGE_VERSION ?? packageJson.version ?? '0.1.0'
  );
  const packageId =
    options.packageId ??
    process.env.SDKWORK_PACKAGE_ID ??
    `${platform}-${architecture}-${SERVER_PROFILE}-tar-gz`;
  const rustTarget = options.rustTarget ?? process.env.SDKWORK_RUST_TARGET ?? '';

  if (format !== SUPPORTED_FORMAT) {
    throw new Error(`Unsupported server package format ${format}; expected ${SUPPORTED_FORMAT}`);
  }
  if (!rustTarget && (platform !== hostPlatform || architecture !== hostArchitecture)) {
    throw new Error(
      `Refusing to label a native ${hostPlatform}/${hostArchitecture} build as ${platform}/${architecture}; pass --rust-target for cross-builds.`
    );
  }

  const distRoot = path.join(appRoot, 'dist', SERVER_PROFILE);
  const stageName = `${APP_ID}-${version}-${platform}-${architecture}-${SERVER_PROFILE}`;
  const stageRoot = path.join(distRoot, stageName);
  const archivePath = path.join(distRoot, `${stageName}.${SUPPORTED_FORMAT}`);
  const binaryName = platform === 'windows' ? `${BINARY_NAME}.exe` : BINARY_NAME;
  const releaseDir = rustTarget
    ? path.join(appRoot, 'target', rustTarget, 'release')
    : path.join(appRoot, 'target', 'release');
  const binaryPath = path.join(releaseDir, binaryName);

  return {
    architecture,
    archivePath,
    binaryName,
    binaryPath,
    distRoot,
    format,
    hostArchitecture,
    hostPlatform,
    packageId,
    platform,
    rustTarget,
    skipBuild: options.skipBuild === true,
    stageName,
    stageRoot,
    version,
  };
}

function normalizeVersion(value) {
  const text = String(value ?? '').trim();
  const normalized = text.startsWith('refs/tags/') ? text.slice('refs/tags/'.length) : text;
  const withoutPrefix = normalized.startsWith('v') && /^[0-9]/u.test(normalized.slice(1))
    ? normalized.slice(1)
    : normalized;
  if (!/^[0-9A-Za-z][0-9A-Za-z._+-]*$/u.test(withoutPrefix)) {
    throw new Error(`Invalid package version: ${value}`);
  }
  return withoutPrefix;
}

function normalizeHostPlatform(value) {
  if (value === 'win32') {
    return 'windows';
  }
  if (value === 'darwin') {
    return 'macos';
  }
  if (value === 'linux') {
    return 'linux';
  }
  throw new Error(`Unsupported host platform: ${value}`);
}

function normalizePackagePlatform(value) {
  const text = String(value ?? '').trim().toLowerCase();
  if (['linux', 'windows', 'macos'].includes(text)) {
    return text;
  }
  throw new Error(`Unsupported server package platform: ${value}`);
}

function normalizeHostArchitecture(value) {
  if (value === 'x64') {
    return 'x64';
  }
  if (value === 'arm64') {
    return 'arm64';
  }
  if (value === 'arm') {
    return 'armv7';
  }
  throw new Error(`Unsupported host architecture: ${value}`);
}

function normalizePackageArchitecture(value) {
  const text = String(value ?? '').trim().toLowerCase();
  if (['x64', 'arm64', 'armv7'].includes(text)) {
    return text;
  }
  throw new Error(`Unsupported server package architecture: ${value}`);
}

async function packageServer(context) {
  if (!context.skipBuild) {
    runCargoBuild(context);
  }
  if (!existsSync(context.binaryPath)) {
    throw new Error(`Missing release binary: ${context.binaryPath}`);
  }

  await assertPathInside(context.stageRoot, context.distRoot);
  await rm(context.stageRoot, { recursive: true, force: true });
  await rm(context.archivePath, { force: true });
  await mkdir(path.join(context.stageRoot, 'bin'), { recursive: true });
  await mkdir(path.join(context.stageRoot, 'config'), { recursive: true });
  await mkdir(path.join(context.stageRoot, 'specs'), { recursive: true });

  await copyFile(context.binaryPath, path.join(context.stageRoot, 'bin', context.binaryName));
  await copyConfigExamples(context.stageRoot);
  await copyRunbooks(context.stageRoot);
  await copyReleaseEvidence(context.stageRoot, context.version);
  await copyIfExists('README.md', path.join(context.stageRoot, 'README.md'));
  await copyIfExists('agent.app.config.json', path.join(context.stageRoot, 'agent.app.config.json'));
  await copyIfExists(path.join('specs', 'component.spec.json'), path.join(context.stageRoot, 'specs', 'component.spec.json'));
  await writeFile(path.join(context.stageRoot, 'INSTALL.md'), renderInstallGuide(context), 'utf8');
  await writeFile(
    path.join(context.stageRoot, 'install-manifest.json'),
    `${JSON.stringify(createInstallManifest(context), null, 2)}\n`,
    'utf8'
  );
  await writeChecksums(context.stageRoot);
  await createArchive(context);
  await validateArchive(context);

  console.log(`[${APP_ID}] packaged ${path.relative(appRoot, context.archivePath)}`);
}

function runCargoBuild(context) {
  const args = [
    'build',
    '--release',
    '-p',
    'sdkwork-discovery-service-host',
    '--bin',
    BINARY_NAME,
    '--features',
    'prometheus',
  ];
  if (context.rustTarget) {
    args.push('--target', context.rustTarget);
  }
  run('cargo', args, { cwd: appRoot });
}

async function copyRunbooks(stageRoot) {
  const runbookDir = path.join(stageRoot, 'docs', 'runbooks');
  await mkdir(runbookDir, { recursive: true });
  for (const fileName of [
    'README.md',
    'RUNBOOK-production-server-deployment.md',
    'RUNBOOK-database-migration-rollback.md',
  ]) {
    const source = path.join(appRoot, 'docs', 'runbooks', fileName);
    if (existsSync(source)) {
      await copyFile(source, path.join(runbookDir, fileName));
    }
  }
}

async function copyReleaseEvidence(stageRoot, version) {
  const changelogDir = path.join(stageRoot, 'docs', 'changelogs');
  const releaseDir = path.join(stageRoot, 'docs', 'releases');
  await mkdir(changelogDir, { recursive: true });
  await mkdir(releaseDir, { recursive: true });

  const changelogSource = path.join(appRoot, 'docs', 'changelogs', 'CHANGELOG.md');
  if (existsSync(changelogSource)) {
    await copyFile(changelogSource, path.join(changelogDir, 'CHANGELOG.md'));
  }

  const releaseSource = path.join(appRoot, 'docs', 'releases', `RELEASE-v${version}.md`);
  if (existsSync(releaseSource)) {
    await copyFile(releaseSource, path.join(releaseDir, `RELEASE-v${version}.md`));
  }
}

async function copyConfigExamples(stageRoot) {
  const examples = [
    'discovery.example.toml',
    'discovery.production.example.toml',
  ];
  for (const fileName of examples) {
    const exampleConfig = path.join(appRoot, 'etc', fileName);
    if (existsSync(exampleConfig)) {
      await copyFile(exampleConfig, path.join(stageRoot, 'config', fileName));
    }
  }
}

async function copyIfExists(relativeSource, destination) {
  const source = path.join(appRoot, relativeSource);
  if (existsSync(source)) {
    await copyFile(source, destination);
  }
}

function renderInstallGuide(context) {
  const binaryPath =
    context.platform === 'windows'
      ? `.\\bin\\${BINARY_NAME}.exe`
      : `./bin/${BINARY_NAME}`;
  return `# SDKWork Discovery Server Package

Package: ${context.packageId}
Version: ${context.version}
Target: ${context.platform}/${context.architecture}

## Configure

1. Apply database migrations before first production start (\`pnpm db:migrate\` from a workspace checkout, or your deployment pipeline equivalent).
2. Copy \`config/discovery.production.example.toml\` to a host-local protected path such as \`/etc/sdkwork/discovery/production.toml\`.
3. Mount secret files referenced by the config (TLS certs, service-token HMAC secret, database password file).
4. Export runtime env:

\`\`\`sh
export SDKWORK_DISCOVERY_CONFIG_FILE=/etc/sdkwork/discovery/production.toml
export SDKWORK_DISCOVERY_METRICS_BIND=0.0.0.0:9090
\`\`\`

Development-only smoke tests may use \`config/discovery.example.toml\` on loopback with unsigned local context enabled.

## Start

\`\`\`sh
${binaryPath}
\`\`\`

The process handles SIGTERM and Ctrl+C for graceful shutdown: health status is cleared, then the gRPC server stops.

## Health and metrics

- gRPC health: enabled when \`[server].enable_health = true\` on the configured bind.
- Prometheus: this package is built with the \`prometheus\` feature. Scrape \`SDKWORK_DISCOVERY_METRICS_BIND\` (default \`127.0.0.1:9090\`).
- Process gauge: \`discovery_health_status\` (1=serving, 0=shutting down).

## Runbooks

- \`docs/runbooks/RUNBOOK-production-server-deployment.md\`
- \`docs/runbooks/RUNBOOK-database-migration-rollback.md\`

## Release notes

- \`docs/changelogs/CHANGELOG.md\`
- \`docs/releases/RELEASE-v${context.version}.md\`
`;
}

function createInstallManifest(context) {
  return {
    schemaVersion: 1,
    appId: APP_ID,
    packageId: context.packageId,
    profile: SERVER_PROFILE,
    platform: context.platform,
    architecture: context.architecture,
    format: context.format,
    version: context.version,
    binary: `bin/${context.binaryName}`,
    configExamples: [
      'config/discovery.example.toml',
      'config/discovery.production.example.toml',
    ],
    runbooks: [
      'docs/runbooks/RUNBOOK-production-server-deployment.md',
      'docs/runbooks/RUNBOOK-database-migration-rollback.md',
    ],
    releaseEvidence: [
      'docs/changelogs/CHANGELOG.md',
      `docs/releases/RELEASE-v${context.version}.md`,
    ],
    observability: {
      prometheusFeature: true,
      metricsBindEnv: 'SDKWORK_DISCOVERY_METRICS_BIND',
      defaultMetricsBind: '127.0.0.1:9090',
      healthGauge: 'discovery_health_status',
    },
    grpcSurfaces: ['application.public-ingress', 'operations.control-ingress'],
  };
}

async function writeChecksums(stageRoot) {
  const entries = [];
  for (const filePath of await listFiles(stageRoot)) {
    const relativePath = toPosixPath(path.relative(stageRoot, filePath));
    if (relativePath === 'checksums.sha256') {
      continue;
    }
    const digest = createHash('sha256').update(await readFile(filePath)).digest('hex');
    entries.push(`${digest}  ${relativePath}`);
  }
  await writeFile(path.join(stageRoot, 'checksums.sha256'), `${entries.sort().join('\n')}\n`, 'utf8');
}

async function listFiles(root) {
  const result = [];
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      result.push(...(await listFiles(entryPath)));
    } else if (entry.isFile()) {
      result.push(entryPath);
    }
  }
  return result;
}

async function createArchive(context) {
  await mkdir(context.distRoot, { recursive: true });
  run('tar', ['-czf', context.archivePath, '-C', context.distRoot, context.stageName], { cwd: appRoot });
}

async function validateArchive(context) {
  if (!existsSync(context.archivePath)) {
    throw new Error(`Missing server archive: ${context.archivePath}`);
  }
  const archiveStats = await stat(context.archivePath);
  if (archiveStats.size <= 0) {
    throw new Error(`Server archive is empty: ${context.archivePath}`);
  }

  const listing = run('tar', ['-tzf', context.archivePath], { cwd: appRoot, capture: true });
  const requiredEntries = [
    `${context.stageName}/bin/${context.binaryName}`,
    `${context.stageName}/config/discovery.example.toml`,
    `${context.stageName}/config/discovery.production.example.toml`,
    `${context.stageName}/docs/runbooks/RUNBOOK-production-server-deployment.md`,
    `${context.stageName}/docs/runbooks/RUNBOOK-database-migration-rollback.md`,
    `${context.stageName}/docs/changelogs/CHANGELOG.md`,
    `${context.stageName}/docs/releases/RELEASE-v${context.version}.md`,
    `${context.stageName}/INSTALL.md`,
    `${context.stageName}/install-manifest.json`,
    `${context.stageName}/checksums.sha256`,
    `${context.stageName}/README.md`,
  ];

  for (const entry of requiredEntries) {
    if (!listing.includes(entry)) {
      throw new Error(`Server archive missing ${entry}`);
    }
  }
  console.log(`[${APP_ID}] validated ${path.relative(appRoot, context.archivePath)}`);
}

function run(command, args, { cwd, capture = false } = {}) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    shell: false,
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
    throw new Error(
      `${command} ${args.join(' ')} failed with exit code ${result.status}${output ? `\n${output}` : ''}`
    );
  }
  return capture ? String(result.stdout ?? '') : '';
}

async function assertPathInside(targetPath, parentPath) {
  const relativePath = path.relative(parentPath, targetPath);
  if (relativePath.startsWith('..') || path.isAbsolute(relativePath)) {
    throw new Error(`Refusing to write outside ${parentPath}: ${targetPath}`);
  }
}

function toPosixPath(value) {
  return value.split(path.sep).join('/');
}

main().catch((error) => {
  console.error(`[${APP_ID}] ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
});
