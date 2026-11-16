// Motion Choreographer Tailwind preset. Usage: presets: [require("./tailwind.preset.js")]
module.exports = {
  theme: {
    borderRadius: { none: "0", DEFAULT: "0" },
    boxShadow: { none: "none", DEFAULT: "none" },
    fontFamily: { sans: ["Inter", "Helvetica Neue", "Helvetica", "Arial", "sans-serif"] },
    fontWeight: { normal: "400", bold: "700" },
    extend: {
      colors: {
        ink: "#0A0A0A", paper: "#FFFFFF", signal: "#FF3000",
        grey: { 100: "#F2F2F2", 300: "#D4D4D4", 400: "#A3A3A3", 600: "#6B6B6B", 800: "#2E2E2E", 900: "#161616" },
        program: "#FF3000", preview: "#00C24E", caution: "#FFB800",
      },
      fontSize: {
        label: ["12px", { lineHeight: "16px", letterSpacing: "0.08em" }],
        body: ["16px", { lineHeight: "24px" }],
        data: ["24px", { lineHeight: "32px" }],
        h2: ["32px", { lineHeight: "40px", letterSpacing: "-0.02em" }],
        h1: ["64px", { lineHeight: "64px", letterSpacing: "-0.03em" }],
        display: ["96px", { lineHeight: "96px", letterSpacing: "-0.03em" }],
      },
      transitionDuration: { fast: "120ms", DEFAULT: "200ms" },
      transitionTimingFunction: { DEFAULT: "cubic-bezier(0, 0, 0.2, 1)" },
    },
  },
};
