import assert from 'node:assert/strict'
import { test } from 'node:test'
import { join } from 'node:path'
import { cargoBuildArgs, cargoReleaseDir, rootDir } from './package-release.mjs'

test('cargoBuildArgs builds default release binary without a target', () => {
  assert.deepEqual(cargoBuildArgs(''), ['build', '--release'])
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
