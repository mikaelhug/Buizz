export function appendPoint<P extends { at: string }>(points: P[], point: P): P[] {
  const last = points.at(-1);
  if (!last) {
    return [point];
  }
  const order = Date.parse(point.at) - Date.parse(last.at);
  if (order < 0) {
    return points;
  }
  return order === 0 ? [...points.slice(0, -1), point] : [...points, point];
}
