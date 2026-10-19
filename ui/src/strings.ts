// Every user-visible string lives here (BUILD.md section 11, Accessibility: English only,
// all strings in one file). Components import from this module and never inline text.

export const strings = {
  appName: "Zero Mail",
} as const;

export type Strings = typeof strings;
