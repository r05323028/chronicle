# Chronicle Website Design

<!-- impeccable:design-schema 1 -->

## Direction

**Quiet manifesto / durable evidence.** Impeccable's assigned direction is the 1914 manifesto page: a typographic field with hard rules and editorial slabs instead of generic product cards. Chronicle tempers that source system with its own material: append-only lines, WAL segment labels, checkpoint marks, session brackets, and replay traces. The result should feel like a technical field manual printed on durable stock, not a vintage costume or SaaS dashboard.

The first surface proves one thing quickly: real application behavior can become replayable evidence without application instrumentation. Documentation inherits the same visual world but keeps Starlight's conventional reading and navigation UX.

## Brand Character

Precise, quiet, technical, durable, trustworthy, minimal, developer-focused. Confidence comes from explicit boundaries and exact commands, not volume, gloss, or invented proof.

## Visual Principles

- **Evidence over decoration.** Every line, marker, label, and diagram should explain capture, durability, transformation, or replay.
- **One field, two inks.** Warm paper and near-black ink carry most surfaces; signal color marks state, not atmosphere.
- **Hard edges, clear joins.** Use rules, offsets, brackets, and rectangular blocks. Avoid floating cards and ornamental softness.
- **Narrative over grid.** Marketing sections follow a behavior from observation through replay, not a three-card feature matrix.
- **Quiet density.** Use generous reading measure and short typographic interruptions: command rows, WAL labels, and timeline rails.
- **Truthful surfaces.** Use actual Chronicle commands and accurately labeled limits/status. No metrics, logos, testimonials, or fake screenshots.

## Typography

Use local/system stacks to avoid large payloads and preserve CJK coverage.

- `--font-display`: `"Arial Narrow", "Avenir Next Condensed", "Hiragino Kaku Gothic ProN", "Yu Gothic", "Noto Sans TC", "Noto Sans JP", sans-serif`. Condensed display voice for short headings and labels; never italic.
- `--font-body`: `ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans TC", "Noto Sans JP", sans-serif`. Long-form documentation and translated content.
- `--font-mono`: `ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Noto Sans Mono CJK TC", monospace`. Technical artifacts only.

Latin body target: 16–18px with 1.65 line-height. CJK body target: 1.8 line-height and no forced uppercase. Display headings use 1.04–1.12 line-height, `font-style: normal`, `overflow-wrap: anywhere`, and a restrained clamp. Use `letter-spacing` only for short uppercase labels.

## Type Scale

Use a fluid, rem-based scale with named tokens:

- `--text-xs`: 0.72rem labels
- `--text-sm`: 0.86rem metadata
- `--text-base`: 1rem body
- `--text-lg`: 1.18rem lead/body emphasis
- `--text-xl`: 1.45rem section heads
- `--text-2xl`: 2.2rem feature heads
- `--text-display`: `clamp(3rem, 8vw, 7.25rem)` landing hero only

Docs reduce display sizes through Starlight surface tokens. Do not use all-caps for long headings.

## Spacing Philosophy

Base unit is 4px; meaningful rhythm uses 8/16/24/40/64/96px. Prefer `clamp()` for page gutters and section spacing. Marketing sections can breathe, but each large gap must separate a change in story. Docs prioritize scanability: 1.5rem paragraph spacing, compact list rhythm, and stable code block margins.

## Color System

All UI colors are tokens; no one-off color values outside token blocks.

- `--color-paper`: #f2efe7 / #1b2422, primary light/dark surface
- `--color-paper-deep`: #e5e0d5 / #25302d, ruled panels
- `--color-ink`: #18211f / #f2efe7, primary text
- `--color-ink-soft`: #53605b / #b9c2bb, secondary text
- `--color-night`: #121918 / #0c1110, dark hero/terminal surface
- `--color-signal`: #a63224 / #ef745c, active state
- `--color-signal-ink`: #fff9ee / #211512, text on signal fill
- `--color-cyan`: #1f6b6e / #72c0bd, checkpoint state
- `--color-night-ink-soft`: #b9c2bb, muted text on dark fields
- `--color-focus`: #a63224 / #ff9d88, keyboard focus
- `--color-line`: color-mix(in srgb, var(--color-ink) 22%, transparent)
- `--code-bg` / `--code-fg`: #e1e2e7 / #3760bf for light code and #1a1b26 / #c0caf5 for dark code.

Page surfaces keep Chronicle's original light/dark palette. Code blocks alone use Tokyo Night Day in light mode and Tokyo Night in dark mode.

