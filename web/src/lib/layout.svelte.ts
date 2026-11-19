// the one place that decides the screen shape. Pages read
// `layout.shape` instead of scattering media queries:
//   A phone portrait (< 700 wide) · B landscape compact (< 520 high)
//   C tablet portrait (≥ 700 wide) · D wide landscape (≥ 1000 wide)
// A landscape window that is neither compact nor wide (e.g. a
// small tablet sideways) uses B's two-column arrangement.

export type Shape = "A" | "B" | "C" | "D";

const COMPACT = "(orientation: landscape) and (max-height: 519px)";
const WIDE = "(orientation: landscape) and (min-width: 1000px)";
const PORTRAIT = "(orientation: portrait)";
const PORTRAIT_WIDE = "(min-width: 700px)";

function matches(query: string): boolean {
  return window.matchMedia(query).matches;
}

function computeShape(): Shape {
  if (matches(COMPACT)) return "B";
  if (matches(WIDE)) return "D";
  if (matches(PORTRAIT)) return matches(PORTRAIT_WIDE) ? "C" : "A";
  return "B";
}

// mutable reactive object, same pattern as `device` / `connection`
export const layout = $state({
  shape: computeShape(),
});

function refresh(): void {
  layout.shape = computeShape();
}

export function watchLayout(): () => void {
  window.addEventListener("resize", refresh);
  window.addEventListener("orientationchange", refresh);
  return () => {
    window.removeEventListener("resize", refresh);
    window.removeEventListener("orientationchange", refresh);
  };
}
