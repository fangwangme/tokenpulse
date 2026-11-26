import { createSignal } from "solid-js";
import { en } from "./en";
import { zh } from "./zh";

// Messages shared by the phone app.

/** A message: plain text, or plural forms chosen by `Intl.PluralRules` from the `count` parameter. */
export type Message = string | ({ other: string } & Partial<Record<Intl.LDMLPluralRule, string>>);
export type MessageId = keyof typeof en;
export type Messages = Record<MessageId, Message>;
export type Params = Record<string, string | number>;

/** One entry per language. English is the reference; another language may lack messages (§10.3). */
export const LOCALES = {
  zh: { label: "中文", htmlLang: "zh-CN", messages: zh as Partial<Messages> },
  en: { label: "English", htmlLang: "en", messages: en as Partial<Messages> },
} satisfies Record<string, { label: string; htmlLang: string; messages: Partial<Messages> }>;

export type Locale = keyof typeof LOCALES;
export type LocalePref = "system" | Locale;

/** An explicit choice wins; else the first preferred language the app has; else English. */
export function resolveLocale(pref: LocalePref, languages: readonly string[]): Locale {
  if (pref !== "system") return pref;
  for (const tag of languages) {
    const base = tag.toLowerCase().split("-")[0];
    if (base in LOCALES) return base as Locale;
  }
  return "en";
}

function fill(text: string, params?: Params): string {
  return params ? text.replace(/\{(\w+)\}/g, (whole, name) => (name in params ? String(params[name]) : whole)) : text;
}

function pick(locale: Locale, id: MessageId, params?: Params): string {
  const msg: Message = LOCALES[locale].messages[id] ?? en[id];
  if (typeof msg === "string") return msg;
  const form = new Intl.PluralRules(LOCALES[locale].htmlLang).select(Number(params?.count ?? 0));
  return msg[form] ?? msg.other;
}

export function translate(locale: Locale, id: MessageId, params?: Params): string {
  return fill(pick(locale, id, params), params);
}

/** "2 hours ago" / "2小时前"; under a minute is "just now"; past 30 days, the date. */
export function relativeTime(locale: Locale, ts: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ts) / 1000));
  if (s < 60) return translate(locale, "time.justNow");
  const rtf = new Intl.RelativeTimeFormat(LOCALES[locale].htmlLang, { numeric: "always" });
  if (s < 3600) return rtf.format(-Math.floor(s / 60), "minute");
  if (s < 86400) return rtf.format(-Math.floor(s / 3600), "hour");
  if (s < 86400 * 30) return rtf.format(-Math.floor(s / 86400), "day");
  return new Date(ts).toLocaleDateString(LOCALES[locale].htmlLang);
}

const [locale, setLocaleSignal] = createSignal<Locale>("en");
export { locale };

/** Sets the language in use; the store calls it when the choice or the system language changes. */
export function setLocale(next: Locale): void {
  setLocaleSignal(next);
  if (typeof document !== "undefined") document.documentElement.lang = LOCALES[next].htmlLang;
}

/** The current language's text for `id`; reactive inside components. */
export function t(id: MessageId, params?: Params): string {
  return translate(locale(), id, params);
}

/**
 * A message with markup inside it: each `{name}` whose part is given is replaced by that element,
 * so word order stays the translator's (`配对：运行 {cmd}` / `Run {cmd} to pair`).
 */
export function rich<T>(id: MessageId, parts: Record<string, T>, params?: Params): Array<string | T> {
  const text = pick(locale(), id, params);
  return text.split(/(\{\w+\})/).map((seg) => {
    const name = /^\{(\w+)\}$/.exec(seg)?.[1];
    if (name && name in parts) return parts[name];
    return fill(seg, params);
  });
}

export function ago(ts: number): string {
  return relativeTime(locale(), ts);
}
