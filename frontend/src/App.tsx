import { useEffect, useMemo, useRef, useState } from 'react'
import type { FormEvent, ReactNode } from 'react'
import './App.css'

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
}

type StatusResponse = {
  files: number
  fileSources: FileSource[]
}

type AroundResponse = {
  path: string
  centerLineNo: number
  centerOffset: number
  lines: ContextLine[]
  hasBefore: boolean
  hasAfter: boolean
}

type PreviewMode = 'search' | 'around'
type LoadDirection = 'before' | 'after' | 'both'

const API_BASE = import.meta.env.VITE_API_BASE ?? ''
const CONTEXT_OPTIONS = [0, 2, 5, 10, 20]
const EXPAND_OPTIONS = [20, 50, 100, 200]
const INITIAL_EXPAND_LINES = 50
const SEARCH_PAGE_SIZE = 20

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

function App() {
  const [query, setQuery] = useState('timeout')
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
  const [selectedFileIds, setSelectedFileIds] = useState<string[]>([])
  const previewRef = useRef<HTMLDivElement | null>(null)
  const resultsRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    let alive = true

    async function loadStatus() {
      try {
        const response = await fetch(`${API_BASE}/api/status`)
        if (!response.ok) return
        const payload = (await response.json()) as StatusResponse
        if (!alive) return
        setFileSources(payload.fileSources ?? [])
      } catch {
        if (alive) setFileSources([])
      }
    }

    void loadStatus()
    return () => {
      alive = false
    }
  }, [])

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

  function shortPath(path: string) {
    return path.split(/[\\/]/).pop() || path
  }

  function selectFileScope(fileId: string) {
    setSelectedFileIds(fileId === 'all' ? [] : [fileId])
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
    if (!query.trim()) return

    setLoading(true)
    setError(null)
    try {
      const payload = await fetchSearchPage(null)
      setResults(payload)
      setSelected(null)
      setPreviewMode('search')
      setAround(null)
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
          compressed: hit.kind === 'gzip',
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
            <button type="submit" disabled={loading || !query.trim()}>
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
            <label className="selectControl fileSelect">
              File
              <select
                value={selectedFileIds[0] ?? 'all'}
                onChange={(event) => selectFileScope(event.target.value)}
              >
                <option value="all">All files</option>
                {fileSources.map((file) => (
                  <option key={file.id} value={file.id}>
                    {file.id} · {shortPath(file.path)}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
        </div>

        <div className="statusLine">
          <span>{status}</span>
          {results?.hasNext ? <span>More available</span> : null}
          {selectedHit && selected !== null ? (
            <span>{selected + 1} / {results?.hits.length}</span>
          ) : null}
          {previewMode === 'around' ? <span>{previewLines.length} preview lines</span> : null}
          {aroundLoading ? <span>Loading lines</span> : null}
        </div>

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
                results.hits.map((hit, index) => (
                  <button
                    className={`resultRow ${index === selected ? 'active' : ''}`}
                    key={`${hit.fileId}-${hit.offset}`}
                    type="button"
                    onClick={() => selectHit(index)}
                  >
                    <span className="path">{hit.path}</span>
                    <span className="line">:{hit.lineNo}</span>
                    <code>{highlightText(hit.content, highlightRegex)}</code>
                  </button>
                ))
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
      {expandedPreview ? (
        <div className="previewOverlay" role="dialog" aria-modal="true">
          <section className="preview previewExpanded">{renderPreview(true)}</section>
        </div>
      ) : null}
    </main>
  )
}

export default App
