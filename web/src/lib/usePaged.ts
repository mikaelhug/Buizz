import { type DependencyList, useState } from "react";
import { type Resource, useResource } from "./useResource";

type Page<T> = { items: T[]; next_after?: number | null };
type Loaded<T> = { items: T[]; next: number | null };

export type Paged<T> = Resource<Loaded<T>> & { loadMore: (() => void) | undefined };

export const PAGE_SIZE = 200;

export function usePaged<T>(
  fetchPage: (after: number | undefined) => Promise<Page<T>>,
  deps: DependencyList,
): Paged<T> {
  const [pages, setPages] = useState(1);
  const resource = useResource(async () => {
    const items: T[] = [];
    let after: number | undefined;
    for (let page = 0; page < pages; page++) {
      const loaded = await fetchPage(after);
      items.push(...loaded.items);
      if (loaded.next_after === null || loaded.next_after === undefined) {
        return { items, next: null };
      }
      after = loaded.next_after;
    }
    return { items, next: after ?? null };
  }, [...deps, pages]);
  const canLoadMore = resource.data !== undefined && resource.data.next !== null;
  return { ...resource, loadMore: canLoadMore ? () => setPages((count) => count + 1) : undefined };
}
