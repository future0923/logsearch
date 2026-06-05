import { useEffect, useMemo, useRef, useState } from 'react'
import type { FormEvent, KeyboardEvent, ReactNode } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import './App.css'
import { shouldOpenResultFromClick } from './resultInteraction'

type ContextLine = {
  lineNo: number
  offset: number
  content: string
}

type SearchHit = {
  fileId: string
  path: string
  lineNo: number
  offset: number
  score: number
  kind: string
  content: string
  before: string[]
  after: string[]
  context: ContextLine[]
}

type SearchResponse = {
  hits: SearchHit[]
  total: number
  truncated: boolean
  hasNext: boolean
  nextCursor: string | null
  elapsedMs: number
}

type FileSource = {
  id: string
  path: string
  kind: string
  source: string
  directoryId?: string | null
  exists: boolean
}

type DirectorySource = {
  id: string
  path: string
  recursive: boolean
  exists: boolean
}

type StatusResponse = {
  files: number
  directories: number
  fileSources: FileSource[]
  configuredDirectories: DirectorySource[]
  discoveredFiles: FileSource[]
  discoveredFilesTruncated?: boolean
}

type AroundResponse = {
  path: string
  centerLineNo: number
  centerOffset: number
  lines: ContextLine[]
  hasBefore: boolean
  hasAfter: boolean
}

type TailLine = {
  lineNo: number
  offset: number
  content: string
}

type TailEventPayload = {
  path: string
  offset: number
  nextLineNo: number
  lines: TailLine[]
}

type PreviewMode = 'search' | 'around'
type LoadDirection = 'before' | 'after' | 'both'

const API_BASE = import.meta.env.VITE_API_BASE ?? ''
const CONTEXT_OPTIONS = [0, 2, 5, 10, 20]
const EXPAND_OPTIONS = [20, 50, 100, 200]
const INITIAL_EXPAND_LINES = 50
const SEARCH_PAGE_SIZE = 20
const COLLAPSIBLE_RESULT_LENGTH = 180
const TAIL_LINE_OPTIONS = [10, 50, 100, 200, 500, 1000]
const MAX_TAIL_LINES = 2000

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function buildHighlightRegex(query: string, regex: boolean, caseInsensitive: boolean, wholeWord: boolean) {
  if (!query.trim()) return null

  const booleanTerms = regex ? [] : query
    .split(/\s+|(?=[()])|(?<=[()])/g)
    .map((part) => part.trim())
    .filter((part) => part && part !== 'AND' && part !== 'OR' && part !== '(' && part !== ')')
  const source = regex
    ? query
    : booleanTerms.length > 1
      ? booleanTerms.map(escapeRegExp).join('|')
      : escapeRegExp(query)
  const bounded = wholeWord ? `\\b(?:${source})\\b` : source

  try {
    return new RegExp(bounded, `g${caseInsensitive ? 'i' : ''}`)
  } catch {
    return null
  }
}

function highlightText(content: string, pattern: RegExp | null): ReactNode {
  if (!pattern) return content

  const parts: ReactNode[] = []
  let lastIndex = 0
  pattern.lastIndex = 0

  for (const match of content.matchAll(pattern)) {
    const index = match.index ?? 0
    const value = match[0]
    if (!value) continue

    if (index > lastIndex) {
      parts.push(content.slice(lastIndex, index))
    }

    parts.push(<mark key={`${index}-${value}`}>{value}</mark>)
    lastIndex = index + value.length
  }

  if (lastIndex === 0) return content
  if (lastIndex < content.length) {
    parts.push(content.slice(lastIndex))
  }

  return parts
}

function renderQueryHighlight(query: string): ReactNode {
  const parts = query.split(/(\bAND\b|\bOR\b|[()])/g).filter(Boolean)
  const content = parts.map((part, index) => {
    if (part === 'AND' || part === 'OR') {
      return <span className="queryOperator" key={`${part}-${index}`}>{part}</span>
    }
    if (part === '(' || part === ')') {
      return <span className="queryParen" key={`${part}-${index}`}>{part}</span>
    }
    return <span key={`${part}-${index}`}>{part}</span>
  })
  return content.length ? content : '\u00a0'
}

