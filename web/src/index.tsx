import { render } from "solid-js/web";
import "./styles.css";
import { App } from "./app";

const isTextField = (element: Element | null) => Boolean(element?.matches?.("input, textarea, [contenteditable]"));

function resetWindowScroll(): void {
  if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
}

let fullHeight = 0;
let fullWidth = 0;

function fitToVisualViewport(): void {
  const viewport = window.visualViewport;
  if (!viewport) return;
  const root = document.documentElement;
  if (viewport.width !== fullWidth) {
    fullWidth = viewport.width;
    fullHeight = 0;
  }
  if (!isTextField(document.activeElement)) {
    fullHeight = Math.max(fullHeight, viewport.height);
    root.style.removeProperty("--vv-top");
    root.style.removeProperty("--vv-height");
    root.removeAttribute("data-keyboard");
    return;
  }
  root.style.setProperty("--vv-top", `${Math.round(viewport.offsetTop)}px`);
  root.style.setProperty("--vv-height", `${Math.round(viewport.height)}px`);
  if (fullHeight - viewport.height > 120) root.setAttribute("data-keyboard", "open");
  else root.removeAttribute("data-keyboard");
}

if (typeof window !== "undefined") {
  const standalone = window.matchMedia("(display-mode: standalone)").matches || Boolean((navigator as any).standalone);
  if (standalone) {
    document.documentElement.setAttribute("data-mode", "standalone");
    // Best effort: Android installed apps can lock, iOS rejects this and the
    // manifest orientation, where the existing landscape layout stays usable.
    const orientation = screen.orientation as ScreenOrientation & { lock?: (value: string) => Promise<void> };
    void orientation?.lock?.("portrait").catch(() => {});
  }
  window.addEventListener("focusin", fitToVisualViewport);
  window.addEventListener("focusout", () => {
    // Focus may be moving to another field; act on where it lands.
    requestAnimationFrame(() => {
      fitToVisualViewport();
      if (!isTextField(document.activeElement)) resetWindowScroll();
    });
  });
  window.addEventListener("scroll", () => {
    if (!isTextField(document.activeElement)) resetWindowScroll();
  }, { passive: true });
  window.visualViewport?.addEventListener("resize", fitToVisualViewport);
  window.visualViewport?.addEventListener("scroll", fitToVisualViewport);
  fitToVisualViewport();
}

render(() => <App />, document.getElementById("root")!);
