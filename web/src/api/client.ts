import createClient from "openapi-fetch";
import type { components, paths } from "./schema";

export type Schemas = components["schemas"];

export const api = createClient<paths>({ credentials: "same-origin" });
