import { createContext, useContext } from "react";
import type { CallRecord, CallSpec } from "./core.ts";

export interface PlaygroundCtx {
  /** Send one authenticated request; resolves to the recorded call. */
  call: (spec: CallSpec) => Promise<CallRecord>;
  /** The in-memory token (null after revoke or before connect). */
  token: string | null;
  /** Mask the token in displayed and copied curl commands. */
  masked: boolean;
  origin: string;
  /** False once the token is revoked, expired or rejected. */
  active: boolean;
}

const Ctx = createContext<PlaygroundCtx | null>(null);

export const PlaygroundProvider = Ctx.Provider;

export function usePlayground(): PlaygroundCtx {
  const v = useContext(Ctx);
  if (!v) throw new Error("usePlayground outside provider");
  return v;
}
