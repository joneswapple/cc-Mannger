/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  // Toggled by src/theme.ts (light / dark / follow system).
  darkMode: "class",
  theme: { extend: {} },
  plugins: [],
};
