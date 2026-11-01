#!/usr/bin/env node

import { spawn } from 'node:child_process';
import process from 'node:process';

import { ensurePostgresDevEnvFile, loadEnvFile } from '../../../sdkwork-app-topology/tools/topology/lib/env-file.mjs';

import {
  DEFAULT_DEV_PROFILE_ID,
  filterDiscoveryProcessEnv,
  isTcpPortReachable,
  listHealthSurfaces,
  listOrchestrationProcesses,
  loadProfile,
  mergeRuntimeEnv,
  REPO_ROOT,
  resolveDevProfileId,
  resolveSurfaceBind,
  resolveSurfaceGrpcUrl,
  splitHostPort,
} from './lib/discovery-topology.mjs';

const STARTUP_WAIT_MS = 500;
const MAX_STARTUP_ATTEMPTS = 60;

function cargoCommand() {
  return process.platform === 'win32' ? 'cargo.exe' : 'cargo';
}

function parseArgs(argv) {
  const settings = {
    deploymentProfile: 'standalone',
    dryRun: false,
    help: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--help' || arg === '-h') {
      settings.help = true;
      continue;
    }
    if (arg === '--deployment-profile') {
      settings.deploymentProfile = argv[index + 1] ?? settings.deploymentProfile;
      index += 1;
      continue;
    }
    if (arg === '--hosting') {
      throw new Error(
        '--hosting is retired; use --deployment-profile (standalone or cloud)',
      );
    }
    if (arg === '--service-layout') {
      throw new Error(
        '--service-layout is retired; use --deployment-profile standalone|cloud',
      );
    }
    if (arg === '--topology') {
      throw new Error(
        '--topology is retired; use --deployment-profile standalone|cloud',
      );
    }
    if (arg === '--dry-run') {
      settings.dryRun = true;
    }
  }

  return settings;
}

function printHelp() {
  console.log(`Usage: node scripts/discovery-dev.mjs [options]

Topology-aware Discovery dev entry. Loads etc/topology profile env via @sdkwork/app-topology.

Options:
  --deployment-profile <standalone|cloud>           Default: standalone
  --dry-run                                         Print plan without executing
  --help, -h
`);
}

async function waitForSurfaceHealth(profileId, env) {
  const surfaces = listHealthSurfaces(profileId);
  for (const surfaceId of surfaces) {
    const bind = resolveSurfaceBind(env, surfaceId);
    if (!bind) {
      continue;
    }
    const { host, port } = splitHostPort(bind);
    if (!host || !port) {
      continue;
    }
    let ready = false;
    for (let attempt = 0; attempt < MAX_STARTUP_ATTEMPTS; attempt += 1) {
      ready = await isTcpPortReachable(Number(port), host);
      if (ready) {
        console.log(`[sdkwork-discovery] healthy ${surfaceId} (${bind})`);
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, STARTUP_WAIT_MS));
    }
    if (!ready) {
      throw new Error(`timed out waiting for ${surfaceId} tcp health at ${bind}`);
    }
  }
}

async function main() {
  const settings = parseArgs(process.argv.slice(2));
  if (settings.help) {
    printHelp();
    process.exit(0);
  }

  const profileId =
    resolveDevProfileId(settings.deploymentProfile) || DEFAULT_DEV_PROFILE_ID;
  const profileEnv = loadProfile(profileId);
  ensurePostgresDevEnvFile(REPO_ROOT, { stdout: console });
  const postgresEnv = loadEnvFile('config/host/postgres.env', REPO_ROOT);
  const processes = listOrchestrationProcesses(profileId);
  const serviceProcess = processes.find((process) => process.id === 'application.public-ingress');
  if (!serviceProcess?.crate) {
    throw new Error(
      `orchestration profile ${profileId} is missing application.public-ingress crate`,
    );
  }

  const runtimeEnv = filterDiscoveryProcessEnv(
    mergeRuntimeEnv(process.env, profileEnv, postgresEnv, {
      SDKWORK_DISCOVERY_PROFILE_ID: profileId,
      SDKWORK_DISCOVERY_DEPLOYMENT_PROFILE: settings.deploymentProfile,
    }),
  );

  const command = cargoCommand();
  const args = ['run', '-p', serviceProcess.crate];

  console.log(`[sdkwork-discovery] profile=${profileId}`);
  console.log(
    `[sdkwork-discovery] application.public-ingress=${resolveSurfaceGrpcUrl(runtimeEnv) ?? runtimeEnv.SDKWORK_DISCOVERY_APPLICATION_PUBLIC_INGRESS_BIND ?? 'unset'}`,
  );

  if (settings.dryRun) {
    console.log(`[sdkwork-discovery] dry-run ${command} ${args.join(' ')}`);
    return;
  }

  const child = spawn(command, args, {
    cwd: REPO_ROOT,
    env: runtimeEnv,
    stdio: 'inherit',
    shell: false,
    windowsHide: true,
  });

  child.on('error', (error) => {
    console.error(`[sdkwork-discovery] failed to start service host: ${error.message}`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 0);
  });

  await waitForSurfaceHealth(profileId, runtimeEnv);
}

main().catch((error) => {
  console.error(`[sdkwork-discovery] ${error.message}`);
  process.exit(1);
});
