// pure drag-reorder math for the chip list rows: the drop index
// from a pointer offset, how far the crossed rows slide, and the
// reordered list. DOM-free so bun can test it directly.

// the dragged row's destination: half a row past a neighbor
// counts as crossing it (symmetrically, in both directions)
export function dropIndex(from: number, dy: number, rowHeight: number, count: number): number {
  if (rowHeight <= 0 || count <= 0) return from;
  const steps = Math.trunc(dy / rowHeight + (dy >= 0 ? 0.5 : -0.5));
  const target = from + steps;
  return Math.min(count - 1, Math.max(0, target));
}

// how far a non-dragged row slides while another is dragged
// over it: the crossed range makes room in the drag direction
export function shiftOffset(index: number, from: number, to: number, rowHeight: number): number {
  if (from < to && index > from && index <= to) return -rowHeight;
  if (to < from && index >= to && index < from) return rowHeight;
  return 0;
}

// the reordered list; a no-op or out-of-range move returns the
// input untouched so callers can skip the write
export function moveItem<T>(list: readonly T[], from: number, to: number): readonly T[] {
  if (from === to || from < 0 || to < 0 || from >= list.length || to >= list.length) {
    return list;
  }
  const next = [...list];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
}
