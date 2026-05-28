import { spawn } from 'node:child_process'

const [, , command, rawBaseUrl] = process.argv
const defaultBaseUrl = process.env.LOG_SEARCH_REMOTE

function usage() {
  console.log('Usage:')
  console.log('  npm run dev:remote')
  console.log('  npm run test:remote')
  console.log('  npm run dev:remote -- http://192.168.0.10:12457')
  console.log('  npm run test:remote -- http://192.168.0.10:12457')
}

function normalizeBaseUrl(value) {
  if (!value) return null
  const withProtocol = /^https?:\/\//.test(value) ? value : `http://${value}`
  try {
    const url = new URL(withProtocol)
    return url.origin
  } catch {
    return null
  }
}

async function testRemote(baseUrl) {
  const statusUrl = `${baseUrl}/api/status`
  console.log(`Testing ${statusUrl}`)

  const response = await fetch(statusUrl)
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`)
  }

  const payload = await response.json()
  console.log('Remote backend is reachable.')
  console.log(`Files: ${payload.files ?? 0}`)
  if (Array.isArray(payload.fileSources) && payload.fileSources.length) {
    console.log('Sources:')
    for (const source of payload.fileSources) {
      console.log(`  - ${source.id}: ${source.path}`)
    }
  }
}

function startDev(baseUrl) {
  console.log(`Starting frontend with remote backend: ${baseUrl}`)
  const child = spawn('npm', ['run', 'dev'], {
    env: {
      ...process.env,
      VITE_API_PROXY_TARGET: baseUrl,
    },
    shell: process.platform === 'win32',
    stdio: 'inherit',
  })

  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal)
      return
    }
    process.exit(code ?? 0)
  })
}

const baseUrl = normalizeBaseUrl(rawBaseUrl ?? defaultBaseUrl)
if (!baseUrl || (command !== 'dev' && command !== 'test')) {
  usage()
  process.exit(1)
}

if (command === 'dev') {
  startDev(baseUrl)
} else {
  testRemote(baseUrl).catch((error) => {
    console.error(`Remote backend test failed: ${error.message}`)
    process.exit(1)
  })
}
