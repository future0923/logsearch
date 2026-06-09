import { mkdir, rm, cp, chmod, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { spawnSync } from 'node:child_process'

export const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const appName = 'log-search'
const version = (process.env.VERSION || '0.1.0').replace(/^v/, '')
const platform = process.env.RELEASE_PLATFORM || process.platform
const arch = process.env.RELEASE_ARCH || normalizedArch(process.arch)
const packageSuffix = process.env.RELEASE_PACKAGE_SUFFIX || `${platform}-${arch}`
const cargoTarget = process.env.RELEASE_CARGO_TARGET || ''
const distDir = join(rootDir, 'dist')
const releaseName = `${appName}_${version}_${packageSuffix}`
const releaseDir = join(distDir, releaseName)

function normalizedArch(value) {
  if (value === 'x64') return 'x64'
  if (value === 'arm64') return 'arm64'
  return value
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd || rootDir,
    shell: process.platform === 'win32',
    stdio: 'inherit',
    env: process.env,
  })
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed`)
  }
}

export function cargoBuildArgs(target = cargoTarget) {
  const args = ['build', '--release']
  if (target) {
    args.push('--target', target)
  }
  return args
}

export function cargoReleaseDir(target = cargoTarget) {
  return target
    ? join(rootDir, 'backend', 'target', target, 'release')
    : join(rootDir, 'backend', 'target', 'release')
}

async function copyIfExists(source, target) {
  try {
    await cp(source, target, { recursive: true })
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }
}

async function build() {
  await rm(releaseDir, { recursive: true, force: true })
  await mkdir(join(releaseDir, 'frontend'), { recursive: true })
  await mkdir(join(releaseDir, 'data'), { recursive: true })

  run('npm', ['ci'], { cwd: join(rootDir, 'frontend') })
  run('npm', ['run', 'build'], { cwd: join(rootDir, 'frontend') })
  run('cargo', cargoBuildArgs(), { cwd: join(rootDir, 'backend') })

  const binaryName = platform === 'windows' ? `${appName}.exe` : appName
  const builtBinaryName = platform === 'windows' ? 'backend.exe' : 'backend'
  await cp(join(cargoReleaseDir(), builtBinaryName), join(releaseDir, binaryName))
  await cp(join(rootDir, 'frontend', 'dist'), join(releaseDir, 'frontend'), { recursive: true })
  await cp(join(rootDir, 'config.example.toml'), join(releaseDir, 'config.toml'))
  await cp(join(rootDir, 'packaging', 'README.txt'), join(releaseDir, 'README.txt'))

  if (platform === 'windows') {
    await writeWindowsReadme()
  } else {
    await cp(join(rootDir, 'packaging', 'start.sh'), join(releaseDir, 'start.sh'))
    await cp(join(rootDir, 'packaging', 'stop.sh'), join(releaseDir, 'stop.sh'))
    await cp(join(rootDir, 'packaging', 'status.sh'), join(releaseDir, 'status.sh'))
    await cp(join(rootDir, 'packaging', 'upgrade.sh'), join(releaseDir, 'upgrade.sh'))
    await chmod(join(releaseDir, binaryName), 0o755)
    await chmod(join(releaseDir, 'start.sh'), 0o755)
    await chmod(join(releaseDir, 'stop.sh'), 0o755)
    await chmod(join(releaseDir, 'status.sh'), 0o755)
    await chmod(join(releaseDir, 'upgrade.sh'), 0o755)
    if (platform === 'linux') {
      await copyIfExists(join(rootDir, 'packaging', 'log-search.service'), join(releaseDir, 'log-search.service'))
    }
  }

  const archive = platform === 'windows'
    ? await createZipArchive()
    : await createTarGzArchive()

  console.log()
  console.log('Release created:')
  console.log(`  ${releaseDir}`)
  console.log(`  ${archive}`)
}

async function writeWindowsReadme() {
  const content = `Log Search Windows
==================

1. Edit config.toml and configure your log files.
2. Start Log Search from PowerShell:

   .\\log-search.exe --config config.toml --static-dir frontend

3. Open:

   http://127.0.0.1:12457

Keep the PowerShell window open while Log Search is running.
`
  await writeFile(join(releaseDir, 'README-windows.txt'), content)
}

async function createZipArchive() {
  const archivePath = join(distDir, `${releaseName}.zip`)
  await rm(archivePath, { force: true })
  run('powershell', [
    '-NoProfile',
    '-Command',
    `Compress-Archive -Path '${releaseDir.replaceAll("'", "''")}\\*' -DestinationPath '${archivePath.replaceAll("'", "''")}' -Force`,
  ])
  return archivePath
}

async function createTarGzArchive() {
  const archivePath = join(distDir, `${releaseName}.tar.gz`)
  await rm(archivePath, { force: true })
  run('tar', ['-C', dirname(releaseDir), '-czf', archivePath, basename(releaseDir)])
  return archivePath
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  build().catch((error) => {
    console.error(error)
    process.exit(1)
  })
}
