import { createWriteStream } from 'node:fs'
import { mkdir, readFile, rename, rm, stat } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const giteeBaseUrl = 'https://gitee.com/api/v5'

export function parseArgs(argv) {
  const options = {
    execute: false,
    githubOwner: 'future0923',
    githubRepo: 'logsearch',
    giteeOwner: 'future94',
    giteeRepo: 'logsearch',
    tag: 'v0.1.0',
    targetCommitish: 'main',
    outDir: '',
    downloadTimeoutMs: 120000,
  }

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--execute') {
      options.execute = true
    } else if (arg === '--tag') {
      options.tag = requiredValue(argv, ++index, arg)
    } else if (arg === '--target-commitish') {
      options.targetCommitish = requiredValue(argv, ++index, arg)
    } else if (arg === '--github-owner') {
      options.githubOwner = requiredValue(argv, ++index, arg)
    } else if (arg === '--github-repo') {
      options.githubRepo = requiredValue(argv, ++index, arg)
    } else if (arg === '--gitee-owner') {
      options.giteeOwner = requiredValue(argv, ++index, arg)
    } else if (arg === '--gitee-repo') {
      options.giteeRepo = requiredValue(argv, ++index, arg)
    } else if (arg === '--out-dir') {
      options.outDir = resolve(requiredValue(argv, ++index, arg))
    } else if (arg === '--download-timeout-ms') {
      options.downloadTimeoutMs = Number(requiredValue(argv, ++index, arg))
    } else if (arg === '--help' || arg === '-h') {
      options.help = true
    } else {
      throw new Error(`Unknown argument: ${arg}`)
    }
  }

  if (!options.outDir) {
    options.outDir = join(process.cwd(), 'dist', 'gitee-release', options.tag)
  }

  return options
}

function requiredValue(argv, index, name) {
  const value = argv[index]
  if (!value || value.startsWith('--')) {
    throw new Error(`${name} requires a value`)
  }
  return value
}

export function assetNamesForVersion(tag) {
  const version = tag.replace(/^v/, '')
  return [
    `log-search_${version}_darwin_amd64.tar.gz`,
    `log-search_${version}_darwin_arm64.tar.gz`,
    `log-search_${version}_linux_amd64.tar.gz`,
    `log-search_${version}_linux_arm64.tar.gz`,
    `log-search_${version}_windows_amd64.zip`,
  ]
}

export function buildGithubAssetUrl({ owner, repo, tag, assetName }) {
  return `https://github.com/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(assetName)}`
}

export function buildGiteeApiUrl(path, params = {}) {
  let resolvedPath = path
  for (const [key, value] of Object.entries(params)) {
    resolvedPath = resolvedPath.replace(`:${key}`, encodeURIComponent(value))
  }
  return `${giteeBaseUrl}${resolvedPath}`
}

export function releaseBody({ tag, notes = '' }) {
  const sections = [`# Log Search ${tag}`]
  const trimmedNotes = notes.trim()
  if (trimmedNotes) {
    sections.push(trimmedNotes)
  } else {
    sections.push([
      '- 在多份应用日志里快速找关键字。',
      '- 搜索错误、链路 ID、订单号、用户 ID、类名、接口名。',
      '- 用 `AND` / `OR` 组合条件缩小范围。',
      '- 点开命中行，直接查看前后上下文。',
      '- 日志持续写入时，索引自动更新。',
      '- 支持多种压缩格式（gz、zst、bz2、xz 等）搜索。',
      '- 支持 tail -f 实时查看日志。',
    ].join('\n'))
  }

  sections.push([
    '## 下载文件',
    ...assetNamesForVersion(tag).map((assetName) => `- ${assetName}`),
  ].join('\n'))

  return sections.join('\n\n')
}

function defaultGithubReleaseNotes(tag) {
  return [
    `发布版本 ${tag}。`,
    '',
    '- 在多份应用日志里快速找关键字。',
    '- 搜索错误、链路 ID、订单号、用户 ID、类名、接口名。',
    '- 用 `AND` / `OR` 组合条件缩小范围。',
    '- 点开命中行，直接查看前后上下文。',
    '- 日志持续写入时，索引自动更新。',
    '- 支持多种压缩格式（gz、zst、bz2、xz 等）搜索。',
    '- 支持 tail -f 实时查看日志。',
  ].join('\n')
}

