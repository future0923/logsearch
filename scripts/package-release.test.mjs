import assert from 'node:assert/strict'
import { test } from 'node:test'
import { join } from 'node:path'
import { readFile } from 'node:fs/promises'
import { cargoBuildArgs, cargoReleaseDir, rootDir } from './package-release.mjs'

test('cargoBuildArgs builds default release binary without a target', () => {
  assert.deepEqual(cargoBuildArgs(''), ['build', '--release'])
})

test('release workflow avoids Cargo internal target environment variable', async () => {
  const workflow = await readFile(join(rootDir, '.github', 'workflows', 'release.yml'), 'utf8')
  assert.match(workflow, /RELEASE_CARGO_TARGET:/)
  assert.doesNotMatch(workflow, /CARGO_BUILD_TARGET:/)
})

test('release package includes executable upgrade script', async () => {
  const script = await readFile(join(rootDir, 'scripts', 'package-release.mjs'), 'utf8')
  assert.match(script, /packaging', 'upgrade\.sh'/)
  assert.match(script, /releaseDir, 'upgrade\.sh'/)
  assert.match(script, /chmod\(join\(releaseDir, 'upgrade\.sh'\), 0o755\)/)
})

test('cargoBuildArgs adds the configured Rust target', () => {
  assert.deepEqual(cargoBuildArgs('x86_64-unknown-linux-musl'), [
    'build',
    '--release',
    '--target',
    'x86_64-unknown-linux-musl',
  ])
})

test('cargoReleaseDir reads target-specific release binaries when cross compiling', () => {
  assert.equal(
    cargoReleaseDir('x86_64-unknown-linux-musl'),
    join(rootDir, 'backend', 'target', 'x86_64-unknown-linux-musl', 'release'),
  )
})
