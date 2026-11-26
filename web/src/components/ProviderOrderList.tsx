import { createEffect, createSignal, For, onCleanup } from "solid-js";
import { patchSettings } from "../api";
import { QUOTA_PROVIDER_NAMES } from "../shared/defaults";
import { showToast, state } from "../store";
import { t } from "../i18n";
import { Icon } from "./ui";

const providerSetting = (id: string) => (id === "antigravity" ? "providers.agy.enabled" : `providers.${id}.enabled`);

function sameOrder(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index]);
}

export function ProviderOrderList() {
  const [order, setOrder] = createSignal<string[]>([...state.quotaProviderOrder]);
  const [dragging, setDragging] = createSignal<string | null>(null);
  let list: HTMLDivElement | undefined;
  let dragID: string | null = null;
  let dragPointer: number | null = null;
  let dragStart: string[] = [];
  let dragStartY = 0;
  let moved = false;

  createEffect(() => {
    if (!dragID) setOrder([...state.quotaProviderOrder]);
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

  const moveTo = (id: string, target: number) => {
    const current = order();
    const from = current.indexOf(id);
    if (from < 0 || target < 0 || target >= current.length || target === from) return;
    const next = current.slice();
    next.splice(from, 1);
    next.splice(target, 0, id);
    setOrder(next);
  };

  const stopListeners = () => {
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", cancelDrag);
    document.body.classList.remove("reordering");
  };

  const onPointerMove = (event: PointerEvent) => {
    if (!dragID || dragPointer === null || event.pointerId !== dragPointer || !list) return;
    event.preventDefault();
    if (Math.abs(event.clientY - dragStartY) > 4) moved = true;
    const rows = Array.from(list.querySelectorAll<HTMLElement>("[data-provider]"));
    let target = rows.length - 1;
    for (let index = 0; index < rows.length; index += 1) {
      const rect = rows[index].getBoundingClientRect();
      if (event.clientY < rect.top + rect.height / 2) {
        target = index;
        break;
      }
    }
    moveTo(dragID, target);
  };

  const finish = (event: PointerEvent) => {
    if (event.pointerId !== dragPointer) return;
    const id = dragID;
    stopListeners();
    dragID = null;
    dragPointer = null;
    setDragging(null);
    if (!id || !moved) return;
    const next = order();
    if (!sameOrder(next, dragStart)) void saveOrder(next, dragStart);
  };

  const cancelDrag = (event: PointerEvent) => {
    if (event.pointerId !== dragPointer) return;
    const restore = dragStart;
    stopListeners();
    dragID = null;
    dragPointer = null;
    setDragging(null);
    if (restore.length) setOrder(restore);
  };

  const startDrag = (event: PointerEvent, id: string) => {
    if (event.button !== 0 || dragID) return;
    event.preventDefault();
    (event.currentTarget as HTMLElement | null)?.setPointerCapture(event.pointerId);
    dragID = id;
    dragPointer = event.pointerId;
    dragStart = order();
    dragStartY = event.clientY;
    moved = false;
    setDragging(id);
    document.body.classList.add("reordering");
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", cancelDrag);
  };

  const moveBy = (id: string, delta: number) => {
    const current = order();
    const from = current.indexOf(id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= current.length) return;
    const next = current.slice();
    next.splice(from, 1);
    next.splice(to, 0, id);
    setOrder(next);
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

  onCleanup(stopListeners);

  return (
    <>
      <div
        class="tp-card"
        style={{ overflow: "hidden" }}
        ref={(element) => (list = element)}
      >
        <For each={order()}>
          {(id) => {
            const enabled = () => state.quotaProviders[id] !== false;
            const name = () => QUOTA_PROVIDER_NAMES[id] || id;
            return (
              <div class="li provider-row" data-provider={id} classList={{ dragging: dragging() === id }}>
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