export function buildGiteeReleasePayload({ tag, targetCommitish, body }) {
  return {
    tag_name: tag,
    target_commitish: targetCommitish,
    name: tag,
    body,
    prerelease: false,
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  if (options.help) {
    console.log(usage())
    return
  }

  const env = await loadLocalEnv()
  const token = process.env.GITEE_ACCESS_TOKEN || env.GITEE_ACCESS_TOKEN || ''
  if (options.execute && !token) {
    throw new Error('GITEE_ACCESS_TOKEN is required when --execute is used. Put it in .env.local or export it.')
  }

  await syncGiteeRelease(options, token)
}

async function loadLocalEnv() {
  try {
    const content = await readFile(join(rootDir, '.env.local'), 'utf8')
    return parseEnvFile(content)
  } catch (error) {
    if (error.code === 'ENOENT') {
      return {}
    }
    throw error
  }
}

export function parseEnvFile(content) {
  const values = {}
  for (const rawLine of content.split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line || line.startsWith('#')) continue
    const separator = line.indexOf('=')
    if (separator === -1) continue
    const key = line.slice(0, separator).trim()
    const rawValue = line.slice(separator + 1).trim()
    if (!key) continue
    values[key] = unquoteEnvValue(rawValue)
  }
  return values
}

function unquoteEnvValue(value) {
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    return value.slice(1, -1)
  }
  return value
}

async function syncGiteeRelease(options, token) {
  const releaseAssets = assetNamesForVersion(options.tag)
  await mkdir(options.outDir, { recursive: true })

  console.log(`Mode: ${options.execute ? 'execute' : 'dry-run'}`)
  console.log(`GitHub: ${options.githubOwner}/${options.githubRepo} ${options.tag}`)
  console.log(`Gitee:  ${options.giteeOwner}/${options.giteeRepo} ${options.tag}`)
  console.log(`Output: ${options.outDir}`)
  console.log()

  for (const assetName of releaseAssets) {
    const url = buildGithubAssetUrl({
      owner: options.githubOwner,
      repo: options.githubRepo,
      tag: options.tag,
      assetName,
    })
    await downloadFile(url, join(options.outDir, assetName), options.downloadTimeoutMs)
  }

  if (!options.execute) {
    console.log()
    console.log('Dry run complete. Would sync these assets to Gitee:')
    for (const assetName of releaseAssets) {
      console.log(`  - ${assetName}`)
    }
    console.log()
    console.log('Run again with --execute and GITEE_ACCESS_TOKEN to update Gitee.')
    return
  }

  const release = await ensureGiteeRelease(options, token)
  await replaceGiteeAssets(options, token, release, releaseAssets)
  console.log()
  console.log(`Gitee release synced: ${options.tag}`)
}

async function downloadFile(url, targetPath, timeoutMs) {
  try {
    const current = await stat(targetPath)
    if (current.size > 0) {
      console.log(`Using existing ${basename(targetPath)}`)
      return
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }

  console.log(`Downloading ${basename(targetPath)}`)
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), timeoutMs)
  const partialPath = `${targetPath}.part`
  try {
    const response = await fetch(url, {
      redirect: 'follow',
      signal: controller.signal,
    })
    if (!response.ok) {
      throw new Error(`Download failed ${response.status} ${response.statusText}: ${url}`)
    }
    await pipeline(response.body, createWriteStream(partialPath))
    await rm(targetPath, { force: true })
    await rename(partialPath, targetPath)
  } catch (error) {
    await rm(partialPath, { force: true })
    if (error.name === 'AbortError') {
      throw new Error(`Download timed out after ${timeoutMs}ms: ${url}`)
    }
    throw error
  } finally {
    clearTimeout(timeout)
  }
}

async function ensureGiteeRelease(options, token) {
  const existing = await findGiteeRelease(options, token)
  const githubNotes = await fetchGithubReleaseNotes(options)
  const body = releaseBody({
    tag: options.tag,
    notes: githubNotes,
  })
  const releasePayload = buildGiteeReleasePayload({
    tag: options.tag,
    targetCommitish: options.targetCommitish,
    body,
  })

  if (!existing) {
    console.log(`Creating Gitee release ${options.tag}`)
    return giteeRequest('/repos/:owner/:repo/releases', options, token, {
      method: 'POST',
      body: releasePayload,
    })
  }

  console.log(`Updating Gitee release ${options.tag}`)
  return giteeRequest('/repos/:owner/:repo/releases/:id', { ...options, id: existing.id }, token, {
    method: 'PATCH',
    body: releasePayload,
  })
}