function fallbackContext(hit: SearchHit): ContextLine[] {
  const before = hit.before.map((content, index) => ({
    lineNo: hit.lineNo - hit.before.length + index,
    offset: 0,
    content,
  }))
  const after = hit.after.map((content, index) => ({
    lineNo: hit.lineNo + index + 1,
    offset: 0,
    content,
  }))

  return [...before, { lineNo: hit.lineNo, offset: hit.offset, content: hit.content }, ...after]
}

function resultKey(hit: SearchHit) {
  return `${hit.fileId}-${hit.offset}`
}

function shortPath(path: string) {
  return path.split(/[\\/]/).pop() || path
}

function isCompressedKind(kind: string) {
  return kind === 'gzip' || kind === 'zstd' || kind === 'bzip2' || kind === 'xz'
}

function fileSearchText(file: FileSource) {
  return [
    file.id,
    file.kind,
    file.path,
    file.source,
    file.directoryId ?? '',
    file.exists ? 'ready' : 'missing',
    shortPath(file.path),
  ].join(' ').toLowerCase()
}

function filterFileSources(files: FileSource[], search: string) {
  const terms = search
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)

  if (!terms.length) return files
  return files.filter((file) => {
    const haystack = fileSearchText(file)
    return terms.every((term) => haystack.includes(term))
  })
}

