import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { ICONS, type IconName } from "../icons";
import { navigate, state } from "../store";
import { t } from "../i18n";
import { formatModelNameWithRecipes } from "../shared/model-name";

export function Icon(props: { name: IconName; sm?: boolean; class?: string }) {
  return (
    <svg
      class={`i${props.sm ? " sm" : ""}${props.class ? ` ${props.class}` : ""}`}
      viewBox="0 0 24 24"
      innerHTML={ICONS[props.name]}
      aria-hidden="true"
    />
  );
}

/** TokenPulse has no agent model recipes, so this is the shared fallback path. */
export function formatModelName(
  agent: string | ReadonlyArray<string | null | undefined> | null | undefined,
  model: string | null | undefined,
): string {
  return formatModelNameWithRecipes({}, agent, model);
}

export { ago } from "../i18n";

/** Shared walk for horizontal-overflow ancestors; `canScroll` decides whether one blocks the caller. */
function horizontalOverflow(
  target: EventTarget | null,
  stopAt: Element | null | undefined,
  canScroll: (el: HTMLElement, style: CSSStyleDeclaration) => boolean,
): boolean {
  let el = target as HTMLElement | null;
  while (el && el !== stopAt && el !== document.body && el !== document.documentElement) {
    if (el.scrollWidth > el.clientWidth + 1) {
      try {
        const style = window.getComputedStyle(el);
        if ((style.overflowX === "auto" || style.overflowX === "scroll") && canScroll(el, style)) return true;
      } catch {
        // An element can disappear while a gesture is in flight.
      }
    }
    el = el.parentElement;
  }
  return false;
}

export function canScrollRightward(target: EventTarget | null, stopAt?: Element | null): boolean {
  return horizontalOverflow(target, stopAt, (el, style) => style.flexDirection.includes("reverse") || el.scrollLeft > 0);
}

export function insideHorizontalScroller(target: EventTarget | null): boolean {
  return horizontalOverflow(target, undefined, () => true);
}

