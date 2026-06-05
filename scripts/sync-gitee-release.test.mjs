import assert from 'node:assert/strict'
import { test } from 'node:test'
import { join } from 'node:path'
import {
  assetNamesForVersion,
  buildGithubAssetUrl,
  buildGiteeApiUrl,
  buildGiteeReleasePayload,
  parseEnvFile,
  parseArgs,
  releaseBody,
} from './sync-gitee-release.mjs'

test('parseArgs defaults to dry run for v0.1.0', () => {
  assert.deepEqual(parseArgs([]), {
    execute: false,
    githubOwner: 'future0923',
    githubRepo: 'logsearch',
    giteeOwner: 'future94',
    giteeRepo: 'logsearch',
    tag: 'v0.1.0',
    targetCommitish: 'main',
    outDir: join(process.cwd(), 'dist', 'gitee-release', 'v0.1.0'),
    downloadTimeoutMs: 120000,
  })
})

test('parseArgs enables writes only with execute flag', () => {
  assert.equal(parseArgs(['--execute']).execute, true)
  assert.equal(parseArgs(['--tag', 'v1.2.3']).tag, 'v1.2.3')
  assert.equal(parseArgs(['--target-commitish', '385a256']).targetCommitish, '385a256')
  assert.equal(parseArgs(['--download-timeout-ms', '5000']).downloadTimeoutMs, 5000)
})

test('assetNamesForVersion lists release archives without checksum file', () => {
  assert.deepEqual(assetNamesForVersion('v0.1.0'), [
    'log-search_0.1.0_darwin_amd64.tar.gz',
    'log-search_0.1.0_darwin_arm64.tar.gz',
    'log-search_0.1.0_linux_amd64.tar.gz',
    'log-search_0.1.0_linux_arm64.tar.gz',
    'log-search_0.1.0_windows_amd64.zip',
  ])
})

test('buildGithubAssetUrl points at downloadable GitHub release assets', () => {
  assert.equal(
    buildGithubAssetUrl({
      owner: 'future0923',
      repo: 'logsearch',
      tag: 'v0.1.0',
      assetName: 'log-search_0.1.0_linux_amd64.tar.gz',
    }),
    'https://github.com/future0923/logsearch/releases/download/v0.1.0/log-search_0.1.0_linux_amd64.tar.gz',
  )
})

test('buildGiteeApiUrl encodes repository paths', () => {
  assert.equal(
    buildGiteeApiUrl('/repos/:owner/:repo/releases', {
      owner: 'future94',
      repo: 'logsearch',
    }),
    'https://gitee.com/api/v5/repos/future94/logsearch/releases',
  )
})

test('releaseBody describes the release without public sync metadata', () => {
  const body = releaseBody({
    githubOwner: 'future0923',
    githubRepo: 'logsearch',
    tag: 'v0.1.0',
  })
  assert.match(body, /在多份应用日志里快速找关键字/)
  assert.match(body, /支持 tail -f 实时查看日志/)
  assert.doesNotMatch(body, /GitHub/i)
  assert.doesNotMatch(body, /同步/)
  assert.doesNotMatch(body, /musl/i)
  assert.doesNotMatch(body, /checksums\.txt/)
})

test('buildGiteeReleasePayload includes fields required by create and update APIs', () => {
  assert.deepEqual(
    buildGiteeReleasePayload({
      tag: 'v0.1.0',
      targetCommitish: 'main',
      body: 'release notes',
    }),
    {
      tag_name: 'v0.1.0',
      target_commitish: 'main',
      name: 'v0.1.0',
      body: 'release notes',
      prerelease: false,
    },
  )
})

test('parseEnvFile reads shell-style token assignments', () => {
  assert.deepEqual(parseEnvFile('GITEE_ACCESS_TOKEN=abc123\nOTHER="hello world"\n# ignored\n'), {
    GITEE_ACCESS_TOKEN: 'abc123',
    OTHER: 'hello world',
  })
})
