export type ThemeMode = 'light' | 'dark'

export type ThemeStorage = {
  getItem(key: string): string | null
}

export const THEME_STORAGE_KEY = 'log-search-theme'

export function isThemeMode(value: string | null): value is ThemeMode {
  return value === 'light' || value === 'dark'
}

export function getStoredTheme(storage: ThemeStorage) {
  const value = storage.getItem(THEME_STORAGE_KEY)
  return isThemeMode(value) ? value : null
}

export function toggleTheme(theme: ThemeMode): ThemeMode {
  return theme === 'light' ? 'dark' : 'light'
}
