import createClient from "openapi-fetch";
import type { components, paths } from "./schema";

export type Schemas = components["schemas"];

export const api = createClient<paths>({ credentials: "same-origin" });

export const UNAUTHORIZED_EVENT = "buizz-unauthorized";

api.use({
  onResponse({ response }) {
    if (response.status === 401) {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
    }
  },
});

export class ApiFailure extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type Outcome<T> = { data?: T; error?: unknown; response: Response };

function messageOf(error: unknown, response: Response): string {
  if (typeof error === "object" && error !== null && "error" in error && typeof error.error === "string") {
    return error.error;
  }
  return `${response.status} ${response.statusText}`.trim();
}

export async function must<T>(pending: Promise<Outcome<T>>): Promise<T> {
  const { data, error, response } = await pending;
  if (!response.ok || data === undefined) {
    throw new ApiFailure(response.status, messageOf(error, response));
  }
  return data;
}

export async function mustSucceed(pending: Promise<Outcome<unknown>>): Promise<void> {
  const { error, response } = await pending;
  if (!response.ok) {
    throw new ApiFailure(response.status, messageOf(error, response));
  }
}
