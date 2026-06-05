type PointerPoint = {
  x: number
  y: number
}

type ResultClickIntent = {
  hasTextSelection: boolean
  pointerDown: PointerPoint | null
  pointerUp: PointerPoint
}

const DRAG_CLICK_DISTANCE_PX = 6

export function shouldOpenResultFromClick({
  hasTextSelection,
  pointerDown,
  pointerUp,
}: ResultClickIntent) {
  if (hasTextSelection) return false
  if (!pointerDown) return true

  const deltaX = pointerUp.x - pointerDown.x
  const deltaY = pointerUp.y - pointerDown.y
  return Math.hypot(deltaX, deltaY) <= DRAG_CLICK_DISTANCE_PX
}
