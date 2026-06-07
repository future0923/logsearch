<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import './App.css'
import Icon from './components/Icon.vue'
import PreviewPane from './components/PreviewPane.vue'
import { authenticationChallengeUrl, isAuthenticationChallenge } from './domain/auth'
import { buildHighlightRegex, highlightedParts, queryTokens } from './domain/highlight'
import { getStoredTheme, THEME_STORAGE_KEY, toggleTheme, type ThemeMode } from './domain/theme'
import {
  fallbackContext,
  filePickerSelectionLabel,
  filePickerSelectionTitle,
  filterFileSources,
  isCompressedKind,
  resultKey,
  selectAllVisibleFileSources,
  shortPath,
  toggleFileSelection,
  type AroundResponse,
  type ContextLine,
  type DirectorySource,
  type FileSource,
  type LoadDirection,
  type PreviewMode,
  type SearchHit,
  type SearchResponse,
  type StatusResponse,
  type TailEventPayload,
  type TailLine,
} from './domain/logs'
import { isOverlaySelfClick, shouldOpenResultFromClick } from './resultInteraction'

const API_BASE = import.meta.env.VITE_API_BASE ?? ''
const CONTEXT_OPTIONS = [0, 2, 5, 10, 20]
const INITIAL_EXPAND_LINES = 50
const SEARCH_PAGE_SIZE = 20
const COLLAPSIBLE_RESULT_LENGTH = 180
const TAIL_LINE_OPTIONS = [10, 50, 100, 200, 500, 1000]
const MAX_TAIL_LINES = 2000

type PointerPoint = {
  x: number
  y: number
}

const query = ref('')
const theme = ref<ThemeMode>('light')
const regex = ref(false)
const caseInsensitive = ref(true)
const wholeWord = ref(false)
const contextRows = ref(2)
const results = ref<SearchResponse | null>(null)
const selected = ref<number | null>(null)
const loading = ref(false)
const loadingMore = ref(false)
const aroundLoading = ref(false)
const error = ref<string | null>(null)
const previewMode = ref<PreviewMode>('search')
const previewMaximized = ref(false)
const around = ref<AroundResponse | null>(null)
const expandRows = ref(50)
const aroundBefore = ref(INITIAL_EXPAND_LINES)
const aroundAfter = ref(INITIAL_EXPAND_LINES)
const fileSources = ref<FileSource[]>([])
const configuredDirectories = ref<DirectorySource[]>([])
const discoveredFiles = ref<FileSource[]>([])
const discoveredFilesTruncated = ref(false)
const showWatchedFiles = ref(false)
const selectedFileIds = ref<string[]>([])
const filePickerOpen = ref(false)
const filePickerSearch = ref('')
const expandedResultKeys = ref<Set<string>>(new Set())
const tailFile = ref<FileSource | null>(null)
const tailInitialLines = ref(10)
const tailLines = ref<TailLine[]>([])
const tailOffset = ref<number | null>(null)
const tailPaused = ref(false)
const tailAutoScroll = ref(true)
const tailMaximized = ref(false)
const tailError = ref<string | null>(null)
const appReady = ref(false)
const authPending = ref(false)
const authMessage = ref('正在验证访问权限')
let authChallengeCount = 0

const resultsRef = ref<HTMLElement | null>(null)
const filePickerRef = ref<HTMLDivElement | null>(null)
const filePickerInputRef = ref<HTMLInputElement | null>(null)
const tailViewportRef = ref<HTMLDivElement | null>(null)
let tailEventSource: EventSource | null = null
let tailOffsetValue: number | null = null
let tailNextLineNoValue: number | null = null
let activeTailInitialLines = 10
let resultPointerDown: PointerPoint | null = null

