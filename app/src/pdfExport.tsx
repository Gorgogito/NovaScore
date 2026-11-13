import { render } from "solid-js/web";
import { layoutScore, LayoutOptions, LayoutResult, LSystem } from "./layout";
import { ScoreView } from "./ScoreView";
import { displayScore, type Score } from "./types";

/** A4 proportions at the layout's 1000-unit page width. */
const PAGE_W = 1000;
const PAGE_H = 1414;
const FIRST_TOP = 110; // matches layout TOP (title area on page one)
const NEXT_TOP = 80;
const BOTTOM = 70;

// The SVG uses CSS variables for colours; the PDF converter needs literal values.
const COLORS: Record<string, string> = {
  "var(--ink)": "#1d1d1f",
  "var(--ink-dim)": "#77777c",
  "var(--accent)": "#1d1d1f",
  "var(--play)": "#1d1d1f",
};

/** Split the systems of a laid-out score over A4 pages. */
export function paginate(layout: LayoutResult): LayoutResult[] {
  const sys = layout.systems;
  const pitch = sys.length > 1 ? sys[1].top - sys[0].top : 200;
  const heightOf = (top: number) => PAGE_H - BOTTOM - top;
  const perPage = (top: number) => Math.max(1, Math.floor((heightOf(top) + 70) / pitch));
  const pages: LayoutResult[] = [];
  let i = 0;
  while (i < sys.length || pages.length === 0) {
    const top = pages.length === 0 ? FIRST_TOP : NEXT_TOP;
    const n = perPage(top);
    const slice: LSystem[] = sys.slice(i, i + n).map((s, j) => ({ ...s, top: top + j * pitch }));
    pages.push({ width: PAGE_W, height: PAGE_H, systems: slice, staffTops: layout.staffTops, tabDy: layout.tabDy });
    i += n;
  }
  return pages;
}

/** Render each page offscreen with the real score view and return standalone SVG strings. */
export function scoreToSvgPages(realScore: Score, opts: LayoutOptions = {}): string[] {
  const score = displayScore(realScore);
  const pages = paginate(layoutScore(score, PAGE_W, opts));
  const host = document.createElement("div");
  host.style.cssText = "position:fixed;left:-99999px;top:0;visibility:hidden";
  document.body.appendChild(host);
  try {
    return pages.map((layout, i) => {
      let svg = "";
      const dispose = render(
        () => (
          <ScoreView
            score={score}
            layout={layout}
            cursor={{ part: -1, voice: -1, m: -1, start: -1 }}
            playing={null}
            inputMode={false}
            onPick={() => {}}
            printMode
            showTitle={i === 0}
            footer={pages.length > 1 ? `${score.title} — ${i + 1}/${pages.length}` : undefined}
          />
        ),
        host,
      );
      const el = host.querySelector("svg");
      if (el) svg = new XMLSerializer().serializeToString(el);
      dispose();
      host.textContent = "";
      for (const [k, v] of Object.entries(COLORS)) svg = svg.split(k).join(v);
      return svg;
    });
  } finally {
    host.remove();
  }
}
