import { createSignal, Show } from "solid-js";
import { pair, ApiError } from "../api";
import { forgetPairing, state } from "../store";
import { Icon } from "./ui";
import { t } from "../i18n";

export function PairPage(props: { onPaired: () => Promise<void> }) {
  const [token, setToken] = createSignal("");
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!token().trim() || busy()) return;
    setBusy(true);
    setError("");
    try {
      await pair(token().trim());
      await props.onPaired();
    } catch (err: any) {
      setError(err?.message || t("pair.failed"));
      if (err instanceof ApiError && err.status === 401) {
        forgetPairing();
        state.paired = false;
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="page">
      <div class="hdr">
        <div style={{ width: "44px" }} />
        <div class="ht">
          <div class="t">{t("pair.title")}</div>
        </div>
        <div style={{ width: "44px" }} />
      </div>
      <div class="scroll">
        <div class="settings" style={{ "padding-top": "24px" }}>
          <div style={{ display: "flex", "justify-content": "center", "margin-bottom": "18px" }}>
            <img
              src="/icon-192.png?v=2"
              alt="TokenPulse"
              width="72"
              height="72"
              style={{
                width: "72px",
                height: "72px",
                "border-radius": "18px",
                display: "block",
                "box-shadow": "0 8px 24px rgba(0,0,0,.28)",
              }}
            />
          </div>
          <p class="note" style={{ "text-align": "center", "font-size": "14px" }}>{t("pair.hint")}</p>
          <Show when={state.bootError}>
            <div
              style={{
                background: "var(--warnbg)",
                border: "1px solid var(--warn)",
                "border-radius": "14px",
                padding: "10px 14px",
                "font-size": "13px",
                color: "var(--warn)",
                display: "flex",
                "align-items": "center",
                gap: "8px",
                margin: "12px 0",
              }}
            >
              <Icon name="alert" sm />
              <span>{state.bootError}</span>
            </div>
          </Show>
          <form onSubmit={submit} style={{ "margin-top": "16px" }}>
            <label style={{ display: "flex", "flex-direction": "column", gap: "6px", "font-size": "12.5px", color: "var(--muted)" }}>
              {t("pair.token")}
              <input
                class="inp"
                type="text"
                autocomplete="off"
                autocapitalize="off"
                spellcheck={false}
                value={token()}
                onInput={(event) => setToken(event.currentTarget.value)}
              />
            </label>
            <Show when={error()}>
              <p class="note" style={{ color: "var(--danger)" }}>{error()}</p>
            </Show>
            <button
              class="btn acc blk"
              type="submit"
              style={{ "margin-top": "16px" }}
              disabled={busy() || !token().trim()}
            >
              {busy() ? t("common.sending") : t("pair.connect")}
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}
