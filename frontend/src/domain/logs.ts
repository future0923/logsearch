export type ContextLine = {
  lineNo: number
  offset: number
  content: string
}

export type SearchHit = {
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

export type SearchResponse = {
  hits: SearchHit[]
  total: number
  truncated: boolean
  hasNext: boolean
  nextCursor: string | null
  elapsedMs: number
}

export type FileSource = {
  id: string
  path: string
  kind: string
  source: string
  directoryId?: string | null
  exists: boolean
}

export type DirectorySource = {
  id: string
  path: string
  recursive: boolean
  exists: boolean
}

export type StatusResponse = {
  files: number
  directories: number
  fileSources: FileSource[]
  configuredDirectories: DirectorySource[]
  discoveredFiles: FileSource[]
  discoveredFilesTruncated?: boolean
}

export type AroundResponse = {
  path: string
  centerLineNo: number
  centerOffset: number
  lines: ContextLine[]
  hasBefore: boolean
  hasAfter: boolean
}

export type TailLine = {
  lineNo: number
  offset: number
  content: string
}

export type TailEventPayload = {
  path: string
  offset: number
  nextLineNo: number
  lines: TailLine[]
}

export type PreviewMode = 'search' | 'around'
export type LoadDirection = 'before' | 'after' | 'both'

export function shortPath(path: string) {
  return path.split(/[\\/]/).pop() || path
}

export function filePickerSelectionLabel(file: FileSource): string
export function filePickerSelectionLabel(files: FileSource[], selectedIds: string[]): string
export function filePickerSelectionLabel(fileOrFiles: FileSource | FileSource[], selectedIds?: string[]) {
  if (!Array.isArray(fileOrFiles)) return `${shortPath(fileOrFiles.path)} · ${fileOrFiles.kind}`
  if (!selectedIds?.length) return '全部文件'
  if (selectedIds.length === 1) {
    const selectedFile = fileOrFiles.find((file) => file.id === selectedIds[0])
    return selectedFile ? filePickerSelectionLabel(selectedFile) : '已选 1 个文件'
  }
  return `已选 ${selectedIds.length} 个文件`
}

export function filePickerSelectionTitle(files: FileSource[], selectedIds: string[]) {
  if (!selectedIds.length) return undefined
  const byId = new Map(files.map((file) => [file.id, file]))
  return selectedIds.map((id) => {
    const file = byId.get(id)
    return file ? `${file.id} · ${file.path}` : id
  }).join('\n')
}

export function toggleFileSelection(selectedIds: string[], fileId: string) {
  if (selectedIds.includes(fileId)) return selectedIds.filter((id) => id !== fileId)
  return [...selectedIds, fileId]
}

export function toggleAllFileSelection(files: FileSource[], selectedIds: string[]) {
  const fileIds = files.map((file) => file.id)
  const selectedIdSet = new Set(selectedIds)
  const allSelected = fileIds.length > 0 && fileIds.every((id) => selectedIdSet.has(id))
  return allSelected ? [] : fileIds
}

export function selectAllVisibleFileSources(files: FileSource[], selectedIds: string[], search: string) {
  return toggleAllFileSelection(filterFileSources(files, search), selectedIds)
}

export function isCompressedKind(kind: string) {
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

export function filterFileSources(files: FileSource[], search: string) {
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

export function fallbackContext(hit: SearchHit): ContextLine[] {
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

export function resultKey(hit: SearchHit) {
  return `${hit.fileId}-${hit.offset}`
}