/** A bottom sheet over a scrim; a tap on the scrim closes it. */
export function Sheet(props: {
  title?: JSX.Element;
  sub?: string;
  onClose: () => void;
  onBack?: () => void;
  right?: JSX.Element;
  full?: boolean;
  children: JSX.Element;
  flush?: boolean;
}) {
  let startX = 0;
  let startY = 0;
  let lastX = 0;
  let lastT = 0;
  let vx = 0;
  let downEligible = false;
  let edgeMode: "none" | "pending" | "horiz" = "none";
  let sheetEl: HTMLDivElement | undefined;
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(settleTimer));
  const [edgeDx, setEdgeDx] = createSignal(0);
  const [edgeSettling, setEdgeSettling] = createSignal(false);

  const canSwipeDown = (target: EventTarget | null, sheet: HTMLElement): boolean => {
    let el = target as HTMLElement | null;
    while (el && el !== sheet) {
      if (el.scrollTop > 0) return false;
      el = el.parentElement;
    }
    return true;
  };
  const handleTouchMove = (event: TouchEvent) => {
    if (edgeMode === "none" || event.touches.length !== 1) return;
    const touch = event.touches[0];
    const dx = touch.clientX - startX;
    const dy = Math.abs(touch.clientY - startY);
    if (edgeMode === "pending") {
      if (dy > 12 && dy > dx) {
        edgeMode = "none";
        return;
      }
      if (dx > 10 && dx > dy * 1.15) {
        edgeMode = "horiz";
        downEligible = false;
        setEdgeSettling(false);
      } else {
        return;
      }
    }
    if (event.cancelable) event.preventDefault();
    const now = performance.now();
    const dt = now - lastT;
    if (dt > 0) vx = 0.6 * ((touch.clientX - lastX) / dt) + 0.4 * vx;
    lastX = touch.clientX;
    lastT = now;
    setEdgeDx(Math.max(0, dx));
  };
  onMount(() => {
    if (!sheetEl) return;
    const el = sheetEl;
    el.addEventListener("touchmove", handleTouchMove, { passive: false });
    onCleanup(() => el.removeEventListener("touchmove", handleTouchMove));
  });
  return (
    <div class="overlay" role="dialog" aria-modal="true">
      <div class="scrim" onClick={() => props.onClose()} />
      <div
        ref={sheetEl}
        class={`sheet${props.full ? " full" : ""}`}
        style={
          edgeDx() > 0 || edgeSettling()
            ? {
                transform: `translate3d(${Math.round(edgeDx())}px, 0, 0)`,
                transition: edgeSettling() ? "transform 200ms cubic-bezier(.2,.85,.3,1)" : "none",
              }
            : undefined
        }
        onTouchStart={(event) => {
          if (event.touches.length !== 1 || edgeSettling()) {
            downEligible = false;
            edgeMode = "none";
            return;
          }
          startX = event.touches[0].clientX;
          startY = event.touches[0].clientY;
          lastX = startX;
          lastT = performance.now();
          vx = 0;
          downEligible = canSwipeDown(event.target, event.currentTarget);
          edgeMode =
            props.full && startX <= 44 && !canScrollRightward(event.target, event.currentTarget) ? "pending" : "none";
        }}
        onTouchEnd={(event) => {
          const touch = event.changedTouches[0];
          if (edgeMode === "horiz") {
            edgeMode = "none";
            const dx = touch ? touch.clientX - startX : edgeDx();
            const width = window.innerWidth || 390;
            const done = dx >= Math.min(120, width * 0.32) || (vx >= 0.5 && dx > 32);
            const act = () => (props.onBack ? props.onBack() : props.onClose());
            if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
              setEdgeDx(0);
              if (done) act();
              return;
            }
            setEdgeSettling(true);
            setEdgeDx(done ? width : 0);
            clearTimeout(settleTimer);
            settleTimer = setTimeout(() => {
              settleTimer = undefined;
              setEdgeSettling(false);
              setEdgeDx(0);
              if (done) act();
            }, 200);
            return;
          }
          edgeMode = "none";
          if (!downEligible || !touch) return;
          downEligible = false;
          const dy = touch.clientY - startY;
          const dx = Math.abs(touch.clientX - startX);
          if (dy > 70 && dy > dx * 1.5) props.onClose();
        }}
        onTouchCancel={() => {
          if (edgeMode === "horiz") {
            edgeMode = "none";
            setEdgeSettling(true);
            setEdgeDx(0);
            clearTimeout(settleTimer);
            settleTimer = setTimeout(() => {
              settleTimer = undefined;
              setEdgeSettling(false);
            }, 200);
          } else {
            edgeMode = "none";
          }
          downEligible = false;
        }}
      >
        <div class="grab" />
        <div class="sh">
          <Show
            when={props.onBack}
            fallback={
              <button class="cbtn" aria-label={t("common.close")} onClick={() => props.onClose()}>
                <Icon name="x" />
              </button>
            }
          >
            <button class="cbtn" aria-label={t("common.back")} onClick={() => props.onBack!()}>
              <Icon name="chevL" />
            </button>
          </Show>
          <div class="t">
            {props.title}
            <Show when={props.sub}>
              <small>{props.sub}</small>
            </Show>
          </div>
          <div>{props.right}</div>
        </div>
        <div class={`body${props.flush ? " flush" : ""}`}>{props.children}</div>
      </div>
    </div>
  );
}

export function FeatureDisabledNotice(props: { message: string }) {
  return (
    <div
      style={{
        margin: "14px 0 0",
        background: "var(--warnbg)",
        border: "1px solid var(--warn)",
        "border-radius": "18px",
        padding: "18px",
        display: "flex",
        "flex-direction": "column",
        gap: "14px",
        color: "var(--warn)",
      }}
    >
      <div style={{ display: "flex", "align-items": "flex-start", gap: "10px", "font-size": "14px", "font-weight": 550, "line-height": "1.5" }}>
        <span style={{ flex: "none", "margin-top": "2px" }}>
          <Icon name="alert" sm />
        </span>
        <span>{props.message}</span>
      </div>
      <button
        onClick={() => navigate({ name: "settings" })}
        style={{
          display: "flex",
          "align-items": "center",
          "justify-content": "center",
          gap: "6px",
          width: "100%",
          "min-height": "42px",
          "border-radius": "12px",
          background: "var(--surface)",
          border: "1px solid var(--warn)",
          color: "var(--warn)",
          "font-size": "14px",
          "font-weight": 600,
        }}
      >
        <Icon name="gear" sm />
        {t("quota.openSettings")}
      </button>
    </div>
  );
}
