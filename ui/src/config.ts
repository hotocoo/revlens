/** Every UI tunable lives here. */

/** Cards in one reveal batch that get increasing delay; later ones appear with the last. */
export const REVEAL_STAGGER_MAX = 10;
/** Furthest the join screen backdrop drifts from center, in pixels. */
export const PARALLAX_PX = 14;
/** Width at which criteria sit in a rail instead of a drawer. */
export const WIDE_QUERY = "(min-width: 1024px)";
/** Hunk cards whose diff is expanded on first render (demoted hunks always start collapsed). */
export const OPEN_BY_DEFAULT = 5;
/** Lines shown per hunk before "show all". */
export const MAX_DIFF_LINES = 400;
export const NAME_STORAGE_KEY = "lens.name";
/** Longest comment accepted by the composer (the server caps the whole document's size). */
export const MAX_COMMENT_CHARS = 2000;
export const PRESENCE_COLORS = [
  "#0b7285",
  "#9c36b5",
  "#c2255c",
  "#e8590c",
  "#2f9e44",
  "#1971c2",
  "#7048e8",
  "#a61e4d",
];
