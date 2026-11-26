import type { ThemeMode } from "./types";

// Two palettes share one fixed accent; components use the CSS tokens only.

export interface Palette {
  bg: string;
  surface: string;
  elev: string;
  fg: string;
  muted: string;
  faint: string;
  line: string;
  line2: string;
  warn: string;
  warnbg: string;
  ok: string;
  danger: string;
  dangerbg: string;
  addbg: string;
  addfg: string;
  delbg: string;
  delfg: string;
  code: string;
  bubble: string;
  /** Liquid Glass fills and edges (§9.2). */
  lgFill: string;
  lgFillStrong: string;
  lgEdge: string;
  lgHi: string;
  lgLo: string;
  lgShadow: string;
  lgSpec: string;
  scrim: string;
  segTrack: string;
  segOn: string;
  segShadow: string;
  agentClaude: string;
  agentCodex: string;
  agentAgy: string;
  agentCopilot: string;
  agentGemini: string;
  agentOpencode: string;
  agentPi: string;
  coAnthropic: string;
  coOpenai: string;
  coGoogle: string;
  coOther: string;
  figCost: string;
  heatEmpty: string;
  heatTokens1: string;
  heatTokens2: string;
  heatTokens3: string;
  heatTokens4: string;
  heatTokens5: string;
  heatCost1: string;
  heatCost2: string;
  heatCost3: string;
  heatCost4: string;
  heatCost5: string;
}

export const PALETTES: Record<"light" | "dark", Palette> = {
  dark: {
    bg: "#111113",
    surface: "#1c1c1f",
    elev: "#232327",
    fg: "#F2F2F3",
    muted: "#A3A3AB",
    faint: "#86868F",
    line: "rgba(255,255,255,.08)",
    line2: "rgba(255,255,255,.14)",
    warn: "#F2B45C",
    warnbg: "rgba(242,180,92,.14)",
    ok: "#10B981",
    danger: "#FF6B6B",
    dangerbg: "rgba(255,107,107,.13)",
    addbg: "rgba(92,203,117,.15)",
    addfg: "#5CCB75",
    delbg: "rgba(255,125,125,.14)",
    delfg: "#FF7D7D",
    code: "#18181B",
    bubble: "#2B2B30",
    lgFill: "rgba(46,46,54,.36)",
    lgFillStrong: "rgba(32,32,38,.66)",
    lgEdge: "rgba(255,255,255,.15)",
    lgHi: "rgba(255,255,255,.24)",
    lgLo: "rgba(255,255,255,.05)",
    lgShadow: "0 10px 30px rgba(0,0,0,.38)",
    lgSpec: "rgba(255,255,255,.16)",
    scrim: "rgba(0,0,0,.42)",
    segTrack: "#151518",
    segOn: "#2b2b31",
    segShadow: "0 2px 6px rgba(0,0,0,.45), 0 0 0 1px rgba(255,255,255,.14)",
    agentClaude: "#FB923C",
    agentCodex: "#34D399",
    agentAgy: "#60A5FA",
    agentCopilot: "#A78BFA",
    agentGemini: "#4285F4",
    agentOpencode: "#818CF8",
    agentPi: "#F472B6",
    coAnthropic: "#FB923C",
    coOpenai: "#34D399",
    coGoogle: "#60A5FA",
    coOther: "#A855F7",
    figCost: "#FACC15",
    heatEmpty: "#2a2a2e",
    heatTokens1: "#0B3A53",
    heatTokens2: "#0E5A82",
    heatTokens3: "#0284C7",
    heatTokens4: "#38BDF8",
    heatTokens5: "#7DD3FC",
    heatCost1: "#0E4429",
    heatCost2: "#166534",
    heatCost3: "#22C55E",
    heatCost4: "#4ADE80",
    heatCost5: "#86EFAC",
  },
  light: {
    bg: "#F8F8F6",
    surface: "#FFFFFF",
    elev: "#FFFFFF",
    fg: "#18181A",
    muted: "#55555C",
    faint: "#6A6A72",
    line: "rgba(0,0,0,.08)",
    line2: "rgba(0,0,0,.13)",
    warn: "#9A5B00",
    warnbg: "rgba(166,98,0,.1)",
    ok: "#059669",
    danger: "#C62F2F",
    dangerbg: "rgba(207,52,52,.09)",
    addbg: "rgba(30,138,58,.1)",
    addfg: "#1E7A34",
    delbg: "rgba(199,50,50,.09)",
    delfg: "#B82E2E",
    code: "#F1F1EE",
    bubble: "#EBEBE8",
    lgFill: "rgba(255,255,255,.46)",
    lgFillStrong: "rgba(255,255,255,.76)",
    lgEdge: "rgba(255,255,255,.75)",
    lgHi: "rgba(255,255,255,.95)",
    lgLo: "rgba(0,0,0,.05)",
    lgShadow: "0 10px 30px rgba(0,0,0,.13)",
    lgSpec: "rgba(255,255,255,.55)",
    scrim: "rgba(0,0,0,.28)",
    segTrack: "#EAEAEB",
    segOn: "#FFFFFF",
    segShadow: "0 2px 5px rgba(0,0,0,.08), 0 0 0 1px rgba(0,0,0,.04)",
    agentClaude: "#C2410C",
    agentCodex: "#047857",
    agentAgy: "#2563EB",
    agentCopilot: "#7C3AED",
    agentGemini: "#1A73E8",
    agentOpencode: "#4F46E5",
    agentPi: "#DB2777",
    coAnthropic: "#C2410C",
    coOpenai: "#047857",
    coGoogle: "#2563EB",
    coOther: "#9333EA",
    figCost: "#A16207",
    heatEmpty: "#EBEBE8",
    heatTokens1: "#B8ECFF",
    heatTokens2: "#80E0FF",
    heatTokens3: "#20BEFF",
    heatTokens4: "#0094D1",
    heatTokens5: "#005C86",
    heatCost1: "#9BE9A8",
    heatCost2: "#40C463",
    heatCost3: "#30A14E",
    heatCost4: "#216E39",
    heatCost5: "#0E4429",
  },
};

