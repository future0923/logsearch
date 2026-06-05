export function isAuthenticationChallenge(status: number) {
  return status === 401 || status === 403
}

export function authenticationChallengeUrl(apiBase: string, currentHref: string, nonce: string) {
  const base = apiBase || currentHref
  const url = new URL('/api/status', base)
  url.searchParams.set('authChallenge', nonce)
  return url.toString()
}
