export type HighlightPart = {
  text: string
  match: boolean
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

export function buildHighlightRegex(query: string, regex: boolean, caseInsensitive: boolean, wholeWord: boolean) {
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

export function highlightedParts(content: string, pattern: RegExp | null): HighlightPart[] {
  if (!pattern) return [{ text: content, match: false }]

  const parts: HighlightPart[] = []
  let lastIndex = 0
  pattern.lastIndex = 0

  for (const match of content.matchAll(pattern)) {
    const index = match.index ?? 0
    const value = match[0]
    if (!value) continue

    if (index > lastIndex) {
      parts.push({ text: content.slice(lastIndex, index), match: false })
    }
    parts.push({ text: value, match: true })
    lastIndex = index + value.length
  }

  if (!parts.length) return [{ text: content, match: false }]
  if (lastIndex < content.length) {
    parts.push({ text: content.slice(lastIndex), match: false })
  }
  return parts
}

export function queryTokens(query: string) {
  const tokens = query.split(/(\bAND\b|\bOR\b|[()])/g).filter(Boolean)
  return tokens.length ? tokens : ['\u00a0']
}