const selectedHit = computed(() => selected.value === null ? null : results.value?.hits[selected.value] ?? null)
const highlightRegex = computed(() => buildHighlightRegex(query.value, regex.value, caseInsensitive.value, wholeWord.value))
const previewLines = computed(() => {
  if (previewMode.value === 'around' && around.value) return around.value.lines
  if (!selectedHit.value) return []
  return selectedHit.value.context?.length ? selectedHit.value.context : fallbackContext(selectedHit.value)
})
const status = computed(() => {
  if (loading.value) return '正在检索索引'
  if (error.value) return '检索失败'
  if (!results.value) return '就绪'
  return `${results.value.total} 条结果 / ${results.value.elapsedMs} ms`
})
const tailStatus = computed(() => {
  if (tailOffset.value === null) return '连接中'
  return `${tailLines.value.length} 行`
})
const selectedFileIdSet = computed(() => new Set(selectedFileIds.value))
const selectedFileScopeLabel = computed(() => filePickerSelectionLabel(fileSources.value, selectedFileIds.value))
const selectedFileScopeTitle = computed(() => filePickerSelectionTitle(fileSources.value, selectedFileIds.value))
const filteredFileSources = computed(() => filterFileSources(fileSources.value, filePickerSearch.value))
const allFilesExplicitlySelected = computed(() => (
  filteredFileSources.value.length > 0 && filteredFileSources.value.every((file) => selectedFileIdSet.value.has(file.id))
))
const visibleFileSourceCount = computed(() => filteredFileSources.value.length)
const hotFileCount = computed(() => fileSources.value.filter((file) => file.kind === 'hot').length)
const compressedFileCount = computed(() => fileSources.value.filter((file) => isCompressedKind(file.kind)).length)
const activeDirectoryCount = computed(() => configuredDirectories.value.filter((directory) => directory.exists).length)

onMounted(() => {
  const storedTheme = getStoredTheme(window.localStorage)
  if (storedTheme) theme.value = storedTheme
  void loadStatus()
  document.addEventListener('mousedown', closeFilePickerOnOutsideClick)
})

onBeforeUnmount(() => {
  tailEventSource?.close()
  document.removeEventListener('mousedown', closeFilePickerOnOutsideClick)
})

watch(filePickerOpen, async (open) => {
  if (!open) return
  await nextTick()
  filePickerInputRef.value?.focus()
})

watch([tailFile, tailPaused], () => {
  connectTail()
})

watch(theme, (value) => {
  window.localStorage.setItem(THEME_STORAGE_KEY, value)
})

watch([tailLines, tailAutoScroll, tailPaused], async () => {
  if (!tailLines.value.length || tailPaused.value || !tailAutoScroll.value) return
  await nextTick()
  const viewport = tailViewportRef.value
  if (viewport) viewport.scrollTop = viewport.scrollHeight
}, { deep: false })

async function loadStatus() {
  try {
    const response = await fetch(`${API_BASE}/api/status`)
    if (!response.ok) {
      if (isAuthenticationChallenge(response.status)) {
        authPending.value = true
        authMessage.value = '需要登录'
        return
      }
      appReady.value = true
      return
    }
    const payload = (await response.json()) as StatusResponse
    fileSources.value = payload.fileSources ?? []
    configuredDirectories.value = payload.configuredDirectories ?? []
    discoveredFiles.value = payload.discoveredFiles ?? []
    discoveredFilesTruncated.value = Boolean(payload.discoveredFilesTruncated)
    appReady.value = true
    authPending.value = false
  } catch {
    appReady.value = true
    fileSources.value = []
    configuredDirectories.value = []
    discoveredFiles.value = []
    discoveredFilesTruncated.value = false
  }
}

function retryAuthentication() {
  authPending.value = false
  authMessage.value = '正在验证访问权限'
  authChallengeCount += 1
  window.location.assign(authenticationChallengeUrl(API_BASE, window.location.href, String(authChallengeCount)))
}

function connectTail() {
  tailEventSource?.close()
  tailEventSource = null

  if (!tailFile.value || tailPaused.value) return

  const params = new URLSearchParams({
    fileId: tailFile.value.id,
    lines: String(activeTailInitialLines),
  })
  if (tailOffsetValue !== null && tailNextLineNoValue !== null) {
    params.set('offset', String(tailOffsetValue))
    params.set('nextLineNo', String(tailNextLineNoValue))
  }

  const source = new EventSource(`${API_BASE}/api/tail?${params.toString()}`)
  tailEventSource = source

  source.addEventListener('tail', (event) => {
    const payload = JSON.parse((event as MessageEvent).data) as TailEventPayload
    tailOffsetValue = payload.offset
    tailNextLineNoValue = payload.nextLineNo
    tailOffset.value = payload.offset
    if (payload.lines.length) {
      tailLines.value = [...tailLines.value, ...payload.lines].slice(-MAX_TAIL_LINES)
    }
  })

  source.addEventListener('error', () => {
    tailError.value = 'Tail 连接已中断'
  })
}

