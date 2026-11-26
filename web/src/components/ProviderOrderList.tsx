import { batch, createEffect, createSignal, For, onCleanup } from "solid-js";
import { patchSettings } from "../api";
import { QUOTA_PROVIDER_NAMES } from "../shared/defaults";
import { showToast, state } from "../store";
import { t } from "../i18n";
import { Icon } from "./ui";

const providerSetting = (id: string) => (id === "antigravity" ? "providers.agy.enabled" : `providers.${id}.enabled`);
const SETTLE_MS = 180;

interface RowMetric {
  top: number;
  height: number;
  mid: number;
}

function sameOrder(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index]);
}

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

export function ProviderOrderList() {
  const [order, setOrder] = createSignal<string[]>([...state.quotaProviderOrder]);
  const [dragging, setDragging] = createSignal<string | null>(null);
  const [settling, setSettling] = createSignal(false);
  const [dragOffset, setDragOffset] = createSignal(0);
  const [fromIndex, setFromIndex] = createSignal(-1);
  const [targetIndex, setTargetIndex] = createSignal(-1);
  const [dragHeight, setDragHeight] = createSignal(0);

  let list: HTMLDivElement | undefined;
  let dragID: string | null = null;
  let dragPointer: number | null = null;
  let dragStart: string[] = [];
  let dragStartY = 0;
  let rowMetrics: RowMetric[] = [];
  let minDy = 0;
  let maxDy = 0;
  let moved = false;
  let settleTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingCommit: { next: string[]; previous: string[] } | null = null;

  createEffect(() => {
    if (!dragID && !settleTimer) setOrder([...state.quotaProviderOrder]);
  });

  const saveOrder = async (next: string[], previous: string[]) => {
    const previousState = [...state.quotaProviderOrder];
    state.quotaProviderOrder = [...next];
    setOrder([...next]);
    try {
      await patchSettings({ "display.quota_provider_order": next });
    } catch (error: any) {
      state.quotaProviderOrder = previousState;
      setOrder(previous);
      showToast(error?.message || t("usage.loadFailed"));
    }
  };

  const resetDragState = () => {
    batch(() => {
      setDragging(null);
      setSettling(false);
      setDragOffset(0);
      setFromIndex(-1);
      setTargetIndex(-1);
      setDragHeight(0);
    });
  };

  const flushSettle = () => {
    if (settleTimer !== null) {
      clearTimeout(settleTimer);
      settleTimer = null;
    }
    const commit = pendingCommit;
    pendingCommit = null;
    batch(() => {
      if (commit) setOrder(commit.next);
      resetDragState();
    });
    if (commit && !sameOrder(commit.next, commit.previous)) {
      void saveOrder(commit.next, commit.previous);
    }
  };

  const stopListeners = () => {
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", cancelDrag);
    window.removeEventListener("keydown", onDragKeyDown);
    document.body.classList.remove("reordering");
  };

  const computeTarget = (from: number, dy: number): number => {
    if (from < 0 || from >= rowMetrics.length) return from;
    const center = rowMetrics[from].mid + dy;
    let target = from;
    if (dy > 0) {
      for (let i = from + 1; i < rowMetrics.length; i += 1) {
        if (center > rowMetrics[i].mid) target = i;
        else break;
      }
    } else if (dy < 0) {
      for (let i = from - 1; i >= 0; i -= 1) {
        if (center < rowMetrics[i].mid) target = i;
        else break;
      }
    }
    return target;
  };

  const computeFinalOffset = (from: number, target: number): number => {
    if (from < 0 || target < 0 || from === target || !rowMetrics[from] || !rowMetrics[target]) return 0;
    const height = rowMetrics[from].height;
    const slotTop =
      target > from
        ? rowMetrics[target].top + rowMetrics[target].height - height
        : rowMetrics[target].top;
    return slotTop - rowMetrics[from].top;
  };

  const onPointerMove = (event: PointerEvent) => {
    if (!dragID || dragPointer === null || event.pointerId !== dragPointer) return;
    event.preventDefault();
    const rawDy = event.clientY - dragStartY;
    if (Math.abs(rawDy) > 3) moved = true;
    const dy = Math.max(minDy, Math.min(maxDy, rawDy));
    const from = fromIndex();
    const target = computeTarget(from, dy);
    batch(() => {
      setDragOffset(dy);
      setTargetIndex(target);
    });
  };

  const endDrag = (commit: boolean) => {
    const id = dragID;
    const from = fromIndex();
    const target = commit && moved ? targetIndex() : from;
    const previous = dragStart;
    stopListeners();
    dragID = null;
    dragPointer = null;
    if (!id) {
      resetDragState();
      return;
    }

    let next = previous;
    if (commit && moved && from >= 0 && target >= 0 && target !== from) {
      next = previous.slice();
      next.splice(from, 1);
      next.splice(target, 0, id);
    }

    const finalDy = computeFinalOffset(from, target);
    const changed = !sameOrder(next, previous);
    pendingCommit = changed ? { next, previous } : null;

    if (prefersReducedMotion()) {
      flushSettle();
      return;
    }

    batch(() => {
      setTargetIndex(target);
      setSettling(true);
      setDragOffset(finalDy);
    });
    settleTimer = setTimeout(flushSettle, SETTLE_MS);
  };

  const finish = (event: PointerEvent) => {
    if (event.pointerId !== dragPointer) return;
    endDrag(true);
  };

  const cancelDrag = (event: PointerEvent) => {
    if (event.pointerId !== dragPointer) return;
    endDrag(false);
  };

  const onDragKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !dragID) return;
    event.preventDefault();
    endDrag(false);
  };

  const startDrag = (event: PointerEvent, id: string) => {
    if (event.button !== 0 || dragID || !list) return;
    if (settleTimer !== null) flushSettle();
    const current = order();
    const from = current.indexOf(id);
    if (from < 0) return;

    const rows = Array.from(list.querySelectorAll<HTMLElement>("[data-provider]"));
    if (rows.length !== current.length) return;

    event.preventDefault();
    try {
      (event.currentTarget as HTMLElement | null)?.setPointerCapture(event.pointerId);
    } catch {
      // Ignore if pointer capture is unavailable for synthetic events.
    }

    rowMetrics = rows.map((row) => ({
      top: row.offsetTop,
      height: row.offsetHeight,
      mid: row.offsetTop + row.offsetHeight / 2,
    }));
    const height = rowMetrics[from].height;
    const first = rowMetrics[0];
    const last = rowMetrics[rowMetrics.length - 1];
    minDy = first.top - rowMetrics[from].top - 8;
    maxDy = last.top + last.height - (rowMetrics[from].top + height) + 8;

    dragID = id;
    dragPointer = event.pointerId;
    dragStart = current;
    dragStartY = event.clientY;
    moved = false;

    batch(() => {
      setFromIndex(from);
      setTargetIndex(from);
      setDragHeight(height);
      setDragOffset(0);
      setSettling(false);
      setDragging(id);
    });

    document.body.classList.add("reordering");
    window.addEventListener("pointermove", onPointerMove, { passive: false });
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", cancelDrag);
    window.addEventListener("keydown", onDragKeyDown);
  };

  const animateFlip = (before: Map<string, number>) => {
    if (!list || prefersReducedMotion()) return;
    const rows = Array.from(list.querySelectorAll<HTMLElement>("[data-provider]"));
    for (const row of rows) {
      const id = row.dataset.provider;
      if (!id) continue;
      const oldTop = before.get(id);
      if (oldTop === undefined) continue;
      const delta = oldTop - row.getBoundingClientRect().top;
      if (Math.abs(delta) < 1) continue;
      row.animate(
        [{ translate: `0 ${delta}px` }, { translate: "0 0px" }],
        { duration: SETTLE_MS, easing: "cubic-bezier(0.2, 0.9, 0.2, 1)" },
      );
    }
  };

  const moveBy = (id: string, delta: number) => {
    if (settleTimer !== null) flushSettle();
    const current = order();
    const from = current.indexOf(id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= current.length) return;

    const before = new Map<string, number>();
    if (list) {
      for (const row of Array.from(list.querySelectorAll<HTMLElement>("[data-provider]"))) {
        if (row.dataset.provider) before.set(row.dataset.provider, row.getBoundingClientRect().top);
      }
    }

    const next = current.slice();
    next.splice(from, 1);
    next.splice(to, 0, id);
    setOrder(next);
    animateFlip(before);
    void saveOrder(next, current);
  };

  const toggle = async (id: string) => {
    const enabled = state.quotaProviders[id] !== false;
    state.quotaProviders[id] = !enabled;
    try {
      await patchSettings({ [providerSetting(id)]: !enabled });
    } catch (error: any) {
      state.quotaProviders[id] = enabled;
      showToast(error?.message || t("usage.loadFailed"));
    }
  };

  const rowTranslate = (id: string, index: number): string | undefined => {
    const active = dragging();
    if (!active) return undefined;
    if (active === id) return `0px ${dragOffset()}px`;
    const from = fromIndex();
    const target = targetIndex();
    const height = dragHeight();
    if (from < target && index > from && index <= target) return `0px ${-height}px`;
    if (from > target && index >= target && index < from) return `0px ${height}px`;
    return "0px 0px";
  };

  onCleanup(() => {
    stopListeners();
    if (settleTimer !== null) {
      clearTimeout(settleTimer);
      settleTimer = null;
    }
  });

  return (
    <>
      <div
        class="tp-card provider-list"
        classList={{ "is-dragging": dragging() !== null }}
        ref={(element) => (list = element)}
      >
        <For each={order()}>
          {(id, index) => {
            const enabled = () => state.quotaProviders[id] !== false;
            const name = () => QUOTA_PROVIDER_NAMES[id] || id;
            const isDragging = () => dragging() === id;
            return (
              <div
                class="li provider-row"
                data-provider={id}
                classList={{ dragging: isDragging(), settling: isDragging() && settling() }}
                style={{ translate: rowTranslate(id, index()) }}
              >
                <button
                  type="button"
                  class="provider-drag"
                  aria-label={t("settings.reorderProvider", { name: name() })}
                  onPointerDown={(event) => startDrag(event, id)}
                  onKeyDown={(event) => {
                    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                    event.preventDefault();
                    moveBy(id, event.key === "ArrowUp" ? -1 : 1);
                  }}
                >
                  <Icon name="grip" sm />
                </button>
                <div class="tx">
                  {name()}
                  <small>{enabled() ? t("settings.providerActive") : t("settings.providerDisabled")}</small>
                </div>
                <button
                  type="button"
                  class="tg"
                  classList={{ off: !enabled() }}
                  role="switch"
                  aria-checked={enabled()}
                  aria-label={t("settings.toggleProvider", { name: name() })}
                  onClick={() => void toggle(id)}
                />
              </div>
            );
          }}
        </For>
      </div>
      <p class="note">{t("settings.providerOrderHint")}</p>
    </>
  );
}
