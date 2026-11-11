import { type DependencyList, useRef } from "react";
import { type Resource, useResource } from "./useResource";

type Page<T> = { items: T[]; next_after?: number | null };
type Loaded<T> = { items: T[]; next: number | null };

export type Paged<T> = Resource<Loaded<T>> & { loadMore: (() => void) | undefined };

export const PAGE_SIZE = 200;

export function usePaged<T>(
  fetchPage: (after: number | undefined) => Promise<Page<T>>,
  deps: DependencyList,
): Paged<T> {
  const pages = useRef(1);
  const resource = useResource(async () => {
    const items: T[] = [];
    let after: number | undefined;
    for (let page = 0; page < pages.current; page++) {
      const loaded = await fetchPage(after);
      items.push(...loaded.items);
      if (loaded.next_after === null || loaded.next_after === undefined) {
        return { items, next: null };
      }
      after = loaded.next_after;
    }
    return { items, next: after ?? null };
  }, deps);
  const canLoadMore = resource.data !== undefined && resource.data.next !== null;
  const loadMore = () => {
    pages.current += 1;
    resource.reload();
  };
  return { ...resource, loadMore: canLoadMore ? loadMore : undefined };
}
