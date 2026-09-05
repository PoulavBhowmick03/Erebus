"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

export type KeyState = "held" | "dropped";

type Ctx = {
  keyState: KeyState;
  /** true once the opening reveal has finished, so sections can stagger after it */
  booted: boolean;
  reduced: boolean;
  toggle: () => void;
};

const KeyCtx = createContext<Ctx>({
  keyState: "held",
  booted: true,
  reduced: false,
  toggle: () => {},
});

export const useKey = () => useContext(KeyCtx);

/**
 * Holds the one piece of global state on this page: whether the reader has a
 * viewing key. `data-key` on <html> is what the stylesheet reads; everything
 * visual follows from that single attribute.
 */
export function KeyProvider({ children }: { children: React.ReactNode }) {
  // SSR renders the readable document. The inline boot script has already set
  // <html data-key> before paint, so there is no flash either way.
  const [keyState, setKeyState] = useState<KeyState>("dropped");
  const [booted, setBooted] = useState(false);
  const [reduced, setReduced] = useState(false);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    setReduced(mq.matches);

    if (mq.matches) {
      setKeyState("held");
      setBooted(true);
      return;
    }

    // The reader arrives without a key and is handed one.
    const t = window.setTimeout(() => {
      setKeyState("held");
      setBooted(true);
    }, 900);
    return () => window.clearTimeout(t);
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-key", keyState);
  }, [keyState]);

  const toggle = useCallback(() => {
    setBooted(true);
    setKeyState((k) => (k === "held" ? "dropped" : "held"));
  }, []);

  const value = useMemo(
    () => ({ keyState, booted, reduced, toggle }),
    [keyState, booted, reduced, toggle],
  );

  return <KeyCtx.Provider value={value}>{children}</KeyCtx.Provider>;
}
