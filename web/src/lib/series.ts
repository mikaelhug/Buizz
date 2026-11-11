export function appendPoints<P extends { at: string }>(points: P[], added: P[]): P[] {
  const next = points.slice();
  for (const point of added) {
    const last = next.at(-1);
    const order = last ? Date.parse(point.at) - Date.parse(last.at) : 1;
    if (order === 0) {
      next[next.length - 1] = point;
    } else if (order > 0) {
      next.push(point);
    }
  }
  return next;
}