async function fetchGithubReleaseNotes(options) {
  const url = `https://api.github.com/repos/${encodeURIComponent(options.githubOwner)}/${encodeURIComponent(options.githubRepo)}/releases/tags/${encodeURIComponent(options.tag)}`
  try {
    const response = await fetch(url, { headers: { Accept: 'application/vnd.github+json' } })
    if (!response.ok) {
      return defaultGithubReleaseNotes(options.tag)
    }
    const release = await response.json()
    return release.body || defaultGithubReleaseNotes(options.tag)
  } catch {
    return defaultGithubReleaseNotes(options.tag)
  }
}

async function findGiteeRelease(options, token) {
  const releases = await giteeRequest('/repos/:owner/:repo/releases', options, token)
  return releases.find((release) => release.tag_name === options.tag) || null
}

async function replaceGiteeAssets(options, token, release, assetNames) {
  const assets = await giteeRequest('/repos/:owner/:repo/releases/:id/attach_files', {
    ...options,
    id: release.id,
  }, token)
  const namesToDelete = new Set([...assetNames, 'checksums.txt'])

  for (const asset of assets) {
    if (namesToDelete.has(asset.name)) {
      console.log(`Deleting existing Gitee asset ${asset.name}`)
      await giteeRequest('/repos/:owner/:repo/releases/:id/attach_files/:attachId', {
        ...options,
        id: release.id,
        attachId: asset.id,
      }, token, { method: 'DELETE' })
    }
  }

  for (const assetName of assetNames) {
    console.log(`Uploading Gitee asset ${assetName}`)
    await uploadGiteeAsset(options, token, release.id, join(options.outDir, assetName))
  }
}

async function uploadGiteeAsset(options, token, releaseId, filePath) {
  const form = new FormData()
  const bytes = await readFile(filePath)
  form.set('access_token', token)
  form.set('file', new Blob([bytes]), basename(filePath))

  const url = buildGiteeApiUrl('/repos/:owner/:repo/releases/:id/attach_files', {
    owner: options.giteeOwner,
    repo: options.giteeRepo,
    id: String(releaseId),
  })
  const response = await fetch(url, { method: 'POST', body: form })
  if (!response.ok) {
    throw new Error(await responseError(response, 'Upload failed'))
  }
  return response.json()
}

async function giteeRequest(path, options, token, request = {}) {
  const url = new URL(buildGiteeApiUrl(path, {
    owner: options.giteeOwner,
    repo: options.giteeRepo,
    id: String(options.id || ''),
    attachId: String(options.attachId || ''),
  }))
  url.searchParams.set('access_token', token)

  const init = { method: request.method || 'GET' }
  if (request.body) {
    init.headers = { 'Content-Type': 'application/json' }
    init.body = JSON.stringify({ access_token: token, ...request.body })
  }

  const response = await fetch(url, init)
  if (!response.ok) {
    throw new Error(await responseError(response, 'Gitee API request failed'))
  }
  if (response.status === 204) {
    return null
  }
  return response.json()
}

async function responseError(response, prefix) {
  const text = await response.text()
  return `${prefix}: ${response.status} ${response.statusText}${text ? `\n${text}` : ''}`
}

function usage() {
  return `Usage:
  node scripts/sync-gitee-release.mjs [options]

Token:
  Put GITEE_ACCESS_TOKEN in .env.local, or export it in the shell.

Options:
  --execute                 Write changes to Gitee. Without this, only dry-runs.
  --tag <tag>               Release tag to sync. Default: v0.1.0
  --target-commitish <ref>  Gitee release target ref. Default: main
  --github-owner <owner>    GitHub owner. Default: future0923
  --github-repo <repo>      GitHub repo. Default: logsearch
  --gitee-owner <owner>     Gitee owner. Default: future94
  --gitee-repo <repo>       Gitee repo. Default: logsearch
  --out-dir <path>          Download directory. Default: dist/gitee-release/<tag>
`
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error.message || error)
    process.exit(1)
  })
}
