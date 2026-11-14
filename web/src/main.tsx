import "@fontsource-variable/inter-tight";
import "@fontsource-variable/jetbrains-mono";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/components.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { adoptPath } from "./lib/route";
import { ErrorBoundary } from "./ui/ErrorBoundary";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root element");
}

adoptPath();

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
