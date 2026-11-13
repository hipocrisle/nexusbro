import { CSSProperties } from "react";
const paths: Record<string, string> = {
  globe: "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0ZM3 12h18M12 3c4 4 4 14 0 18-4-4-4-14 0-18Z",
  plus: "M12 5v14M5 12h14", close: "m6 6 12 12M18 6 6 18", chevron: "m9 5 7 7-7 7",
  back: "m12 5-7 7 7 7M5 12h14", forward: "m12 5 7 7-7 7M5 12h14", reload: "M20 7v5h-5M20 12a8 8 0 1 0-2 5M20 7l-3-3",
  settings: "M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1 1-3ZM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z",
  shield: "M12 3 4 6v6c0 5 8 9 8 9s8-4 8-9V6l-8-3Zm-4 9 3 3 5-6",
  save: "M5 3h12l4 4v14H3V3h2ZM7 3v6h10V3M7 21v-8h10v8", archive: "M3 4h18v5H3V4Zm2 5v12h14V9M9 13h6",
  code: "m8 7-5 5 5 5m8-10 5 5-5 5m-3-13-2 20", menu: "M5 6h14M5 12h14M5 18h14",
  monitor: "M3 4h18v13H3V4Zm5 17h8m-4-4v4", network: "M9 3h6v6H9V3ZM3 16h6v5H3v-5Zm12 0h6v5h-6v-5ZM12 9v4M6 16v-3h12v3",
  text: "M5 4h14M12 4v16M8 20h8", tabs: "M4 5h16v15H4V5Zm0 5h16M8 5v5", check: "m5 12 4 4L19 6", pin: "m9 3 6 0-1 5 4 4H6l4-4-1-5Zm3 9v9",
};
export default function Icon({ name, size = 18, style }: { name: string; size?: number; style?: CSSProperties }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={style}><path d={paths[name] || paths.globe} /></svg>;
}
