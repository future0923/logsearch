import test from 'node:test'
import assert from 'node:assert/strict'

import { buildHighlightRegex, highlightedParts } from '../src/domain/highlight.ts'
import {
  filePickerSelectionLabel,
  filePickerSelectionTitle,
  filterFileSources,
  isCompressedKind,
  selectAllVisibleFileSources,
  shortPath,
  toggleAllFileSelection,
  toggleFileSelection,
} from '../src/domain/logs.ts'

const files = [
  {
    id: 'app-hot',
    path: '/var/log/app.log',
    kind: 'hot',
    source: 'file',
    exists: true,
  },
  {
    id: 'archive-gz',
    path: '/var/log/app.log.1.gz',
    kind: 'gzip',
    source: 'directory',
    directoryId: 'main',
    exists: true,
  },
]

test('detects compressed log source kinds', () => {
  assert.equal(isCompressedKind('gzip'), true)
  assert.equal(isCompressedKind('zstd'), true)
  assert.equal(isCompressedKind('bzip2'), true)
  assert.equal(isCompressedKind('xz'), true)
  assert.equal(isCompressedKind('hot'), false)
})

test('returns the last path segment for unix and windows paths', () => {
  assert.equal(shortPath('/var/log/app.log'), 'app.log')
  assert.equal(shortPath('C:\\logs\\worker.log'), 'worker.log')
})

test('filters file sources by multiple file terms', () => {
  assert.deepEqual(
    filterFileSources(files, 'archive gzip').map((file) => file.id),
    ['archive-gz'],
  )
})

test('summarizes selected file source without repeating directory generated ids', () => {
  assert.equal(
    filePickerSelectionLabel({
      id: 'directory-demo:app.log.2.zst',
      path: '/var/log/directory-demo/app.log.2.zst',
      kind: 'zstd',
      source: 'directory',
      directoryId: 'directory-demo',
      exists: true,
    }),
    'app.log.2.zst · zstd',
  )
})

test('summarizes multiple selected file sources by count', () => {
  assert.equal(filePickerSelectionLabel(files, ['app-hot', 'archive-gz']), '已选 2 个文件')
})

test('builds a readable title for multiple selected file sources', () => {
  assert.equal(
    filePickerSelectionTitle(files, ['app-hot', 'archive-gz']),
    'app-hot · /var/log/app.log\narchive-gz · /var/log/app.log.1.gz',
  )
})

test('toggles file selection without duplicating ids', () => {
  assert.deepEqual(toggleFileSelection([], 'app-hot'), ['app-hot'])
  assert.deepEqual(toggleFileSelection(['app-hot'], 'archive-gz'), ['app-hot', 'archive-gz'])
  assert.deepEqual(toggleFileSelection(['app-hot', 'archive-gz'], 'app-hot'), ['archive-gz'])
  assert.deepEqual(toggleFileSelection(['app-hot'], 'app-hot'), [])
})

test('toggles all file selections by explicit selected ids', () => {
  assert.deepEqual(toggleAllFileSelection(files, []), ['app-hot', 'archive-gz'])
  assert.deepEqual(toggleAllFileSelection(files, ['app-hot']), ['app-hot', 'archive-gz'])
  assert.deepEqual(toggleAllFileSelection(files, ['archive-gz', 'app-hot']), [])
})

test('toggles all file selections within filtered sources', () => {
  const allFiles = [
    ...files,
    {
      id: 'worker-hot',
      path: '/var/log/worker.log',
      kind: 'hot',
      source: 'file',
      exists: true,
    },
  ]

  assert.deepEqual(selectAllVisibleFileSources(allFiles, [], 'archive'), ['archive-gz'])
})

test('builds case-sensitive highlight regex when ignore case is disabled', () => {
  const pattern = buildHighlightRegex('Error', false, false, false)
  assert.deepEqual(
    highlightedParts('Error error', pattern),
    [
      { text: 'Error', match: true },
      { text: ' error', match: false },
    ],
  )
})
