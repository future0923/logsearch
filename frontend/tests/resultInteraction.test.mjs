import test from 'node:test'
import assert from 'node:assert/strict'

import { shouldOpenResultFromClick } from '../src/resultInteraction.ts'

test('opens result for a plain click with no text selection', () => {
  assert.equal(
    shouldOpenResultFromClick({
      hasTextSelection: false,
      pointerDown: { x: 20, y: 30 },
      pointerUp: { x: 21, y: 31 },
    }),
    true,
  )
})

test('ignores result click when text is selected', () => {
  assert.equal(
    shouldOpenResultFromClick({
      hasTextSelection: true,
      pointerDown: { x: 20, y: 30 },
      pointerUp: { x: 21, y: 31 },
    }),
    false,
  )
})

test('ignores result click after dragging across row text', () => {
  assert.equal(
    shouldOpenResultFromClick({
      hasTextSelection: false,
      pointerDown: { x: 20, y: 30 },
      pointerUp: { x: 44, y: 30 },
    }),
    false,
  )
})