Code block syntax uses Tokyo Night semantic colors: Day `#e1e2e7` / `#3760bf` and Night `#1a1b26` / `#c0caf5`, with palette accents for comments, keywords, strings, numbers, and links. Do not apply these tokens to page chrome. Do not use gradients, glows, or decorative neon.

## Layout Principles

- Landing uses a full-bleed dark opening field with a narrow top rail, then a centered content column capped at 76rem.
- Hero visual is an evidence strip: capture socket marks, committed WAL ticks, ETL checkpoint, canonical session bracket, replay arrow. It is a semantic diagram, not an illustration.
- Main story uses an asymmetric editorial split: a persistent left rule/label rail and a right reading column. Mobile collapses to one column with label above heading.
- Flow section is a horizontal sequence at desktop and a vertical ledger at mobile; each stage names the real artifact and its handoff.
- Quick start is a real command transcript on paper with a single adjacent explanation column, not a fake terminal window.
- Final CTA is a compact documentation/repository fork, not a repeated sales banner.
- Docs remain recognizably Starlight: sidebar, breadcrumbs, table of contents, search, code blocks, callouts, pagination. Customization lives in color, type, rules, code surfaces, and diagram components.

## Border / Radius Treatment

Rules are 1px, with occasional 2px structural dividers. Default radius is 0; use at most 2px for focus or code surfaces where needed. No pill buttons, floating glass, shadow stacks, or card grids. Buttons are rectangular ink/signal blocks with a small offset hover state.

## Code / Terminal Treatment

Code is treated as an artifact: monospace, left rule, explicit command prompt, line numbers only when they aid reading. Use actual commands (`chronicle doctor`, `chronicle record --name checkout -- ./my-app`, `chronicle list`, `chronicle inspect checkout`, `chronicle replay checkout -- ./my-app`). Do not draw fake terminal chrome or pretend output. If output is illustrative, label it as an example and keep it tied to documented behavior.

## Diagrams

Use HTML/CSS/SVG only where each mark has semantic meaning. Core vocabulary: event dots, directional lines, WAL commit ticks, checkpoint squares, session brackets, target loopback boundary, denied-effect slash, and replay verification marks. Provide a text alternative or adjacent accessible summary. Prefer static diagrams; animation only traces data flow and never carries meaning alone.

## Motion Philosophy

CSS-only where possible. A short, linear signal trace may progress through capture → WAL → ETL → canonical → replay on load or hover, with a static state always visible. No scroll choreography, blobs, parallax, or looping distraction. `prefers-reduced-motion: reduce` disables transitions/animation and reveals all states.

## Responsive Behavior

Mobile-first. Root `html` and `body` use `overflow-x: clip`. Validate 320, 375, 414, 768, and desktop widths. Clickable labels remain one line; nav collapses rather than wrapping. Use `minmax(0, 1fr)` for image-bearing tracks, logical properties, `dvh` where needed, and content-driven `rem` breakpoints around 40rem/60rem. Section heads with label + heading always collapse to one column.

CJK layouts receive extra vertical room, avoid hard-coded line breaks, and use the same semantic heading hierarchy. Locale controls use native links and clear language names; version controls use native links/select-style navigation with current state announced.

## Accessibility Expectations

Semantic landmarks, one `h1` per page, logical heading order, keyboard-visible `:focus-visible`, skip link, contrast at WCAG AA, `aria-current` on active locale/version links, descriptive diagram text, no hover-only content, and reduced motion. Code blocks remain selectable and readable at narrow widths. Search, navigation, locale, and version controls work as links without JavaScript.

## Anti-patterns

No purple/blue gradients, glassmorphism, neon glow, fake metrics, fake terminal windows, fake logos/testimonials, generic three-card feature rows, arbitrary numbered section labels, excessive rounded cards, decorative charts, animated blobs, gradient text, AI-marketing phrasing, external font dependency by default, or custom client-side router.

## Chronicle Visual Vocabulary

Capture stream; socket endpoint; event dot; append line; segmented WAL; commit marker; recovery boundary; ETL checkpoint; session bracket; canonical artifact; local filesystem; loopback target; explicit effect gate; replay verification; denied operation; `rec_<uuid>`; `latest`.

## Surface Modes

- Marketing landing: **Persuade**, but grounded in the actual pipeline and command surface.
- Documentation: **Read**, with conventional Starlight ergonomics and restrained Chronicle-specific styling.
- Versioned/localized docs: same reading system, with visible archived/current context and locale/version controls.
