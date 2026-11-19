<script module lang="ts">
  // the one icon set — inline SVG on a 24 grid, Material Symbols
  // Rounded geometry: transport glyphs are filled, the rest are
  // 2 px round-cap outlines in currentColor
  const icons = {
    play: `<path fill="currentColor" stroke="none" d="M8 6.82v10.36c0 .79.87 1.27 1.54.84l8.14-5.18a1 1 0 0 0 0-1.69L9.54 5.98A1 1 0 0 0 8 6.82z"/>`,
    pause: `<path fill="currentColor" stroke="none" d="M8 19a2 2 0 0 0 2-2V7a2 2 0 0 0-4 0v10a2 2 0 0 0 2 2zm6-12v10a2 2 0 0 0 4 0V7a2 2 0 0 0-4 0z"/>`,
    volumeX: `<path fill="currentColor" stroke="none" d="M11 5 6 9H2v6h4l5 4z"/><line x1="22" y1="9" x2="16" y2="15"/><line x1="16" y1="9" x2="22" y2="15"/>`,
    volumeLow: `<path fill="currentColor" stroke="none" d="M11 5 6 9H2v6h4l5 4z"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>`,
    volumeHigh: `<path fill="currentColor" stroke="none" d="M11 5 6 9H2v6h4l5 4z"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>`,
    power: `<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>`,
    music: `<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>`,
    vu: `<path d="M2 10v4"/><path d="M6 6v12"/><path d="M10 3v18"/><path d="M14 8v8"/><path d="M18 5v14"/><path d="M22 10v4"/>`,
    queue: `<path d="M21 15V6"/><path d="M18.5 18a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z"/><path d="M12 12H3"/><path d="M16 6H3"/><path d="M12 18H3"/>`,
    settings: `<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>`,
    palette: `<circle cx="13.5" cy="6.5" r=".7" fill="currentColor" stroke="none"/><circle cx="17.5" cy="10.5" r=".7" fill="currentColor" stroke="none"/><circle cx="8.5" cy="7.5" r=".7" fill="currentColor" stroke="none"/><circle cx="6.5" cy="12.5" r=".7" fill="currentColor" stroke="none"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"/>`,
    moon: `<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>`,
    home: `<path d="m3 10 9-7 9 7"/><path d="M5 9v11a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9"/><path d="M9 22V12h6v10"/>`,
    bell: `<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>`,
    chevronUp: `<path d="m18 15-6-6-6 6"/>`,
    chevronDown: `<path d="m6 9 6 6 6-6"/>`,
    close: `<path d="M18 6 6 18"/><path d="m6 6 12 12"/>`,
    drag: `<circle cx="9" cy="6" r="1.6" fill="currentColor" stroke="none"/><circle cx="15" cy="6" r="1.6" fill="currentColor" stroke="none"/><circle cx="9" cy="12" r="1.6" fill="currentColor" stroke="none"/><circle cx="15" cy="12" r="1.6" fill="currentColor" stroke="none"/><circle cx="9" cy="18" r="1.6" fill="currentColor" stroke="none"/><circle cx="15" cy="18" r="1.6" fill="currentColor" stroke="none"/>`,
    plus: `<path d="M12 5v14"/><path d="M5 12h14"/>`,
    chevronLeft: `<path d="m15 18-6-6 6-6"/>`,
    chevronRight: `<path d="m9 18 6-6-6-6"/>`,
    external: `<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>`,
    restart: `<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>`,
    sun: `<circle cx="12" cy="12" r="4.2"/><path d="M12 2.5v2.5"/><path d="M12 19v2.5"/><path d="M2.5 12H5"/><path d="M19 12h2.5"/><path d="m5.2 5.2 1.8 1.8"/><path d="m17 17 1.8 1.8"/><path d="m18.8 5.2-1.8 1.8"/><path d="m7 17-1.8 1.8"/>`,
    ma: `<circle cx="12" cy="12" r="9.2"/><path fill="currentColor" stroke="none" d="M10.3 15.3V7.4l6-1.6v2.1l-4 1.1v6"/><circle cx="8.9" cy="15.6" r="1.9" fill="currentColor" stroke="none"/><circle cx="14.9" cy="13" r="1.9" fill="currentColor" stroke="none"/>`,
    alert: `<circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>`,
    remote: `<path d="M8 2h8a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z"/><path d="M12 18h.01"/>`,
    upload: `<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/>`,
    check: `<path d="M20 6 9 17l-5-5"/>`,
    lock: `<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>`,
    wifi: `<path d="M5 12.55a11 11 0 0 1 14.08 0"/><path d="M1.42 9a16 16 0 0 1 21.16 0"/><path d="M8.53 16.11a6 6 0 0 1 6.95 0"/><path d="M12 20h.01"/>`,
  } as const;

  export type IconName = keyof typeof icons;
</script>

<script lang="ts">
  let {
    name,
    size = 24,
  }: {
    name: IconName;
    size?: number;
  } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html icons[name]}
</svg>
