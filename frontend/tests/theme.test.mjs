import test from 'node:test'
import assert from 'node:assert/strict'

import { getStoredTheme, toggleTheme } from '../src/domain/theme.ts'

test('toggles between light and dark themes', () => {
  assert.equal(toggleTheme('light'), 'dark')
  assert.equal(toggleTheme('dark'), 'light')
})

test('ignores invalid stored theme values', () => {
  const storage = {
    getItem() {
      return 'blue'
    },
  }

  assert.equal(getStoredTheme(storage), null)
})

test('reads a valid stored theme value', () => {
  const storage = {
    getItem() {
      return 'dark'
    },
  }

  assert.equal(getStoredTheme(storage), 'dark')
})
