import test from 'node:test'
import assert from 'node:assert/strict'

import { authenticationChallengeUrl, isAuthenticationChallenge } from '../src/domain/auth.ts'

test('treats unauthorized status as an authentication challenge', () => {
  assert.equal(isAuthenticationChallenge(401), true)
})

test('does not treat ordinary backend errors as an authentication challenge', () => {
  assert.equal(isAuthenticationChallenge(500), false)
})

test('builds same-origin authentication challenge url', () => {
  assert.equal(
    authenticationChallengeUrl('', 'http://localhost:5173', '1'),
    'http://localhost:5173/api/status?authChallenge=1',
  )
})

test('builds remote authentication challenge url', () => {
  assert.equal(
    authenticationChallengeUrl('http://127.0.0.1:12457', 'http://localhost:5173', '2'),
    'http://127.0.0.1:12457/api/status?authChallenge=2',
  )
})