function closeFilePickerOnOutsideClick(event: MouseEvent) {
  if (!filePickerOpen.value) return
  if (!filePickerRef.value?.contains(event.target as Node)) {
    filePickerOpen.value = false
  }
}

function selectFileScope(fileId: string) {
  if (fileId === 'all') {
    selectedFileIds.value = selectAllVisibleFileSources(fileSources.value, selectedFileIds.value, filePickerSearch.value)
    return
  }
  selectedFileIds.value = toggleFileSelection(selectedFileIds.value, fileId)
}

function startTail(file: FileSource) {
  activeTailInitialLines = tailInitialLines.value
  tailFile.value = file
  tailLines.value = []
  tailOffset.value = null
  tailOffsetValue = null
  tailNextLineNoValue = null
  tailPaused.value = false
  tailAutoScroll.value = true
  tailMaximized.value = false
  tailError.value = null
}

function toggleTailPaused() {
  if (tailPaused.value) tailError.value = null
  tailPaused.value = !tailPaused.value
}

function closeTail() {
  tailEventSource?.close()
  tailEventSource = null
  tailFile.value = null
  tailLines.value = []
  tailOffset.value = null
  tailOffsetValue = null
  tailNextLineNoValue = null
  tailPaused.value = false
  tailAutoScroll.value = true
  tailMaximized.value = false
  tailError.value = null
}