const ACCENT = {
  light: "#1F6FEB",
  dark: "#5B9CFF",
};

const KEBAB: Record<keyof Palette, string> = {
  bg: "bg",
  surface: "surface",
  elev: "elev",
  fg: "fg",
  muted: "muted",
  faint: "faint",
  line: "line",
  line2: "line2",
  warn: "warn",
  warnbg: "warnbg",
  ok: "ok",
  danger: "danger",
  dangerbg: "dangerbg",
  addbg: "addbg",
  addfg: "addfg",
  delbg: "delbg",
  delfg: "delfg",
  code: "code",
  bubble: "bubble",
  lgFill: "lg-fill",
  lgFillStrong: "lg-fill-strong",
  lgEdge: "lg-edge",
  lgHi: "lg-hi",
  lgLo: "lg-lo",
  lgShadow: "lg-shadow",
  lgSpec: "lg-spec",
  scrim: "scrim",
  segTrack: "seg-track",
  segOn: "seg-on",
  segShadow: "seg-shadow",
  agentClaude: "agent-claude",
  agentCodex: "agent-codex",
  agentAgy: "agent-agy",
  agentCopilot: "agent-copilot",
  agentGemini: "agent-gemini",
  agentOpencode: "agent-opencode",
  agentPi: "agent-pi",
  coAnthropic: "co-anthropic",
  coOpenai: "co-openai",
  coGoogle: "co-google",
  coOther: "co-other",
  figCost: "fig-cost",
  heatEmpty: "heat-empty",
  heatTokens1: "heat-tokens-1",
  heatTokens2: "heat-tokens-2",
  heatTokens3: "heat-tokens-3",
  heatTokens4: "heat-tokens-4",
  heatTokens5: "heat-tokens-5",
  heatCost1: "heat-cost-1",
  heatCost2: "heat-cost-2",
  heatCost3: "heat-cost-3",
  heatCost4: "heat-cost-4",
  heatCost5: "heat-cost-5",
};

export function resolveMode(mode: ThemeMode): "light" | "dark" {
  if (mode !== "system") return mode;
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** Sets the palette and the fixed accent tokens on the document root. */
export function applyTheme(mode: ThemeMode): void {
  if (typeof document === "undefined") return;
  const resolved = resolveMode(mode);
  const root = document.documentElement;
  root.setAttribute("data-theme", resolved);
  root.style.colorScheme = resolved;
  const p = PALETTES[resolved];
  for (const key of Object.keys(KEBAB) as Array<keyof Palette>) root.style.setProperty(`--${KEBAB[key]}`, p[key]);
  const a = ACCENT[resolved];
  const [r, g, b] = hexToRgb(a);
  root.style.setProperty("--accent", a);
  root.style.setProperty("--accentbg", `rgba(${r},${g},${b},${resolved === "dark" ? 0.16 : 0.1})`);
  root.style.setProperty("--onaccent", resolved === "dark" ? "#0B0B0C" : "#FFFFFF");
  if (typeof document.querySelectorAll === "function") {
    document.querySelectorAll('meta[name="theme-color"]').forEach((m) => m.setAttribute("content", p.bg));
  } else {
    document.querySelector?.('meta[name="theme-color"]')?.setAttribute("content", p.bg);
  }
}
