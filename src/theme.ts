// Light / dark / follow-system theme, remembered per browser.
import { useEffect, useState } from "react";

export type ThemePref = "light" | "dark" | "system";
const KEY = "ccm_theme";

function load(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // Storage unavailable: follow the system.
  }
  return "system";
}

function apply(pref: ThemePref) {
  const dark = pref === "dark" || (pref === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export function useTheme() {
  const [pref, setPref] = useState<ThemePref>(load);
  useEffect(() => {
    apply(pref);
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Not persisted; fine for this session.
    }
    if (pref !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => apply("system");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [pref]);
  const next = () => setPref(pref === "light" ? "dark" : pref === "dark" ? "system" : "light");
  return { pref, setPref, next };
}

// Apply before first paint to avoid a flash.
apply(load());
