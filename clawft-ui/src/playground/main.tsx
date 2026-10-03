import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../index.css";
import { PlaygroundApp } from "./PlaygroundApp.tsx";

// No service worker, no MSW, no storage: this page holds a bearer token and
// keeps it in memory only (ADR-102 D2).
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <PlaygroundApp />
  </StrictMode>,
);
