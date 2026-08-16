import { defineEcConfig } from "@astrojs/starlight/expressive-code";

const palettes = {
  day: {
    bg: "#e1e2e7",
    fg: "#3760bf",
    comment: "#848cb5",
    blue: "#2e7de9",
    cyan: "#007197",
    green: "#587539",
    orange: "#b15c00",
    purple: "#7847bd",
    red: "#f52a65",
  },
  night: {
    bg: "#1a1b26",
    fg: "#c0caf5",
    comment: "#565f89",
    blue: "#7aa2f7",
    cyan: "#7dcfff",
    green: "#73daca",
    orange: "#ff9e64",
    purple: "#bb9af7",
    red: "#f7768e",
  },
};

function theme(name, type, p) {
  return {
    name,
    displayName: type === "dark" ? "Tokyo Night" : "Tokyo Night Day",
    type,
    colors: {
      "editor.background": p.bg,
      "editor.foreground": p.fg,
      "editorLineNumber.foreground": p.comment,
      "editorLineNumber.activeForeground": p.fg,
      "editor.selectionBackground": `${p.blue}44`,
      "editorCursor.foreground": p.fg,
      "editorIndentGuide.background": `${p.comment}55`,
      "editorIndentGuide.activeBackground": p.blue,
      "textCodeBlock.background": p.bg,
      "textLink.foreground": p.cyan,
    },
    tokenColors: [
      {
        scope: ["comment", "punctuation.definition.comment"],
        settings: { foreground: p.comment, fontStyle: "italic" },
      },
      {
        scope: ["keyword", "storage", "storage.type"],
        settings: { foreground: p.purple },
      },
      {
        scope: ["string", "constant.character"],
        settings: { foreground: p.green },
      },
      {
        scope: ["constant.numeric", "constant.language"],
        settings: { foreground: p.orange },
      },
      {
        scope: ["entity.name.function", "support.function"],
        settings: { foreground: p.blue },
      },
      {
        scope: ["entity.name.type", "support.type"],
        settings: { foreground: p.cyan },
      },
      { scope: ["entity.name.tag"], settings: { foreground: p.red } },
      {
        scope: ["keyword.operator", "punctuation"],
        settings: { foreground: p.cyan },
      },
      {
        scope: ["variable", "entity.name.variable"],
        settings: { foreground: p.fg },
      },
      {
        scope: ["invalid", "invalid.illegal"],
        settings: { foreground: p.red },
      },
    ],
  };
}

export default defineEcConfig({
  themes: [
    theme("tokyo-night", "dark", palettes.night),
    theme("tokyo-night-day", "light", palettes.day),
  ],
  useStarlightDarkModeSwitch: true,
  useStarlightUiThemeColors: false,
  styleOverrides: {
    codePaddingInline: "1.25rem",
    codePaddingBlock: "1rem",
  },
});
