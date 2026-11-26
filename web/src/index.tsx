import { render } from "solid-js/web";
import "./styles.css";
import { App } from "./app";

function fitToVisualViewport(): void {
  const viewport = window.visualViewport;
  if (!viewport) return;
  const root = document.documentElement;
  const active = document.activeElement;
  const typing = Boolean(active?.matches?.("input, textarea, [contenteditable]"));
  if (!typing) {
    root.style.removeProperty("--vv-top");
    root.style.removeProperty("--vv-height");
    root.removeAttribute("data-keyboard");
    return;
  }
  root.style.setProperty("--vv-top", `${Math.round(viewport.offsetTop)}px`);
  root.style.setProperty("--vv-height", `${Math.round(viewport.height)}px`);
}

if (typeof window !== "undefined") {
  const standalone = window.matchMedia("(display-mode: standalone)").matches || Boolean((navigator as any).standalone);
  if (standalone) document.documentElement.setAttribute("data-mode", "standalone");
  window.visualViewport?.addEventListener("resize", fitToVisualViewport);
  window.visualViewport?.addEventListener("scroll", fitToVisualViewport);
  window.addEventListener("focusin", fitToVisualViewport);
  window.addEventListener("focusout", () => requestAnimationFrame(fitToVisualViewport));
  fitToVisualViewport();
}

render(() => <App />, document.getElementById("root")!);