async function fetchSearchPage(cursor: string | null) {
  const response = await fetch(`${API_BASE}/api/search`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      query: query.value,
      regex: regex.value,
      caseInsensitive: caseInsensitive.value,
      wholeWord: wholeWord.value,
      limit: SEARCH_PAGE_SIZE,
      cursor,
      fileIds: selectedFileIds.value,
      contextBefore: 0,
      contextAfter: 0,
    }),
  })

  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`)
  }

  return (await response.json()) as SearchResponse
}

async function runSearch() {
  loading.value = true
  error.value = null
  try {
    const payload = await fetchSearchPage(null)
    results.value = payload
    selected.value = null
    previewMode.value = 'search'
    around.value = null
    expandedResultKeys.value = new Set()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Unknown error'
  } finally {
    loading.value = false
  }
}

async function loadMoreResults() {
  if (!results.value?.hasNext || !results.value.nextCursor || loadingMore.value || loading.value) return

  loadingMore.value = true
  error.value = null
  try {
    const current = results.value
    const payload = await fetchSearchPage(current.nextCursor)
    results.value = {
      ...payload,
      hits: [...current.hits, ...payload.hits],
      total: current.hits.length + payload.hits.length,
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Unknown error'
  } finally {
    loadingMore.value = false
  }
}

async function loadMoreResultsAndScroll() {
  const viewport = resultsRef.value
  const previousScrollHeight = viewport?.scrollHeight ?? 0
  await loadMoreResults()
  requestAnimationFrame(() => {
    const current = resultsRef.value
    if (!current) return
    const addedHeight = current.scrollHeight - previousScrollHeight
    current.scrollBy({ top: Math.max(addedHeight, current.clientHeight * 0.8), behavior: 'smooth' })
  })
}

async function loadAround(
  before = aroundBefore.value,
  after = aroundAfter.value,
  direction: LoadDirection = 'both',
) {
  if (!selectedHit.value || aroundLoading.value) return
  await loadAroundForHit(selectedHit.value, before, after, direction)
}

function activePreviewViewport() {
  return document.querySelector<HTMLDivElement>('.previewOverlay .logViewport')
}

async function loadAroundForHit(
  hit: SearchHit,
  before: number,
  after: number,
  direction: LoadDirection,
) {
  const previousViewport = activePreviewViewport()
  const previousScrollHeight = previousViewport?.scrollHeight ?? 0
  const previousScrollTop = previousViewport?.scrollTop ?? 0
  aroundLoading.value = true
  error.value = null
  try {
    const response = await fetch(`${API_BASE}/api/around`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        path: hit.path,
        lineNo: hit.lineNo,
        offset: hit.offset,
        compressed: isCompressedKind(hit.kind),
        before,
        after,
      }),
    })

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }

    around.value = (await response.json()) as AroundResponse
    aroundBefore.value = before
    aroundAfter.value = after
    previewMode.value = 'around'
    await nextTick()
    const viewport = activePreviewViewport()
    if (!viewport) return
    if (direction === 'before') {
      viewport.scrollTop = viewport.scrollHeight - previousScrollHeight + previousScrollTop
    } else if (direction === 'both') {
      viewport.querySelector('.matchLine')?.scrollIntoView({ block: 'center' })
    }
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Unknown error'
  } finally {
    aroundLoading.value = false
  }
}

async function scrollResults(direction: 'previous' | 'next') {
  const viewport = resultsRef.value
  if (!viewport) return

  const distance = Math.max(viewport.clientHeight * 0.82, 160)
  if (direction === 'previous') {
    viewport.scrollBy({ top: -distance, behavior: 'smooth' })
    return
  }

  const remaining = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight
  if (remaining <= 24 && results.value?.hasNext && !loadingMore.value && !loading.value) {
    await loadMoreResultsAndScroll()
    return
  }
  viewport.scrollBy({ top: distance, behavior: 'smooth' })
}

function resetPreviewState() {
  previewMode.value = 'search'
  around.value = null
  aroundBefore.value = INITIAL_EXPAND_LINES
  aroundAfter.value = INITIAL_EXPAND_LINES
}

function selectHit(index: number) {
  const hit = results.value?.hits[index]
  selected.value = index
  previewMaximized.value = false
  resetPreviewState()
  if (hit && contextRows.value > 0) {
    void loadAroundForHit(hit, contextRows.value, contextRows.value, 'both')
  }
}

function toggleResultExpansion(hit: SearchHit) {
  const key = resultKey(hit)
  const next = new Set(expandedResultKeys.value)
  if (next.has(key)) {
    next.delete(key)
  } else {
    next.add(key)
  }
  expandedResultKeys.value = next
}

function selectHitFromKeyboard(event: KeyboardEvent, index: number) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  selectHit(index)
}

function hasSelectedText() {
  return Boolean(window.getSelection()?.toString())
}

function closePreview() {
  selected.value = null
  previewMaximized.value = false
  resetPreviewState()
}

function closePreviewOnOverlayClick(event: MouseEvent) {
  if (isOverlaySelfClick(event)) closePreview()
}

function closeTailOnOverlayClick(event: MouseEvent) {
  if (isOverlaySelfClick(event)) closeTail()
}

function expandBefore() {
  void loadAround(aroundBefore.value + expandRows.value, aroundAfter.value, 'before')
}

function expandAfter() {
  void loadAround(aroundBefore.value, aroundAfter.value + expandRows.value, 'after')
}

function expandBoth() {
  void loadAround(aroundBefore.value + expandRows.value, aroundAfter.value + expandRows.value, 'both')
}

function openResultFromPointer(event: MouseEvent, index: number) {
  if (!shouldOpenResultFromClick({
    hasTextSelection: hasSelectedText(),
    pointerDown: resultPointerDown,
    pointerUp: { x: event.clientX, y: event.clientY },
  })) {
    return
  }
  selectHit(index)
}

function rememberResultPointer(event: PointerEvent) {
  resultPointerDown = { x: event.clientX, y: event.clientY }
}

function lineParts(line: ContextLine | TailLine | SearchHit) {
  return highlightedParts(line.content, highlightRegex.value)
}

function sourceLabel(file: FileSource) {
  return file.source === 'directory' ? file.directoryId : '文件'
}

function switchTheme() {
  theme.value = toggleTheme(theme.value)
}
</script>

<template>
  <main class="shell" :data-theme="theme">
    <section v-if="!appReady" class="authGate" aria-live="polite">
      <div class="authGatePanel">
        <div class="brandMark">LOG</div>
        <strong>{{ authMessage }}</strong>
        <button v-if="authPending" type="button" @click="retryAuthentication">登录</button>
      </div>
    </section>

    <section v-else class="workspace">
      <div class="topBar">
        <header class="appHeader">
          <div class="appIdentity">
            <div class="brandMark">LOG</div>
            <div>
              <h1>日志检索</h1>
              <p>索引日志搜索</p>
            </div>
          </div>
          <button class="themeToggle" type="button" :aria-label="theme === 'light' ? '切换到暗色模式' : '切换到浅色模式'" @click="switchTheme">
            <Icon class="themeToggleIcon" :name="theme === 'light' ? 'moon' : 'sun'" />
            <span>{{ theme === 'light' ? '暗色模式' : '浅色模式' }}</span>
          </button>
        </header>

        <form class="searchBar" @submit.prevent="runSearch">
          <div class="searchInputWrap">
            <span class="prompt">$</span>
            <div class="queryEditor">
              <div class="queryInputHighlight" aria-hidden="true">
                <span v-for="(token, index) in queryTokens(query)" :key="token + '-' + index">
                  <span v-if="token === 'AND' || token === 'OR'" class="queryOperator">{{ token }}</span>
                  <span v-else-if="token === '(' || token === ')'" class="queryParen">{{ token }}</span>
                  <span v-else>{{ token }}</span>
                </span>
              </div>
              <input
                id="log-query"
                v-model="query"
                placeholder="搜索日志、ID、类名、路径或正则"
                autofocus
              >
            </div>
          </div>
          <button type="submit" :disabled="loading">
            {{ loading ? '检索中' : '检索' }}
          </button>
        </form>
      </div>

      <div class="controls" aria-label="检索选项">
        <label>
          <input v-model="caseInsensitive" type="checkbox">
          忽略大小写
        </label>
        <label>
          <input v-model="wholeWord" type="checkbox">
          全词匹配
        </label>
        <label>
          <input v-model="regex" type="checkbox">
          正则
        </label>
        <label class="selectControl">
          上下文
          <select v-model.number="contextRows">
            <option v-for="value in CONTEXT_OPTIONS" :key="value" :value="value">
              {{ value }}
            </option>
          </select>
        </label>
        <div v-if="fileSources.length" ref="filePickerRef" class="filePickerField">
          <span class="filePickerLabel">文件</span>
          <div class="filePickerControl">
            <button
              type="button"
              class="filePickerTrigger"
              :class="{ open: filePickerOpen }"
              aria-haspopup="listbox"
              :aria-expanded="filePickerOpen"
              @click="filePickerOpen = !filePickerOpen; filePickerSearch = ''"
            >
              <span class="filePickerTriggerText" :title="selectedFileScopeTitle">
                {{ selectedFileScopeLabel }}
              </span>
              <span class="filePickerChevron" aria-hidden="true">⌄</span>
            </button>
            <div v-if="filePickerOpen" class="filePickerMenu">
              <input
                ref="filePickerInputRef"
                v-model="filePickerSearch"
                class="filePickerSearch"
                placeholder="搜索文件 ID、路径、类型..."
                @keydown.escape="filePickerOpen = false"
              >
              <div class="filePickerList" role="listbox" aria-label="日志文件">
                <button
                  type="button"
                  class="filePickerOption"
                  :class="{ selected: allFilesExplicitlySelected }"
                  role="option"
                  :aria-selected="allFilesExplicitlySelected"
                  @click="selectFileScope('all')"
                >
                  <span class="filePickerOptionMain">
                    <span class="filePickerCheck" :class="{ checked: allFilesExplicitlySelected }" aria-hidden="true"></span>
                    <span>全部文件</span>
                  </span>
                  <span class="filePickerOptionMeta">{{ visibleFileSourceCount }} 个来源</span>
                </button>
                <button
                  v-for="file in filteredFileSources"
                  :key="file.id"
                  type="button"
                  class="filePickerOption"
                  :class="{ selected: selectedFileIdSet.has(file.id) }"
                  role="option"
                  :aria-selected="selectedFileIdSet.has(file.id)"
                  @click="selectFileScope(file.id)"
                >
                  <span class="filePickerOptionMain">
                    <span class="filePickerCheck" :class="{ checked: selectedFileIdSet.has(file.id) }" aria-hidden="true"></span>
                    <span class="kindPill" :class="{ compressed: isCompressedKind(file.kind) }">{{ file.kind }}</span>
                    <strong>{{ file.id }}</strong>
                    <span>{{ shortPath(file.path) }}</span>
                  </span>
                  <span class="filePickerOptionMeta" :title="file.path">
                    {{ sourceLabel(file) }} · {{ file.exists ? '就绪' : '缺失' }} · {{ file.path }}
                  </span>
                </button>
                <div v-if="!filteredFileSources.length" class="filePickerEmpty">
                  没有匹配 "{{ filePickerSearch }}" 的文件
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="statusLine">
        <span>{{ configuredDirectories.length }} 个目录</span>
        <span>{{ discoveredFiles.length }}{{ discoveredFilesTruncated ? '+' : '' }} 个监听文件</span>
        <span v-if="results?.hasNext">还有更多</span>
        <span v-if="selectedHit && selected !== null">{{ selected + 1 }} / {{ results?.hits.length }}</span>
        <span v-if="previewMode === 'around'">{{ previewLines.length }} 行预览</span>
        <span v-if="aroundLoading">正在加载行</span>
        <button type="button" class="linkButton" @click="showWatchedFiles = !showWatchedFiles">
          {{ showWatchedFiles ? '隐藏监听' : '显示监听' }}
        </button>
      </div>

      <section v-if="showWatchedFiles" class="watchedPanel" aria-label="监听文件">
        <div class="watchedSummary">
          <span>{{ hotFileCount }} 个实时文件</span>
          <span>{{ compressedFileCount }} 个压缩文件</span>
          <span>{{ activeDirectoryCount }} 个有效目录</span>
          <label class="tailLineSelect">
            初始行数
            <select v-model.number="tailInitialLines">
              <option v-for="value in TAIL_LINE_OPTIONS" :key="value" :value="value">
                {{ value }}
              </option>
            </select>
          </label>
        </div>
        <div class="watchedTable">
          <div v-for="file in fileSources" :key="file.id" class="watchedRow">
            <span :class="file.exists ? 'stateOk' : 'stateMissing'">{{ file.exists ? '就绪' : '缺失' }}</span>
            <span class="kindPill" :class="{ compressed: isCompressedKind(file.kind) }">{{ file.kind }}</span>
            <span class="watchedName">{{ file.id }}</span>
            <span class="watchedSource">{{ sourceLabel(file) }}</span>
            <span class="watchedPath" :title="file.path">{{ file.path }}</span>
            <button
              class="tailButton"
              type="button"
              :disabled="!file.exists || file.kind !== 'hot'"
              @click="startTail(file)"
            >
              追踪
            </button>
          </div>
          <div v-if="!fileSources.length" class="watchedEmpty">暂无监听文件</div>
        </div>
      </section>

      <div v-if="error" class="errorPanel">{{ error }}</div>

      <div v-if="results" class="resultLayout resultsOnly">
        <section ref="resultsRef" class="results">
          <div v-if="results.hits.length" class="resultsHeader">
            <span>{{ status }}</span>
            <div class="resultNav">
              <button type="button" @click="scrollResults('previous')">上一屏</button>
              <button type="button" :disabled="loadingMore" @click="scrollResults('next')">
                {{ loadingMore ? '加载中' : '下一屏' }}
              </button>
            </div>
          </div>

          <div v-if="results.hits.length">
            <div
              v-for="(hit, index) in results.hits"
              :key="resultKey(hit)"
              class="resultRow"
              :class="{
                active: index === selected,
                collapsible: hit.content.length > COLLAPSIBLE_RESULT_LENGTH,
                collapsed: hit.content.length > COLLAPSIBLE_RESULT_LENGTH && !expandedResultKeys.has(resultKey(hit)),
              }"
              role="button"
              tabindex="0"
              @pointerdown="rememberResultPointer"
              @click="openResultFromPointer($event, index)"
              @keydown="selectHitFromKeyboard($event, index)"
            >
              <button
                v-if="hit.content.length > COLLAPSIBLE_RESULT_LENGTH"
                class="resultToggle"
                type="button"
                :aria-label="expandedResultKeys.has(resultKey(hit)) ? '收起日志行' : '展开日志行'"
                :title="expandedResultKeys.has(resultKey(hit)) ? '收起' : '展开'"
                @click.stop="toggleResultExpansion(hit)"
              >
                {{ expandedResultKeys.has(resultKey(hit)) ? '⌄' : '›' }}
              </button>
              <span v-else class="resultToggleSpacer" aria-hidden="true" />
              <span class="path">{{ hit.path }}</span>
              <span class="line">:{{ hit.lineNo }}</span>
              <code>
                <span v-for="(part, partIndex) in lineParts(hit)" :key="partIndex">
                  <mark v-if="part.match">{{ part.text }}</mark>
                  <span v-else>{{ part.text }}</span>
                </span>
              </code>
            </div>
          </div>
          <div v-else class="emptyState">
            <strong>没有结果</strong>
            <span>换个关键词，或调整筛选条件。</span>
          </div>
        </section>

      </div>
    </section>

    <footer v-if="appReady" class="appFooter">Copyright (c) 2026 future0923</footer>

    <div
      v-if="selectedHit"
      class="previewOverlay"
      :class="{ previewOverlayMaximized: previewMaximized }"
      role="dialog"
      aria-modal="true"
      @click="closePreviewOnOverlayClick"
    >
      <section class="preview previewExpanded" @click.stop>
        <PreviewPane
          :selected-hit="selectedHit"
          :expanded="previewMaximized"
          :preview-lines="previewLines"
          :preview-mode="previewMode"
          :expand-rows="expandRows"
          :around-loading="aroundLoading"
          :around="around"
          :highlight-regex="highlightRegex"
          @close="closePreview"
          @toggle-expanded="previewMaximized = !previewMaximized"
          @load-initial="loadAround(INITIAL_EXPAND_LINES, INITIAL_EXPAND_LINES, 'both')"
          @update-expand-rows="expandRows = $event"
          @expand-before="expandBefore"
          @expand-after="expandAfter"
          @expand-both="expandBoth"
        />
      </section>
    </div>

    <div
      v-if="tailFile"
      class="tailOverlay"
      :class="{ tailOverlayMaximized: tailMaximized }"
      role="dialog"
      aria-modal="true"
      aria-label="实时追踪"
      @click="closeTailOnOverlayClick"
    >
      <section class="tailPanel" @click.stop>
        <div class="tailHeader previewHeader">
          <div class="previewTitle">
            <span class="previewPath" :title="tailFile.path">{{ tailFile.path }}</span>
          </div>
          <div class="tailActions">
            <strong>{{ tailStatus }}</strong>
            <label class="tailToggle">
              <input v-model="tailAutoScroll" type="checkbox">
              <span class="tailToggleTrack" aria-hidden="true">
                <span class="tailToggleThumb" />
              </span>
              自动滚动
            </label>
            <button class="previewActionButton" type="button" @click="toggleTailPaused">
              {{ tailPaused ? '继续' : '暂停' }}
            </button>
            <button
              type="button"
              class="previewActionButton"
              :aria-label="tailMaximized ? '退出全屏' : '全屏'"
              :aria-pressed="tailMaximized"
              :title="tailMaximized ? '退出全屏' : '全屏'"
              @click="tailMaximized = !tailMaximized"
            >
              {{ tailMaximized ? '退出全屏' : '全屏' }}
            </button>
            <button class="previewClose" type="button" aria-label="关闭追踪" title="关闭追踪" @click="closeTail">x</button>
          </div>
        </div>
        <div v-if="tailError" class="tailError">{{ tailError }}</div>
        <div ref="tailViewportRef" class="logViewport tailViewport">
          <div
            v-for="line in tailLines"
            :key="line.lineNo + '-' + line.offset"
            class="logLine"
          >
            <span class="lineNumber">{{ line.lineNo }}</span>
            <code>{{ line.content }}</code>
          </div>
        </div>
      </section>
    </div>
  </main>
</template>