function App() {
  const [query, setQuery] = useState('')
  const [regex, setRegex] = useState(false)
  const [caseInsensitive, setCaseInsensitive] = useState(true)
  const [wholeWord, setWholeWord] = useState(false)
  const [contextRows, setContextRows] = useState(2)
  const [results, setResults] = useState<SearchResponse | null>(null)
  const [selected, setSelected] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [aroundLoading, setAroundLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [previewMode, setPreviewMode] = useState<PreviewMode>('search')
  const [expandedPreview, setExpandedPreview] = useState(false)
  const [around, setAround] = useState<AroundResponse | null>(null)
  const [expandRows, setExpandRows] = useState(50)
  const [aroundBefore, setAroundBefore] = useState(INITIAL_EXPAND_LINES)
  const [aroundAfter, setAroundAfter] = useState(INITIAL_EXPAND_LINES)
  const [fileSources, setFileSources] = useState<FileSource[]>([])
  const [configuredDirectories, setConfiguredDirectories] = useState<DirectorySource[]>([])
  const [discoveredFiles, setDiscoveredFiles] = useState<FileSource[]>([])
  const [discoveredFilesTruncated, setDiscoveredFilesTruncated] = useState(false)
  const [showWatchedFiles, setShowWatchedFiles] = useState(false)
  const [selectedFileIds, setSelectedFileIds] = useState<string[]>([])
  const [filePickerOpen, setFilePickerOpen] = useState(false)
  const [filePickerSearch, setFilePickerSearch] = useState('')
  const [expandedResultKeys, setExpandedResultKeys] = useState<Set<string>>(() => new Set())
  const [tailFile, setTailFile] = useState<FileSource | null>(null)
  const [tailInitialLines, setTailInitialLines] = useState(10)
  const [tailLines, setTailLines] = useState<TailLine[]>([])
  const [tailOffset, setTailOffset] = useState<number | null>(null)
  const [tailPaused, setTailPaused] = useState(false)
  const [tailAutoScroll, setTailAutoScroll] = useState(true)
  const [tailMaximized, setTailMaximized] = useState(false)
  const [tailError, setTailError] = useState<string | null>(null)
  const previewRef = useRef<HTMLDivElement | null>(null)
  const resultsRef = useRef<HTMLElement | null>(null)
  const filePickerRef = useRef<HTMLDivElement | null>(null)
  const filePickerInputRef = useRef<HTMLInputElement | null>(null)
  const tailViewportRef = useRef<HTMLDivElement | null>(null)
  const tailEventSourceRef = useRef<EventSource | null>(null)
  const tailOffsetRef = useRef<number | null>(null)
  const tailNextLineNoRef = useRef<number | null>(null)
  const activeTailInitialLinesRef = useRef(10)
  const resultPointerDownRef = useRef<{ x: number, y: number } | null>(null)
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual manages its own scroll measurement state.
  const tailVirtualizer = useVirtualizer({
    count: tailLines.length,
    getScrollElement: () => tailViewportRef.current,
    estimateSize: () => 22,
    overscan: 12,
  })
  const tailVirtualRows = tailVirtualizer.getVirtualItems()

  useEffect(() => {
    let alive = true

    async function loadStatus() {
      try {
        const response = await fetch(`${API_BASE}/api/status`)
        if (!response.ok) return
        const payload = (await response.json()) as StatusResponse
        if (!alive) return
        setFileSources(payload.fileSources ?? [])
        setConfiguredDirectories(payload.configuredDirectories ?? [])
        setDiscoveredFiles(payload.discoveredFiles ?? [])
        setDiscoveredFilesTruncated(Boolean(payload.discoveredFilesTruncated))
      } catch {
        if (alive) {
          setFileSources([])
          setConfiguredDirectories([])
          setDiscoveredFiles([])
          setDiscoveredFilesTruncated(false)
        }
      }
    }

    void loadStatus()
    return () => {
      alive = false
    }
  }, [])

  useEffect(() => {
    tailEventSourceRef.current?.close()
    tailEventSourceRef.current = null

    if (!tailFile || tailPaused) return

    const params = new URLSearchParams({
      fileId: tailFile.id,
      lines: String(activeTailInitialLinesRef.current),
    })
    if (tailOffsetRef.current !== null && tailNextLineNoRef.current !== null) {
      params.set('offset', String(tailOffsetRef.current))
      params.set('nextLineNo', String(tailNextLineNoRef.current))
    }

    const source = new EventSource(`${API_BASE}/api/tail?${params.toString()}`)
    tailEventSourceRef.current = source

    source.addEventListener('tail', (event) => {
      const payload = JSON.parse((event as MessageEvent).data) as TailEventPayload
      tailOffsetRef.current = payload.offset
      tailNextLineNoRef.current = payload.nextLineNo
      setTailOffset(payload.offset)
      if (payload.lines.length) {
        setTailLines((current) => [...current, ...payload.lines].slice(-MAX_TAIL_LINES))
      }
    })

    source.addEventListener('error', () => {
      setTailError('Tail connection interrupted')
    })

    return () => {
      source.close()
      if (tailEventSourceRef.current === source) {
        tailEventSourceRef.current = null
      }
    }
  }, [tailFile, tailPaused])

  useEffect(() => {
    if (!tailLines.length || tailPaused || !tailAutoScroll) return
    tailVirtualizer.scrollToIndex(tailLines.length - 1, { align: 'end' })
  }, [tailAutoScroll, tailLines.length, tailPaused, tailVirtualizer])

  useEffect(() => {
    if (!filePickerOpen) return
    filePickerInputRef.current?.focus()
  }, [filePickerOpen])

  useEffect(() => {
    if (!filePickerOpen) return

    function closeOnOutsideClick(event: MouseEvent) {
      if (!filePickerRef.current?.contains(event.target as Node)) {
        setFilePickerOpen(false)
      }
    }

    document.addEventListener('mousedown', closeOnOutsideClick)
    return () => document.removeEventListener('mousedown', closeOnOutsideClick)
  }, [filePickerOpen])

  const selectedHit = selected === null ? null : results?.hits[selected]
  const highlightRegex = useMemo(
    () => buildHighlightRegex(query, regex, caseInsensitive, wholeWord),
    [caseInsensitive, query, regex, wholeWord],
  )
  const previewLines = useMemo(() => {
    if (previewMode === 'around' && around) return around.lines
    if (!selectedHit) return []
    return selectedHit.context?.length ? selectedHit.context : fallbackContext(selectedHit)
  }, [around, previewMode, selectedHit])

  const status = useMemo(() => {
    if (loading) return 'Searching index'
    if (error) return 'Search failed'
    if (!results) return 'Ready'
    return `${results.total} loaded in ${results.elapsedMs} ms`
  }, [error, loading, results])

  const selectedFileId = selectedFileIds[0] ?? 'all'
  const selectedFile = fileSources.find((file) => file.id === selectedFileId) ?? null
  const filteredFileSources = useMemo(
    () => filterFileSources(fileSources, filePickerSearch),
    [filePickerSearch, fileSources],
  )

function selectFileScope(fileId: string) {
    setSelectedFileIds(fileId === 'all' ? [] : [fileId])
    setFilePickerOpen(false)
  }

  function startTail(file: FileSource) {
    activeTailInitialLinesRef.current = tailInitialLines
    setTailFile(file)
    setTailLines([])
    setTailOffset(null)
    tailOffsetRef.current = null
    tailNextLineNoRef.current = null
    setTailPaused(false)
    setTailAutoScroll(true)
    setTailMaximized(false)
    setTailError(null)
  }

  function toggleTailPaused() {
    setTailPaused((value) => {
      if (value) setTailError(null)
      return !value
    })
  }

  function closeTail() {
    tailEventSourceRef.current?.close()
    tailEventSourceRef.current = null
    setTailFile(null)
    setTailLines([])
    setTailOffset(null)
    tailOffsetRef.current = null
    tailNextLineNoRef.current = null
    setTailPaused(false)
    setTailAutoScroll(true)
    setTailMaximized(false)
    setTailError(null)
  }

  async function fetchSearchPage(cursor: string | null) {
    const response = await fetch(`${API_BASE}/api/search`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        query,
        regex,
        caseInsensitive,
        wholeWord,
        limit: SEARCH_PAGE_SIZE,
        cursor,
        fileIds: selectedFileIds,
        contextBefore: 0,
        contextAfter: 0,
      }),
    })

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }

    return (await response.json()) as SearchResponse
  }

  async function runSearch(event?: FormEvent) {
    event?.preventDefault()

    setLoading(true)
    setError(null)
    try {
      const payload = await fetchSearchPage(null)
      setResults(payload)
      setSelected(null)
      setPreviewMode('search')
      setAround(null)
      setExpandedResultKeys(new Set())
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unknown error')
    } finally {
      setLoading(false)
    }
  }

  async function loadMoreResults() {
    if (!results?.hasNext || !results.nextCursor || loadingMore || loading) return

    setLoadingMore(true)
    setError(null)
    try {
      const payload = await fetchSearchPage(results.nextCursor)
      setResults({
        ...payload,
        hits: [...results.hits, ...payload.hits],
        total: results.hits.length + payload.hits.length,
      })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unknown error')
    } finally {
      setLoadingMore(false)
    }
  }

  async function loadMoreResultsAndScroll() {
    const viewport = resultsRef.current
    const previousScrollHeight = viewport?.scrollHeight ?? 0
    await loadMoreResults()
    window.requestAnimationFrame(() => {
      const current = resultsRef.current
      if (!current) return
      const addedHeight = current.scrollHeight - previousScrollHeight
      current.scrollBy({ top: Math.max(addedHeight, current.clientHeight * 0.8), behavior: 'smooth' })
    })
  }

  async function loadAround(
    before = aroundBefore,
    after = aroundAfter,
    direction: LoadDirection = 'both',
  ) {
    if (!selectedHit || aroundLoading) return
    await loadAroundForHit(selectedHit, before, after, direction)
  }

  async function loadAroundForHit(
    hit: SearchHit,
    before: number,
    after: number,
    direction: LoadDirection,
  ) {
    const previousScrollHeight = previewRef.current?.scrollHeight ?? 0
    const previousScrollTop = previewRef.current?.scrollTop ?? 0
    setAroundLoading(true)
    setError(null)
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

      const payload = (await response.json()) as AroundResponse
      setAround(payload)
      setAroundBefore(before)
      setAroundAfter(after)
      setPreviewMode('around')
      window.requestAnimationFrame(() => {
        const viewport = previewRef.current
        if (!viewport) return
        if (direction === 'before') {
          viewport.scrollTop = viewport.scrollHeight - previousScrollHeight + previousScrollTop
        } else if (direction === 'both') {
          const match = viewport.querySelector('.matchLine')
          match?.scrollIntoView({ block: 'center' })
        }
      })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unknown error')
    } finally {
      setAroundLoading(false)
    }
  }

  async function scrollResults(direction: 'previous' | 'next') {
    const viewport = resultsRef.current
    if (!viewport) return

    const distance = Math.max(viewport.clientHeight * 0.82, 160)
    if (direction === 'previous') {
      viewport.scrollBy({ top: -distance, behavior: 'smooth' })
      return
    }

    const remaining = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight
    if (remaining <= 24 && results?.hasNext && !loadingMore && !loading) {
      await loadMoreResultsAndScroll()
      return
    }
    viewport.scrollBy({ top: distance, behavior: 'smooth' })
  }

  function resetPreviewState() {
    setPreviewMode('search')
    setAround(null)
    setAroundBefore(INITIAL_EXPAND_LINES)
    setAroundAfter(INITIAL_EXPAND_LINES)
  }

  function selectHit(index: number) {
    const hit = results?.hits[index]
    setSelected(index)
    resetPreviewState()
    if (hit && contextRows > 0) {
      void loadAroundForHit(hit, contextRows, contextRows, 'both')
    }
  }

  function toggleResultExpansion(hit: SearchHit) {
    const key = resultKey(hit)
    setExpandedResultKeys((current) => {
      const next = new Set(current)
      if (next.has(key)) {
        next.delete(key)
      } else {
        next.add(key)
      }
      return next
    })
  }

  function selectHitFromKeyboard(event: KeyboardEvent<HTMLDivElement>, index: number) {
    if (event.key !== 'Enter' && event.key !== ' ') return
    event.preventDefault()
    selectHit(index)
  }

  function hasSelectedText() {
    return Boolean(window.getSelection()?.toString())
  }

  function closePreview() {
    setSelected(null)
    setExpandedPreview(false)
    resetPreviewState()
  }

  function expandBefore() {
    void loadAround(aroundBefore + expandRows, aroundAfter, 'before')
  }

  function expandAfter() {
    void loadAround(aroundBefore, aroundAfter + expandRows, 'after')
  }

  function expandBoth() {
    void loadAround(aroundBefore + expandRows, aroundAfter + expandRows, 'both')
  }

  function renderPreview(expanded = false) {
    if (!selectedHit) {
      return (
        <div className="emptyState">
          <strong>No result selected</strong>
          <span>Run a search to inspect the matching line.</span>
        </div>
      )
    }

    return (
      <>
        <div className="previewHeader">
          <span>{selectedHit.path}</span>
          <div className="previewActions">
            <strong>line {selectedHit.lineNo}</strong>
            <button
              type="button"
              onClick={() => loadAround(INITIAL_EXPAND_LINES, INITIAL_EXPAND_LINES, 'both')}
              disabled={aroundLoading}
            >
              展开上下文
            </button>
            <button
              type="button"
              onClick={() => setExpandedPreview((current) => !current)}
            >
              {expanded ? '关闭' : '放大'}
            </button>
            <button
              className="previewClose"
              type="button"
              aria-label="关闭预览"
              title="关闭预览"
              onClick={closePreview}
            >
              ×
            </button>
          </div>
        </div>
        <div className="previewToolbar">
          <span>
            {previewMode === 'around'
              ? `已显示前 ${aroundBefore} / 后 ${aroundAfter} 行`
              : `当前显示前后各 ${contextRows} 行`}
          </span>
          <label>
            每次加载
            <select
              value={expandRows}
              onChange={(event) => setExpandRows(Number(event.target.value))}
            >
              {EXPAND_OPTIONS.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
            行
          </label>
        </div>
        <div className="logViewport" ref={previewRef}>
          {previewMode === 'around' ? (
            <div className="loadMoreRow top">
              <button
                type="button"
                onClick={expandBefore}
                disabled={aroundLoading || !around?.hasBefore}
              >
                向上加载 {expandRows} 行
              </button>
            </div>
          ) : null}
          {previewLines.map((line) => {
            const isMatchLine = line.lineNo === selectedHit.lineNo
            return (
              <div className={`logLine ${isMatchLine ? 'matchLine' : ''}`} key={line.lineNo}>
                <span className="lineNumber">{line.lineNo}</span>
                <code>{highlightText(line.content, highlightRegex)}</code>
              </div>
            )
          })}
          {previewMode === 'around' ? (
            <div className="loadMoreRow bottom">
              <button
                type="button"
                onClick={expandAfter}
                disabled={aroundLoading || !around?.hasAfter}
              >
                向下加载 {expandRows} 行
              </button>
            </div>
          ) : null}
        </div>
        {previewMode === 'around' ? (
          <div className="previewFooter">
            <button type="button" onClick={expandBoth} disabled={aroundLoading}>
              上下各加载 {expandRows} 行
            </button>
          </div>
        ) : null}
      </>
    )
  }

  return (
    <main className="shell">
      <section className="workspace">
        <div className="topBar">
          <header className="appHeader">
            <div className="brandMark">LOG</div>
            <div>
              <h1>Log Search</h1>
              <p>Indexed log search</p>
            </div>
          </header>

          <form className="searchBar" onSubmit={runSearch}>
            <div className="searchInputWrap">
              <span className="prompt">$</span>
              <div className="queryEditor">
                <div className="queryInputHighlight" aria-hidden="true">
                  {renderQueryHighlight(query)}
                </div>
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search logs, ids, classes, paths, or regex"
                  autoFocus
                />
              </div>
            </div>
            <button type="submit" disabled={loading}>
              Search
            </button>
          </form>
        </div>

        <div className="controls" aria-label="Search options">
          <label>
            <input
              type="checkbox"
              checked={caseInsensitive}
              onChange={(event) => setCaseInsensitive(event.target.checked)}
            />
            Ignore case
          </label>
          <label>
            <input
              type="checkbox"
              checked={wholeWord}
              onChange={(event) => setWholeWord(event.target.checked)}
            />
            Whole word
          </label>
          <label>
            <input
              type="checkbox"
              checked={regex}
              onChange={(event) => setRegex(event.target.checked)}
            />
            Regex
          </label>
          <label className="selectControl">
            Context
            <select
              value={contextRows}
              onChange={(event) => setContextRows(Number(event.target.value))}
            >
              {CONTEXT_OPTIONS.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          {fileSources.length ? (
            <div className="filePickerField" ref={filePickerRef}>
              <span className="filePickerLabel">File</span>
              <button
                type="button"
                className={`filePickerTrigger ${filePickerOpen ? 'open' : ''}`}
                aria-haspopup="listbox"
                aria-expanded={filePickerOpen}
                onClick={() => {
                  setFilePickerOpen((open) => !open)
                  setFilePickerSearch('')
                }}
              >
                <span className="filePickerTriggerText">
                  {selectedFile ? `${selectedFile.id} · ${selectedFile.kind} · ${shortPath(selectedFile.path)}` : 'All files'}
                </span>
                <span className="filePickerChevron" aria-hidden="true">⌄</span>
              </button>
              {filePickerOpen ? (
                <div className="filePickerMenu">
                  <input
                    ref={filePickerInputRef}
                    className="filePickerSearch"
                    value={filePickerSearch}
                    onChange={(event) => setFilePickerSearch(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Escape') {
                        setFilePickerOpen(false)
                      }
                    }}
                    placeholder="Search file id, path, kind..."
                  />
                  <div className="filePickerList" role="listbox" aria-label="Log files">
                    <button
                      type="button"
                      className={`filePickerOption ${selectedFileId === 'all' ? 'selected' : ''}`}
                      role="option"
                      aria-selected={selectedFileId === 'all'}
                      onClick={() => selectFileScope('all')}
                    >
                      <span className="filePickerOptionMain">All files</span>
                      <span className="filePickerOptionMeta">{fileSources.length} sources</span>
                    </button>
                    {filteredFileSources.length ? filteredFileSources.map((file) => (
                      <button
                        type="button"
                        className={`filePickerOption ${selectedFileId === file.id ? 'selected' : ''}`}
                        role="option"
                        aria-selected={selectedFileId === file.id}
                        key={file.id}
                        onClick={() => selectFileScope(file.id)}
                      >
                        <span className="filePickerOptionMain">
                          <span className={`kindPill ${isCompressedKind(file.kind) ? 'compressed' : 'hot'}`}>{file.kind}</span>
                          <strong>{file.id}</strong>
                          <span>{shortPath(file.path)}</span>
                        </span>
                        <span className="filePickerOptionMeta" title={file.path}>
                          {file.source === 'directory' ? file.directoryId : 'file'} · {file.exists ? 'ready' : 'missing'} · {file.path}
                        </span>
                      </button>
                    )) : (
                      <div className="filePickerEmpty">No files match "{filePickerSearch}"</div>
                    )}
                  </div>
                </div>
              ) : null}
            </div>
          ) : null}
        </div>

        <div className="statusLine">
          <span>{status}</span>
          <span>{configuredDirectories.length} dirs</span>
          <span>{discoveredFiles.length}{discoveredFilesTruncated ? '+' : ''} watched files</span>
          {results?.hasNext ? <span>More available</span> : null}
          {selectedHit && selected !== null ? (
            <span>{selected + 1} / {results?.hits.length}</span>
          ) : null}
          {previewMode === 'around' ? <span>{previewLines.length} preview lines</span> : null}
          {aroundLoading ? <span>Loading lines</span> : null}
          <button type="button" className="linkButton" onClick={() => setShowWatchedFiles((value) => !value)}>
            {showWatchedFiles ? 'Hide watched' : 'Show watched'}
          </button>
        </div>

        {showWatchedFiles ? (
          <section className="watchedPanel" aria-label="Watched files">
            <div className="watchedSummary">
              <span>{fileSources.filter((file) => file.kind === 'hot').length} hot</span>
              <span>{fileSources.filter((file) => isCompressedKind(file.kind)).length} compressed</span>
              <span>{configuredDirectories.filter((directory) => directory.exists).length} active dirs</span>
              <label className="tailLineSelect">
                Initial lines
                <select
                  value={tailInitialLines}
                  onChange={(event) => setTailInitialLines(Number(event.target.value))}
                >
                  {TAIL_LINE_OPTIONS.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="watchedTable">
              {fileSources.length ? fileSources.map((file) => (
                <div className="watchedRow" key={file.id}>
                  <span className={file.exists ? 'stateOk' : 'stateMissing'}>{file.exists ? 'ready' : 'missing'}</span>
                  <span className={`kindPill ${isCompressedKind(file.kind) ? 'compressed' : 'hot'}`}>{file.kind}</span>
                  <span className="watchedName">{file.id}</span>
                  <span className="watchedSource">{file.source === 'directory' ? file.directoryId : 'file'}</span>
                  <span className="watchedPath" title={file.path}>{file.path}</span>
                  <button
                    className="tailButton"
                    type="button"
                    onClick={() => startTail(file)}
                    disabled={!file.exists || file.kind !== 'hot'}
                  >
                    Tail
                  </button>
                </div>
              )) : (
                <div className="watchedEmpty">No watched files</div>
              )}
            </div>
          </section>
        ) : null}

        {error ? <div className="errorPanel">{error}</div> : null}

        {results ? (
          <div className={`resultLayout ${selectedHit ? 'withPreview' : 'resultsOnly'}`}>
            <section className="results" ref={resultsRef}>
              {results?.hits.length ? (
                <div className="resultsHeader">
                  <span>{results.hits.length} loaded</span>
                  <div className="resultNav">
                    <button type="button" onClick={() => void scrollResults('previous')}>
                      Previous
                    </button>
                    <button
                      type="button"
                      onClick={() => void scrollResults('next')}
                      disabled={loadingMore}
                    >
                      {loadingMore ? 'Loading' : 'Next'}
                    </button>
                  </div>
                </div>
              ) : null}
              {results?.hits.length ? (
                results.hits.map((hit, index) => {
                  const key = resultKey(hit)
                  const canCollapse = hit.content.length > COLLAPSIBLE_RESULT_LENGTH
                  const isExpanded = expandedResultKeys.has(key)
                  return (
                    <div
                      className={`resultRow ${index === selected ? 'active' : ''} ${canCollapse ? 'collapsible' : ''} ${canCollapse && !isExpanded ? 'collapsed' : ''}`}
                      key={key}
                      role="button"
                      tabIndex={0}
                      onPointerDown={(event) => {
                        resultPointerDownRef.current = { x: event.clientX, y: event.clientY }
                      }}
                      onClick={(event) => {
                        if (!shouldOpenResultFromClick({
                          hasTextSelection: hasSelectedText(),
                          pointerDown: resultPointerDownRef.current,
                          pointerUp: { x: event.clientX, y: event.clientY },
                        })) {
                          return
                        }
                        selectHit(index)
                      }}
                      onKeyDown={(event) => selectHitFromKeyboard(event, index)}
                    >
                      {canCollapse ? (
                        <button
                          className="resultToggle"
                          type="button"
                          aria-label={isExpanded ? '收起日志行' : '展开日志行'}
                          title={isExpanded ? '收起' : '展开'}
                          onClick={(event) => {
                            event.stopPropagation()
                            toggleResultExpansion(hit)
                          }}
                        >
                          {isExpanded ? '⌄' : '›'}
                        </button>
                      ) : (
                        <span className="resultToggleSpacer" aria-hidden="true" />
                      )}
                      <span className="path">{hit.path}</span>
                      <span className="line">:{hit.lineNo}</span>
                      <code>{highlightText(hit.content, highlightRegex)}</code>
                    </div>
                  )
                })
              ) : (
                <div className="emptyState">
                  <strong>No results</strong>
                  <span>Try another keyword or adjust the filters.</span>
                </div>
              )}
            </section>

            {selectedHit ? (
              <section className="preview">
                {renderPreview()}
              </section>
            ) : null}
          </div>
        ) : null}
      </section>
      <footer className="appFooter">
        Copyright (c) 2026 future0923
      </footer>
      {expandedPreview ? (
        <div className="previewOverlay" role="dialog" aria-modal="true">
          <section className="preview previewExpanded">{renderPreview(true)}</section>
        </div>
      ) : null}
      {tailFile ? (
        <div
          className={`tailOverlay${tailMaximized ? ' tailOverlayMaximized' : ''}`}
          role="dialog"
          aria-modal="true"
          aria-label="Live tail"
        >
          <section className="tailPanel">
            <div className="tailHeader">
              <div>
                <strong>{tailFile.id}</strong>
                <span title={tailFile.path}>{tailFile.path}</span>
              </div>
              <div className="tailActions">
                {tailOffset !== null ? <span>{tailLines.length} lines</span> : <span>Connecting</span>}
                <label className="tailToggle">
                  <input
                    type="checkbox"
                    checked={tailAutoScroll}
                    onChange={(event) => setTailAutoScroll(event.target.checked)}
                  />
                  <span className="tailToggleTrack" aria-hidden="true">
                    <span className="tailToggleThumb" />
                  </span>
                  Auto scroll
                </label>
                <button type="button" onClick={toggleTailPaused}>
                  {tailPaused ? 'Resume' : 'Pause'}
                </button>
                <button
                  type="button"
                  className={`tailIconButton${tailMaximized ? ' tailRestoreButton' : ''}`}
                  aria-label={tailMaximized ? 'Restore tail panel' : 'Maximize tail panel'}
                  aria-pressed={tailMaximized}
                  title={tailMaximized ? 'Restore' : 'Maximize'}
                  onClick={() => setTailMaximized((value) => !value)}
                >
                  <span aria-hidden="true" />
                </button>
                <button type="button" onClick={closeTail}>
                  Close
                </button>
              </div>
            </div>
            {tailError ? <div className="tailError">{tailError}</div> : null}
            <div className="logViewport tailViewport" ref={tailViewportRef}>
              <div
                className="tailVirtualCanvas"
                style={{ height: `${tailVirtualizer.getTotalSize()}px` }}
              >
                {tailVirtualRows.map((virtualRow) => {
                  const line = tailLines[virtualRow.index]
                  return (
                    <div
                      className="logLine tailVirtualLine"
                      data-index={virtualRow.index}
                      key={`${line.lineNo}-${line.offset}`}
                      ref={tailVirtualizer.measureElement}
                      style={{ transform: `translateY(${virtualRow.start}px)` }}
                    >
                      <span className="lineNumber">{line.lineNo}</span>
                      <code>{line.content}</code>
                    </div>
                  )
                })}
                </div>
            </div>
          </section>
        </div>
      ) : null}
    </main>
  )
}

export default App
