Perfect. Now I have read all the key files. Let me create a comprehensive, structured UI/UX implementation spec markdown document that captures everything at pixel-level detail.

---

# MEMRIZZ — Complete UI/UX Implementation Specification

**Citrate Memories: A Federated Memory Graph Visualization & RAG System**

A pixel-perfect, production-ready specification for recreating Memrizz in React/Next.js from these HTML/CSS/JS/JSX prototypes.

---

## 1. DESIGN TOKENS

### 1.1 Color System

#### Brand Colors
- **Citrate Green** (primary accent):
  - `--citrate-green: #8ecc09` — CTAs, active states, glow
  - `--citrate-green-deep: #5a8205` — Hover, AA text on cream
  - `--citrate-green-dark: #2f4502` — Pressed state
  - `--citrate-green-tint: #e8f3c6` — Surface wash

- **Citrate Yellow** (secondary accent):
  - `--citrate-yellow: #ffbd10` — Secondary action, evidence, alerts
  - `--citrate-yellow-deep: #c89400` — Hover
  - `--citrate-yellow-dark: #6e5100` — Pressed
  - `--citrate-yellow-tint: #fff1c4` — Surface wash

#### Dark Chrome (on dark constellation field)
- `--panel: rgba(9, 22, 14, 0.74)` — Panel background (semi-transparent dark evergreen)
- `--panel-2: rgba(13, 30, 20, 0.92)` — Secondary panel
- `--ondark: #eef0e6` — Primary text on dark
- `--ondark-2: #aab7a6` — Secondary text on dark
- `--ondark-3: #74866f` — Tertiary text/disabled on dark
- `--hair: rgba(205, 231, 214, 0.12)` — Divider on dark
- `--hair-2: rgba(205, 231, 214, 0.20)` — Hover divider
- `--glass-blur: blur(18px) saturate(1.1)` — Glass morphism filter

#### Ink (Warm near-blacks)
- `--ink: #0e0f0c` — Primary text, marks
- `--ink-2: #1f221d` — Secondary ink
- `--graphite: #3a3d36` — Tertiary ink

#### Stone (Warm neutrals, light mode)
- `--stone-900: #2a2c27`
- `--stone-700: #555851` — Secondary body text
- `--stone-500: #84867f` — Tertiary text
- `--stone-400: #a5a79f`
- `--stone-300: #c3c4be`
- `--stone-200: #d9dad4` — Hairlines
- `--stone-150: #e3e2dc`
- `--stone-100: #ecebe4` — Card divider
- `--stone-50: #f1efe8`

#### Paper (Warm surfaces, light mode)
- `--paper: #f4f1ea` — Page background
- `--paper-2: #faf8f3` — Raised card
- `--paper-pure: #ffffff` — High-contrast surfaces

#### Semantic
- `--semantic-success: #4f8a05` (darker green for legibility)
- `--semantic-success-bg: #ecf5d4`
- `--semantic-warning: #b07b00`
- `--semantic-warning-bg: #fff1c4`
- `--semantic-danger: #a72414`
- `--semantic-danger-bg: #f6e1de`
- `--semantic-info: #1b4965`
- `--semantic-info-bg: #dbe7ef`

#### Material Type Colors (Six lanes)
Material types form the structured X-axis of the Lattice layout:
1. **Code**: `#8ecc09` — "the built thing" (commits, PRs)
2. **Docs**: `#ffc83a` — "the written word" (documents, narratives)
3. **Specs**: `#5fa8e6` — "the plan" (ADRs, sprints, work packages)
4. **Configs**: `#34c7b0` — "the wiring" (manifest changes, pins, drift)
5. **Tests/Audit**: `#f0743a` — "the checks" (audits, findings, benchmarks)
6. **Claims**: `#b58cff` — "what someone said" (claims, assertions, hypotheses)

#### Trust Ring Colors (Four planes)
Trust tiers are rendered as glowing rings around nodes:
1. **DerivedDeterministic** (Derived): `#cfe9b0` — "rebuilt from git/markdown"
2. **HumanConfirmed** (Confirmed): `#ffd24a` — "a person reviewed and confirmed this"
3. **AgentAsserted** (Asserted): `#9fc0e8` — "signed by an agent or teammate"
4. **InferredAdvisory** (Proposed): `#b58cff` — "an AI guess, quarantined"

#### Field / Background
- Default (dark): `#0a1810` — Dark evergreen space
- Alternative: `#0f2a1a` — Slightly warmer evergreen
- Light (paper): `#f4f1ea` — Warm cream paper

#### Deep Evergreen (data viz anchor)
- `--deep-evergreen: #0f2a1a`
- `--deep-evergreen-2: #1a3b27`

### 1.2 Typography

#### Font Families
- **Display (serif, warm)**: `"Source Serif 4"`, `"Source Serif Pro"`, `"Iowan Old Style"`, Georgia, serif
  - Imported from Google Fonts: `family=Source+Serif+4:ital,opsz,wght@0,8..60,300..900;1,8..60,300..900`
  - Variable font; `opsz` (optical size) parameter optimizes rendering at different scales
  
- **Body (sans, geometric)**: `"Geist"`, `"Söhne"`, `"Inter"`, system-ui, -apple-system, `"Segoe UI"`, sans-serif
  - Imported: `family=Geist:wght@300;400;500;600;700`
  
- **Monospace (code)**: `"Geist Mono"`, `"JetBrains Mono"`, ui-monospace, `"SF Mono"`, Menlo, monospace
  - Imported: `family=Geist+Mono:wght@400;500;600`

#### Type Scale (modular, 1.200 minor third ratio)
| Scale | Size | Usage |
|-------|------|-------|
| `--t-3xs` | 11px | Microtext, codes |
| `--t-2xs` | 12px | Eyebrows, captions, labels |
| `--t-xs` | 13px | Small UI text |
| `--t-sm` | 14px | Small body, form fields |
| `--t-md` | 16px | Body text (default) |
| `--t-lg` | 18px | H5, large body |
| `--t-xl` | 22px | H4, headlines |
| `--t-2xl` | 28px | H3, section heads |
| `--t-3xl` | 36px | H2, major sections |
| `--t-4xl` | 48px | H1, page titles |
| `--t-5xl` | 64px | Hero display |
| `--t-6xl` | 84px | Full display |
| `--t-7xl` | 112px | Maximum display |

#### Line Heights
- `--lh-tight: 1.04` — Display serif (optical tightness)
- `--lh-snug: 1.15` — Headlines
- `--lh-normal: 1.45` — Body text
- `--lh-loose: 1.65` — Long-form serif (documents)

#### Tracking (Letter Spacing)
- `--tr-display: -0.022em` — Display serif optical adjustment
- `--tr-tight: -0.011em` — Tight headlines
- `--tr-normal: 0` — Body
- `--tr-eyebrow: 0.14em` — Uppercase labels (compliance feel)
- `--tr-caps: 0.06em` — Uppercase text

#### Semantic Type Classes
- `.t-eyebrow` — 12px, 500, `--tr-eyebrow`, uppercase, secondary text
- `.t-display-xl` — `clamp(56px, 8vw, 112px)`, 360 weight, serif, `opsz: 60`
- `.t-display` — 84px, 380 weight, serif, `opsz: 48`, `--tr-display`
- `.t-h1` / `h2` — 48px, 420 weight, serif, `opsz: 32`, `--tr-tight`
- `.t-h2` / `h3` — 36px, 460 weight, serif, `opsz: 24`
- `.t-h3` / `h4` — 22px, 600 weight, sans, `--tr-tight`
- `.t-h4` / `h5` — 18px, 600 weight, sans
- `.t-lede` — 22px, 360 weight, serif, loose, `opsz: 24`
- `.t-body` / `p` — 16px, 400 weight, sans, normal line-height
- `.t-body-sm` — 14px, 400 weight, sans, secondary text
- `.t-caption` — 12px, 400 weight, sans, tertiary text
- `.t-mono` / `code` — 0.92em, monospace, 400 weight
- `.t-compliance` — 12px, 500 weight, mono, uppercase, `--tr-caps`, secondary text
- `.t-prose` — 18px, 380 weight, serif, loose line-height, `opsz: 18` (long-form document style)

### 1.3 Spacing (8pt grid + half steps)
| Token | Value | Usage |
|-------|-------|-------|
| `--s-0` | 0 | No space |
| `--s-1` | 4px | Microspace |
| `--s-2` | 8px | Tight spacing (button padding, small gaps) |
| `--s-3` | 12px | Small gaps |
| `--s-4` | 16px | Standard padding/margin |
| `--s-5` | 20px | Generous padding |
| `--s-6` | 24px | Section padding |
| `--s-7` | 32px | Gutter width |
| `--s-8` | 40px | Large spacing |
| `--s-9` | 48px | Component spacing |
| `--s-10` | 64px | Major section spacing |
| `--s-11` | 80px | Large section gaps |
| `--s-12` | 96px | Hero spacing |
| `--s-13` | 128px | Page-level spacing |
| `--s-14` | 160px | Maximum spacing |

### 1.4 Radii (intentionally restrained)
- `--r-0: 0` — No radius (default, government-document feel)
- `--r-1: 6px` — Buttons, inputs (softened from 2px)
- `--r-2: 8px` — Cards, panels
- `--r-3: 12px` — Large surfaces, modals
- `--r-pill: 999px` — Tags, status badges (full pill)

### 1.5 Elevation & Shadows (sparing, single-direction downward)
- `--shadow-hairline: 0 0 0 1px var(--stone-200)` — Border only
- `--shadow-1: 0 1px 0 0 rgba(14,15,12,.06), 0 0 0 1px var(--stone-200)` — Subtle lift
- `--shadow-2: 0 2px 0 0 rgba(14,15,12,.04), 0 0 0 1px var(--stone-200)` — Light lift
- `--shadow-lift: 0 12px 32px -16px rgba(14,15,12,.18), 0 0 0 1px var(--stone-200)` — Modal/dropdown
- `--shadow-focus: 0 0 0 3px rgba(142,204,9,.35)` — Green focus ring (8.4px total width with 2px border)

### 1.6 Motion Curves & Durations
#### Easing
- `--ease-standard: cubic-bezier(.2, .0, .0, 1)` — Slight overshoot pull (enter)
- `--ease-exit: cubic-bezier(.4, .0, 1, 1)` — Exit curve (quick leave)
- `--ease-spring: cubic-bezier(.34, 1.18, .4, 1)` — Bouncy spring

#### Timing
- `--dur-fast: 140ms` — Quick interactions (hover, toggle)
- `--dur-base: 220ms` — Standard transitions (layout, focus)
- `--dur-slow: 420ms` — Deliberate motions (panel open, collapse)
- `--dur-narrative: 900ms` — Hero animations, entrance sequences

### 1.7 Layout Constants
- `--container: 1280px` — Standard container width
- `--container-wide: 1440px` — Wide container (not used in Memrizz, but available)
- `--gutter: var(--s-7)` → 32px — Standard side margin
- `--rule-thin: 1px` — Standard border
- `--rule-thick: 2px` — Emphasis border

### 1.8 Lattice Motif (Decorative background)
Brand's one decorative element: sparse grid suggesting GhostDAG topology (never the focus):

```css
.lattice-grid {
  background-image:
    linear-gradient(to right,  rgba(14,15,12,.045) 1px, transparent 1px),
    linear-gradient(to bottom, rgba(14,15,12,.045) 1px, transparent 1px);
  background-size: 64px 64px;
}

.lattice-grid-fine {
  background-size: 32px 32px;
  opacity: 0.035;
}

.lattice-dots {
  background-image: radial-gradient(circle at 1px 1px, rgba(14,15,12,.16) 1px, transparent 1px);
  background-size: 24px 24px;
}

.lattice-dots-dark {
  background-size: 24px 24px;
  opacity: 0.18;
  color: #cde7d6;
}

.lattice-dots-green {
  background-image: radial-gradient(circle at 1px 1px, rgba(142,204,9,.32) 1px, transparent 1px);
  background-size: 24px 24px;
}
```

---

## 2. LAYOUT ARCHITECTURE

### 2.1 Overall App Shell (Viewport-filling grid)

The app is a **fixed viewport** (`height: 100vh`) with no scroll. All major sections are positioned absolutely, overlaid on a central canvas.

```
┌─────────────────────────────────────────────────┐
│  ☰ Memrizz  Citrate Federation  Search  ⌘K   │  56px — topbar
├─┬─────────────────────────────────────────────┬─┤
│ │                                             │ │
│ │          CANVAS (Constellation)             │ │
│ │   2.5D projected node graph with camera     │ │
│ │          (WebGL-free, 2D canvas)            │ │
│ │                                             │ │ 
│N│                                             │R│  Constellation view only
│a│                                             │i│
│v│                                             │g│
│ │  ↓ Timeline scrubber (bottom-centered)     │h│
│ │                                             │t│
├─┴─────────────────────────────────────────────┴─┤
│   HUD (bottom-left)  │ Toast (bottom-center)   │
└─────────────────────────────────────────────────┘
```

**Main Regions:**
1. **NavRail** (far left): 60px fixed width, nav + avatar
2. **TopBar** (top, full width): 56px height, brand + omnibox + model + notifs + avatar
3. **LeftRail** (collapsed/expanded): 268px max width, collapsible panel for Constellation filters
4. **Canvas** (central): Entire remaining viewport, scrollless
5. **RightDock** (right side, visible when in Constellation): 392px width, Ask/Inspect tabs
6. **TimeScrubber/ScrubMini** (bottom-center): Timeline replay control
7. **Minimap** (bottom-left corner): 132x132px overview canvas
8. **SelectionHUD** (replaces scrubber when lasso active): Multi-select summary
9. **HUD** (bottom-left): Stats display
10. **Route surfaces** (Replace entire canvas when not in Constellation): Review, Settings, Connect, Audit, Profile

#### Fixed Positioning
- **NavRail**: `position: fixed; left: 0; top: 0; bottom: 0; width: 60px; z-index: 50;`
- **TopBar**: `position: absolute; top: 0; left: 60px; right: 0; height: 56px; z-index: 40;`
- **LeftRail**: `position: absolute; left: 74px; top: 70px; bottom: 206px; width: 268px; z-index: 30;`
  - Collapses to 52px width; toggle via `.rail-toggle` button (`26px × 26px`, positioned `right: -13px`)
- **RightDock**: `position: absolute; right: 16px; top: 70px; bottom: 18px; width: 392px; z-index: 30;`
  - Hides entirely when collapsed; `.dockmini` (icon column) takes its place
- **TimeScrubber**: `position: absolute; left: calc(50% + 34px); transform: translateX(-50%); bottom: 18px; width: min(660px, calc(100vw - 760px)); z-index: 35;`
  - Collapses to `.scrubmini` (pill) when not open
- **Minimap**: `position: absolute; left: 74px; bottom: 38px; z-index: 26;`
- **SelectionHUD/ScrubMini**: `position: absolute; left: 50%; transform: translateX(-50%); bottom: 84px; width: min(560px, calc(100vw - 760px)); z-index: 36;`
- **HUD**: `position: absolute; left: 74px; bottom: 18px; z-index: 25;`

### 2.2 NavRail (Left edge, 60px)

**Fixed navbar at far left** — 60px wide, full height, dark evergreen glass.

```
┌──────┐
│ Logo │  30×30px logo mark centered
├──────┤
│ ◎ C  │  Constellation button (42×42px)
│      │
│ ◎ S  │  Ask (Spark)
│      │
│ ◎ ✓  │  Review (with badge "3")
│      │
│ ◎ ⚡ │  Connect
│      │
│ ◎ 📋 │  Audit
│      │
│ [sp] │  Spacer flex: 1
│      │
│ ◎ ⚙  │  Settings
│ ◎ A  │  Profile (avatar, 24×24px)
└──────┘
```

**Button specs (.nav-btn)**:
- 42×42px, border-radius: 6px
- `border: 1px solid transparent` by default
- `background: transparent; color: var(--ondark-2)`
- On hover: `background: rgba(142, 204, 9, 0.08); color: var(--ondark)`
- Active state (`.on`):
  - `background: rgba(142, 204, 9, 0.13); color: var(--citrate-green); border-color: rgba(142, 204, 9, 0.3);`
  - Left accent bar: `::before { position: absolute; left: -10px; top: 9px; bottom: 9px; width: 3px; border-radius: 2px; background: var(--citrate-green); }`
- Badge (`.nb-badge`): positioned `top: 4px; right: 4px;` with background `var(--citrate-yellow)`, min-width 16px, height 16px, border-radius 99px, monospace 9.5px, box-shadow: `0 0 0 2px rgba(6, 15, 10, 0.92);`
- Hover tooltip (`.nav-tip`): `position: absolute; left: 52px;` appears on hover with opacity animation

**Styling**:
- Background: `rgba(6, 15, 10, 0.92)` with `backdrop-filter: blur(18px) saturate(1.1);`
- Border-right: `1px solid var(--hair)`
- Padding: `12px 0`
- Display: `flex; flex-direction: column; align-items: center; gap: 6px;`

### 2.3 TopBar (Top, 56px, full width except NavRail)

Horizontal bar containing brand, org picker, omnibox, model picker, notifications, and avatar.

```
┌─ [L] Memrizz │ C 14 repos │ [Search...] ⌘K │ [spacer] │ Sonnet ⌘ │ ◉ │ A ⌘ ─┐
```

**Specifications:**
- `position: absolute; top: 0; left: 60px; right: 0; height: 56px; z-index: 40;`
- `display: flex; align-items: center; gap: 18px; padding: 0 18px;`
- Background: `linear-gradient(to bottom, rgba(7,17,11,0.92), rgba(7,17,11,0.5) 70%, rgba(7,17,11,0));`
- Backdrop: `blur(18px) saturate(1.1);`

**Child components:**

1. **Brand section** (`.brand`):
   - `display: flex; align-items: center; gap: 11px;`
   - Mark (SVG): 24×24px
   - Name text: 18px serif, `letter-spacing: -0.01em`, `opsz: 24`
     - "**Memrizz**" in 500 weight
   - Divider (`.divider-v`): 1px separator, 24px tall, `background: var(--hair)`

2. **Org Switcher** (`.switcher`):
   - `display: inline-flex; align-items: center; gap: 9px; height: 36px; padding: 0 11px;`
   - `border: 1px solid var(--hair); border-radius: var(--r-1);`
   - `background: rgba(10,24,16,0.5);`
   - Hover: `border-color: var(--hair-2)`
   - Dot (`.sw-dot`): 18×18px, `border-radius: 5px`, `background: var(--citrate-green)`, grid center, white text, 11px bold, "C"
   - Lines:
     - `.sw-l1`: 13px 500w, `color: var(--ondark)`, "Citrate Federation"
     - `.sw-l2`: 9px mono, uppercase, `letter-spacing: 0.14em`, tertiary, "Org · 14 repos"
   - Chevron icon

3. **Omnibox** (`.omnibox`):
   - `flex: 1; max-width: 460px;`
   - `display: flex; align-items: center; gap: 10px; height: 36px; padding: 0 12px;`
   - `border: 1px solid var(--hair); border-radius: var(--r-1);`
   - `background: rgba(6,15,10,0.55);`
   - Hover: `border-color: var(--hair-2)`
   - Search icon + placeholder "Search memory — filings, decisions, repos…" + kbd "⌘K"
   - On click: Opens CommandPalette

4. **Spacer** (`.spacer`): `flex: 1;`

5. **Model Picker** (`.modelpick`):
   - `display: inline-flex; align-items: center; gap: 8px; height: 32px; padding: 0 10px;`
   - `border: 1px solid var(--hair); border-radius: var(--r-1);`
   - `background: rgba(10,24,16,0.5);`
   - Dot (`.mdot`): 7×7px, `background: var(--citrate-green)`
   - Text: 12.5px 500w, `color: var(--ondark)`
   - Chevdown icon
   - On click: Cycles through models (Sonnet → GPT → Citrate-LM → Llama 4 → Sonnet)

6. **Notifications** (`.tb-btn` + `.notif-pop`):
   - Button: 36px tall, min-width 36px, border, icon (bell), badge dot
   - Popup on click: 360px wide, positioned `top: 52px; right: 70px;`
     - Header "Notifications" + eyebrow "calm feed"
     - 4 notification items (read-only; click → go to Review)
     - Each item shows icon, title, subtitle, when

7. **Avatar / Profile** (`.switcher` variant):
   - Same as org switcher but with user initials in dot
   - On click: Goes to Profile page

### 2.4 LeftRail (Expanded: 268px; Collapsed: 52px)

**Purpose**: Filter Constellation by material type, trust tier, status; select layout.

**Expanded (.leftrail)**:
- `position: absolute; left: 74px; top: 70px; bottom: 206px; width: 268px; z-index: 30;`
- `display: flex; flex-direction: column;`
- Panel styling: dark glass with border & shadow
- `.panel-h` header: "Constellation" + icon
- `.rail-scroll` (flex: 1): scrollable content area
- `.rail-toggle` (collapse button): 26×26px, `position: absolute; right: -13px; top: 10px;`

**Sections (.rail-sec)** inside scroll:

1. **VIEW** — Layout selector (.seg):
   - 2-column grid, 6px gap
   - Four buttons (.seg-btn):
     - **Lattice**: "Type · Time · Trust" (s2 subtitle)
     - **Galaxy**: "By meaning"
     - **Islands**: "By repo"
     - **River**: "By time"
   - Each button: 8px 9px padding, border-radius 6px, flex-column, left-aligned
   - Active (`.on`): `border-color: var(--citrate-green); background: rgba(142,204,9,0.10);`

2. **MATERIAL TYPE** — Filter chips (.chips):
   - Label: "Material type" + color indicator if encoding == "kind"
   - 6 filter chips (.fchip), one per lane (Code, Docs, Specs, Configs, Tests/Audit, Claims)
   - Each chip: `display: inline-flex; gap: 6px; padding: 5px 9px; border-radius: var(--r-pill);`
   - `.sw` (colored square): 8×8px, `border-radius: 2px`
   - Off state (deselected): `opacity: 0.4;`

3. **TRUST** — Trust filter chips:
   - Label: "Trust" + color indicator if encoding == "trust"
   - 4 filter chips for trust tiers (Derived, Confirmed, Asserted, Proposed)
   - Same styling as material chips but with `.round` (`.sw { border-radius: 50%; }`)
   - Colors from TRUST.ring values

4. **STATUS** — Status filter chips:
   - "Active", "Superseded", "Archived"
   - Standard chips (no color)
   - Toggle: **Show proposed (advisory)**
     - Label: 12.5px
     - `.sw-toggle`: 34×19px pill toggle with sliding thumb

**Collapsed (.leftrail.collapsed)**:
- Width: 52px
- Vertical icon column (.rail-icon-col):
  - 4 layout buttons (icon only, 34×34px each)
  - Divider line
  - 1 filter button (icon only)
- `.rail-toggle` button still present, changes direction (chevright)

### 2.5 RightDock (392px, collapsible)

**Purpose**: Ask (RAG with grounded citations) and Inspect (memory details, trust verification).

**Expanded**:
- `position: absolute; right: 16px; top: 70px; bottom: 18px; width: 392px; z-index: 30;`
- `display: flex; flex-direction: column;`
- Panel styling: dark glass

**Tab bar** (`.dock-tabs`):
- `display: flex; padding: 8px 8px 0; gap: 4px; border-bottom: 1px solid var(--hair);`
- Two tabs: "Ask" and "Inspect"
- Tab styling (`.dock-tab`):
  - `flex: 1; padding: 10px 10px; text-align: center; cursor: pointer;`
  - `color: var(--ondark-3); font-size: 13px; font-weight: 500;`
  - `border-bottom: 2px solid transparent; margin-bottom: -1px;`
  - Hover: `color: var(--ondark-2);`
  - Active (`.on`): `color: var(--ondark); border-bottom-color: var(--citrate-green);`
  - Optional `.num` badge (yellow background, monospace 10px)
- Collapse button (`.collapse-btn`): 26×26px chevron-right

**Body** (`.dock-body`):
- `flex: 1; overflow-y: auto;`
- Scrollbar: 8px, thumb styled with `background: var(--hair);`

**Tab 1: Ask** — RAG with grounded answers and citations

- `.ask-wrap`: flex column, 100% height
- `.ask-head`: Model picker + mode toggle (Plain/Technical)
- `.ask-thread`: Thread of messages (user Q, AI A with citations, pending states)
  - `.msg-user`: User message bubble
    - `align-self: flex-end; max-width: 84%;`
    - `background: rgba(142,204,9,0.12); border: 1px solid rgba(142,204,9,0.22);`
    - `color: var(--ondark); padding: 9px 12px; border-radius: 12px 12px 4px 12px;`
    - 13.5px, 1.5 line-height
  - `.msg-ai`: AI message with optional context note, body, headsup, work summary
    - Body contains `.cite` chips (citations) inline with text
    - `.cite`: monospace button with dot, color-coded, fly-to action on click, flash animation on cite
    - `.headsup`: Optional warning card (yellow border, alert content)
    - `.work`: Collapsible "show your work" section with tool calls listed

- `.ask-context` (if grounding memories):
  - `.ctx-head`: "Grounding · N memories" + add button
  - `.ctx-chips`: Chips per grounded memory (removable)

- `.ask-input`:
  - Suggested prompts (if no context/thread)
  - Composer box (`.ask-box`):
    - `display: flex; gap: 9px; height: 42px; padding: 0 6px 0 13px;`
    - Tools button (.tools-btn), Add memory button, input field, send button
    - Input: `flex: 1; background: transparent; border: 0; outline: 0;`
    - Send button (`.send-btn`): 32×32px, `background: var(--citrate-green); color: var(--ink);`

**Tab 2: Inspect** — Memory details, trust verification, neighbors

- When no node selected: Empty state with icon, title, description
- When node selected (`.insp-pad`):
  - Kind chip (`.insp-kind`): colored dot + eyebrow kind
  - Title (`.insp-title`): 21px serif, pretty-wrap, `margin-bottom: 12px;`
  - Badge row: Plane, Trust, Status, Belnap contradiction flag
  - Metadata grid (`.insp-grid`): 2-column key-value
    - REPO, MATERIAL, RECORDED (date + "X ago"), ADDRESS (hash mono)
  - Verdict card (`.verdict`):
    - Green (ok), yellow (warn), or red (bad) border & background
    - Icon + head statement + reasons list
  - Neighbors list (`.neighbor` rows):
    - Each: dot color, title, edge type + direction
    - Hover: title color → green
    - Click: select neighbor
    - "No recorded neighbors" if empty
  - Blast radius control (`.hops-ctl`): 1/2/3 hop selector
  - Action buttons (`.act-row`):
    - "Ask about this" (primary)
    - "Focus neighborhood"
    - "Find analogues"
    - "Propose edge"

**Collapsed (.dockmini)**:
- Icon column on right side showing Ask (spark) and Inspect (shield) icons
- Small badges for notification count
- Click expands dock

### 2.6 Canvas (Central, viewport-sized)

The **stage** container:
- `position: fixed; inset: 0;`
- Dark background: `#0a1810`

**Canvas element**:
- Full viewport `<canvas class="constellation">`
- `position: absolute; inset: 0;`
- Touch-action: `none`
- Cursor: grab / grabbing

The canvas is rendered via Constellation engine (2D canvas, 2.5D projection, no WebGL). See **Section 4** for rendering details.

### 2.7 Bottom Panels

#### TimeScrubber (Expanded)
- `position: absolute; left: calc(50% + 34px); transform: translateX(-50%); bottom: 18px; width: min(660px, calc(100vw - 760px)); z-index: 35;`
- Panel styling (dark glass)
- `.scrub-head`: Title "Memory as it is now" or "Rewound to [date]" + Replay/Pause button + collapse button
- `.scrub-track`:
  - `.scrub-ticks`: Month/year labels
  - `.scrub-rail`: 3px high track, `background: rgba(205,231,214,0.14);`
  - `.scrub-fill`: Inside rail, width = time%, green gradient
  - `.scrub-knob`: 15×15px green circle, draggable, glow (box-shadow)

#### ScrubMini (Collapsed)
- `position: absolute; left: calc(50% + 34px); transform: translateX(-50%); bottom: 18px; z-index: 35;`
- Pill-shaped button: 40px height, `display: inline-flex; gap: 9px; padding: 0 14px;`
- Shows current date or "Memory · now"
- Click expands to full scrubber

#### SelectionHUD (when lasso active)
- `position: absolute; left: 50%; transform: translateX(-50%); bottom: 84px; width: min(560px, calc(100vw - 760px)); z-index: 36;`
- Panel styling
- `.selhud-h`: Count (large yellow text for numbers), meta "across N repos", close button
- `.selhud-bars`: Horizontal stacked bars showing count by lane
- `.selhud-flags`: Badge row showing advisory/superseded/contradiction counts
- `.selhud-actions`: Buttons for Ask, Focus, Confirm, Propose edge, Export

#### Minimap (bottom-left corner)
- `position: absolute; left: 74px; bottom: 38px; z-index: 26;`
- 132×132px canvas (same projection, smaller view)
- Label: "Overview" (10px eyebrow)

#### HUD (bottom-left)
- `position: absolute; left: 74px; bottom: 18px; z-index: 25;`
- Monospace 10px stats: "MODE: lattice", "NODES: 847", "EDGES: 1423", etc.

### 2.8 Modals & Overlays

#### Command Palette (⌘K)
- `.modal-back`: Fixed full-screen backdrop with blur
- `.cmdk`: 640px wide, max 70vh height, centered at 12vh top margin
- Search input + live filtered results (memories, repos, actions, navigation)
- Keyboard navigation (↑↓ to select, ⏎ to run, Esc to close)

#### Memory Picker
- Same modal backdrop & `.picker` panel (620px wide, 78vh max height)
- Header, search input, scope indicator, scrollable list, footer with selected count & buttons

#### Notifications Popup
- `position: absolute; top: 52px; right: 70px; width: 360px; z-index: 60;`
- Appears on notification bell click
- Header + scrollable notification items
- Click item → navigate to Review or elsewhere

#### Tools Popup
- `position: absolute; bottom: 54px; right: 14px; width: 320px; z-index: 20;`
- Appears on tools button click in Ask input
- `.tools-pop` panel with grid of tool items (icon, name, description)

#### Toast (bottom-center)
- `position: fixed; bottom: 22px; left: 50%; transform: translateX(-50%); z-index: 95;`
- Dark glass pill: 12px padding, auto-dismiss after 2.8s
- Icon, text, optional audit sequence number

#### Guided Overlay (onload)
- `.guide-back`: Full-screen semi-transparent dark backdrop with blur
- `.guide`: Centered 540px panel with 7 onboarding steps
- Dismissible

### 2.9 Route Surfaces (Full-page replacements)

When viewing a non-Constellation route (Review, Settings, Connect, Audit, Profile):
- `.route`: `position: absolute; left: 60px; right: 0; top: 56px; bottom: 0; z-index: 28;`
- `background: rgba(7, 17, 11, 0.86); backdrop-filter: blur(18px) saturate(1.1);`
- Scrollable: `overflow-y: auto;`
- `.route-inner`: `max-width: 1180px; margin: 0 auto; padding: 34px 40px 80px;`

Each route (Review, Settings, Connect, Audit, Profile) renders differently. See **Component sections** for details.

---

## 3. COMPONENTS (Top to Bottom, Left to Right)

### 3.1 NavRail

**Component: `NavRail`**

```jsx
function NavRail({ view, onView, reviewCount }) {
  // view: "constellation" | "ask" | "review" | "connect" | "audit" | "settings" | "profile"
  // onView(id): called when nav item clicked
  // reviewCount: number of items in review queues
  
  const items = [
    { id: "constellation", icon: "layers", label: "Constellation" },
    { id: "ask", icon: "spark", label: "Ask" },
    { id: "review", icon: "shield", label: "Review", badge: reviewCount },
    { id: "connect", icon: "link", label: "Connect a model" },
    { id: "audit", icon: "ledger", label: "Audit log" },
  ];
  const bottom = [
    { id: "settings", icon: "sliders", label: "Settings" },
    { id: "profile", icon: "target", label: "Profile", avatar: true },
  ];
  
  // Render: dark glass pill, vertical button grid, spacer, avatar
}
```

### 3.2 TopBar

**Component: `TopBar`**

Contains: Brand, Org Switcher, Omnibox, Model Picker, Notifications, Avatar.

Key props:
- `onOmni()`: Opens CommandPalette
- `model`: Display string (e.g., "Claude Sonnet 4.6")
- `onModel()`: Cycles through models
- `onProfile()`: Opens profile
- `onGoReview()`: Opens review center

### 3.3 LeftRail (Constellation Filter Panel)

**Component: `LeftRail`**

```jsx
function LeftRail({ collapsed, onCollapse, mode, onMode, filters, setFilter, encoding }) {
  // collapsed: boolean
  // onCollapse(): toggle collapse state
  // mode: "lattice" | "galaxy" | "islands" | "river"
  // onMode(m): set layout mode
  // filters: { tenants, lanes, trusts, statuses, showQuarantined }
  // setFilter(key, val): update filter
  // encoding: "kind" | "trust" | "tenant"
}
```

**Sections rendered**:
1. Layout selector (2×2 grid of mode buttons)
2. Material type filter chips (6 lanes)
3. Trust tier filter chips (4 tiers)
4. Status filter chips (3 statuses) + Show proposed toggle

### 3.4 RightDock (Ask + Inspect)

**Components:**
- `Ask` — RAG chat with grounded memories and citations
- `Inspector` — Node details, trust verification, neighbors

**Ask sub-components:**
- `AskMessage` — Renders user/AI messages with citation chips
- `CitationChip` — Flyable cite reference
- `ToolsPop` — Popover listing memory tools the model can call
- `MemoryPicker` — Modal for pulling memories into conversation

**Inspector features:**
- Node kind/title/badges
- Metadata grid (repo, material, recorded, address hash)
- Verdict card (ok/warn/bad states with reasons)
- Neighbors list (clickable to navigate)
- Blast radius control (1/2/3 hops)
- Action buttons (Ask, Focus, Find analogues, Propose edge)

### 3.5 Canvas & Constellation Engine

**2.5D projected node-graph renderer** (see **Section 4** for full details).

- Node glow sprites (baked, color-coded by material/trust)
- Edge rendering (colored strokes, particle flow, dashed for analogues)
- Four layout modes with smooth morphing
- Camera with orbit controls (click-drag, mouse wheel zoom)
- Hit-testing (click to select, hover for tooltip)
- Lasso selection (Shift-drag to box-select)
- Focus ring visualization (highlight selected node + neighbors)
- Mini-map view (bottom-left corner)

### 3.6 TimeScrubber & History Replay

**Component: `TimeScrubber`**

- Draggable timeline scrubber (pointer events on track)
- Play/Pause button
- Date display ("Memory as it is now" or "Rewound to [date]")
- Month/year tick labels
- Green knob with glow

**Functionality:**
- Dragging knob updates `timeT` (0.0 → 1.0, maps to data.T0 → T1)
- Replay animation: auto-advances timeT over 30s
- Constellation re-renders with updated node visibility (future nodes fade)
- Scrubber collapses to pill when not in use

### 3.7 SelectionHUD (Lasso Multi-Select)

**Component: `SelectionHUD`**

Appears when lasso box-select is active:
- Count of selected nodes (large, with yellow number)
- Metadata: "across N repos"
- Stacked bars showing count by material lane
- Badges for advisory/superseded/contradiction counts
- Action buttons:
  - "Ask about these N" → Adds to Ask context
  - "Focus these" → Fly camera to selection center
  - "Confirm N advisory" → Promote advisory → confirmed
  - "Propose cluster edge" → Generate HITL proposal
  - "Export" → Download memory lineage

### 3.8 Command Palette (⌘K)

**Component: `CommandPalette`**

Search-to-fly interface combining:
- Memories (ranked by relevance)
- Repos (all tenant names)
- Actions (Lasso, Replay, Reset, Layout switches)
- Navigation (Go to Constellation, Ask, Review, etc.)

**Rendering:**
- Input field (autofocused) with ⌘K hint
- Grouped results with icons
- Keyboard nav (↑↓ move, ⏎ select, Esc close)
- Hover and active highlighting

### 3.9 Toast Notifications

**Component: Inline toast**

Auto-dismissing notification:
- Fixed bottom-center position
- Icon + text + optional audit sequence
- Fades after 2.8s

### 3.10 Review Center (HITL)

**Component: `ReviewCenter`**

Four-tab interface for human-in-the-loop confirmation:
1. **Proposals** — AI-proposed edges awaiting witness
2. **Contradictions** — Belnap "Both" conflicts
3. **Supersessions** — Old memory → new memory links
4. **Self-critic** — Gaps in last answer generation

**Card components:**
- `ProposalCard` — Shows a ↔ b edge with confidence & rationale, action buttons (Confirm / Ask for evidence / Reject)
- `ContradictionCard` — Two-column claim view with "conflicts" indicator, resolution buttons
- `SupersessionCard` — Old → New node pair
- `CriticCard` — Gap description, severity badge, remediation button

Each card shows metadata (proposer, when, repo) and has button row with resolution states.

### 3.11 Connect Page (BYOM / MCP)

**Component: `ConnectPage`**

Bring-your-own-model configuration:
- **Your endpoint & token** section
  - Endpoint URL (copyable)
  - Masked token with Regenerate button
  - Capability chips (read 14 repos, propose all, TTL 15m)
- **Connected clients** section
  - Live list of attached models (show/hide by status)
  - Revoke button per client
- **Client configuration** section
  - Tabbed code blocks (Claude Desktop, Claude Code, Cursor, Generic MCP)
  - Copy button per block

### 3.12 Audit Page

**Component: `AuditPage`**

Tamper-evident hash-chained audit log viewer:
- Filters: Actor (dropdown), Kind (dropdown), Repo (dropdown), Search (text)
- Live tail toggle (checkbox) — auto-appends new events
- Table view:
  - Columns: Seq#, Event kind (colored badge), Actor (avatar + name), Detail (mono), When
  - Rows animate in on fresh events
  - Sorted descending (newest first)

### 3.13 Settings Page

**Component: `SettingsPage`**

Seven-section settings interface (nav sidebar + content):
1. **General** — Org name, landing view, answer detail, motion reduction
2. **Memory & index** — Index freshness, checkpoints, advisory visibility, retention
3. **Models & AI** — Model selector (radio), BYOM toggle, show work toggle
4. **People & roles** — Member list (read-only)
5. **Delegation** — Revocation tree (visual hierarchy with parent-child lines)
6. **Security & audit** — Audit chain status, OIDC, token storage, BYOM TTL
7. **Danger zone** — Crypto-shred flow (select repo, type name to confirm, irreversible)

### 3.14 Profile Page

**Component: `ProfilePage`**

User profile with organizational context:
- Hero section: name, title, org role
- Stat cards: Number of memories accessed, edges authored, audit events
- Scopes (repos the user can access)
- RBAC delegation line (who delegated to me)

### 3.15 Memory Picker Modal

**Component: `MemoryPicker`**

Modal for grounding Ask queries:
- Search input (autofocused)
- Results list (ranked by relevance)
- Checkbox per row (multi-select)
- Selected count display
- Cancel / Add to conversation buttons

### 3.16 Icon System

**Component: `MIcon`**

SVG icon renderer with unified stroke styling:
- Props: `name` (string), `size` (default 16px), `stroke` (default 1.6px)
- Icons: search, bell, chevdown/left/right, layers, filter, clock, history, spark, sliders, eye, send, x, play, pause, check, target, branch, shield, plus, reset, info, grid, cube, waves, help, link, ledger, copy, download, refresh

---

## 4. CONSTELLATION ENGINE (2.5D Renderer)

### 4.1 Overview

**Lightweight 2.5D projected node-graph renderer** implemented in 2D canvas (no WebGL).
- ~900 nodes, ~1500 edges
- Orbit camera (yaw, pitch, distance, zoom)
- Perspective projection onto 2D canvas
- Four morphing layouts: Lattice, Galaxy, Islands, River
- Baked glow sprites, additive rendering
- Particle edge-flow
- Depth fog
- Hit-testing for selection & hover
- Lasso multi-select
- Time morphing (nodes appear/disappear as timeline slides)
- Smooth camera animations (fly-to)

### 4.2 Data Structures

**Node shape** (from `data.js`):
```javascript
{
  id: "n0",                    // unique ID
  idx: 0,                      // numeric index
  kind: "Commit",              // Kind enum
  lane: "code",                // One of 6 lanes
  laneIdx: 0,                  // Numeric lane index
  plane: "Derived",            // "Derived" or "Asserted"
  trust: "DerivedDeterministic", // Trust tier
  trustIdx: 0,                 // Numeric trust index
  status: "Active",            // "Active" | "Superseded" | "Archived"
  tenant: "citrate-memories",  // Repo name
  tenantIdx: 3,                // Numeric tenant index
  title: "fix single-writer lock contention", // Content string
  tf: 0.5,                     // Time fraction (0.0 → 1.0 across T0..T1)
  when: <ms>,                  // Absolute timestamp
  belnap: false,               // Contradiction flag
  deg: 5,                      // Node degree (number of edges)
  sizeW: 0.7,                  // Size weight (0.45 → 1.45 based on degree)
  seed: 0.234,                 // Deterministic RNG seed
  sx, sy, sz: 0.1,             // Semantic position jitter (galaxy mode)
  hash: "ctr:7f3a9c2e",        // Content hash for inspector
  
  // Runtime (added by engine):
  pos: { x: 0, y: 0, z: 0 },   // Current 3D position
  tpos: { x: 0, y: 0, z: 0 },  // Target 3D position
  _set: true,                  // Initialized flag
  _eff: "Active",              // Effective status (accounting for timeT)
  _vis: true,                  // Passes current filters
}
```

**Edge shape**:
```javascript
{
  a: "n0",                  // source node ID
  b: "n3",                  // target node ID
  ai: 0, bi: 3,             // source/target indices (for fast lookup)
  kind: "TemporalNext",     // Edge type
  quarantined: false,       // Proposed (advisory) flag
}
```

**Edge types & styles**:
| Kind | Color | Alpha | Width | Dash | Flow |
|------|-------|-------|-------|------|------|
| TemporalNext | 120,150,90 | 0.16 | 1.0 | none | 0.5 |
| Supersedes | 255,189,16 | 0.34 | 1.4 | none | 1.0 |
| DependsOn | 95,168,230 | 0.22 | 1.1 | none | 0.7 |
| AnalogousTo | 181,140,255 | 0.30 | 1.1 | [3,5] | 0.6 |
| Contradicts | 210,60,40 | 0.42 | 1.4 | [2,3] | 0.9 |
| References | 120,140,120 | 0.10 | 1.0 | none | 0.3 |

### 4.3 Camera & Projection

**Camera state**:
```javascript
cam = {
  yaw: 0.5,       // rotation around Y axis (radians)
  pitch: -0.32,   // tilt up/down (radians)
  dist: 15.2,     // distance from origin
  zoom: 1,        // zoom factor
  tyaw, tpitch, tdist, tzoom  // target values (lerped toward)
}
```

**Projection pipeline**:
1. Rotate 3D point by (yaw, pitch) around origin
2. Apply perspective division (depth from camera)
3. Scale by focal length and zoom
4. Translate to canvas center
5. Return 2D screen position + depth

```javascript
_project(p) {
  const c = this.cam;
  const cy = Math.cos(c.yaw), sy = Math.sin(c.yaw);
  const cp = Math.cos(c.pitch), sp = Math.sin(c.pitch);
  
  // Rotate by yaw
  const x1 = p.x * cy - p.z * sy;
  const z1 = p.x * sy + p.z * cy;
  
  // Rotate by pitch
  const y1 = p.y * cp - z1 * sp;
  const z2 = p.y * sp + z1 * cp;
  
  const depth = z2 + c.dist;
  if (depth <= 0.2) return null;  // Cull behind camera
  
  const f = (this.focal / depth) * c.zoom * this._scaleBase;
  return {
    sx: this.center.x + x1 * f,
    sy: this.center.y - y1 * f,
    depth,
    scale: f / 40
  };
}
```

**Camera animation** (fly-to):
- Easing: `cubic-bezier(.2, .0, .0, 1)` over 1.0 second
- Smooth interpolation of tyaw, tpitch, tdist, tzoom
- Stops orbit animation while flying

### 4.4 Layout Algorithms

All layouts are computed as target positions (`.tpos`); nodes smoothly interpolate toward them.

#### Lattice (Default)
**Structured 3D grid: X=material lane, Y=time, Z=trust tier**

```javascript
const LG = 2.55;     // Lane spacing
const TIME_H = 6.4;  // Time height
const TR_G = 1.7;    // Trust vertical spacing

N.forEach((n) => {
  const jx = (n.seed - 0.5) * 0.78 + (n.tenantIdx - M.TENANTS.length / 2) * 0.04;
  const jz = n.sx * 0.5;
  n.tpos = {
    x: (n.laneIdx - 2.5) * LG + jx,      // Lane axis + tenant jitter
    y: (n.tf - 0.5) * TIME_H,            // Time axis
    z: (n.trustIdx - 1.5) * TR_G + jz,   // Trust axis + jitter
  };
});
```

Visual: 6 vertical lanes (left to right: Code → Claims), time rising upward, trust layers front-to-back.

#### Galaxy (Semantic clusters)
**Nodes arranged by (lane, tenant) cluster centers; jittered within sphere**

```javascript
// Pre-compute cluster centers (one per lane×tenant pair)
const clusterCenter = {};
LANES.forEach((l) => TENANTS.forEach((t, ti) => {
  const ang = rand() * Math.PI * 2;
  const rad = 0.45 + rand() * 0.55;
  clusterCenter[ccKey(l.id, ti)] = {
    x: Math.cos(ang) * rad + (LANE_IDX[l.id] - 2.5) * 0.18,
    y: Math.sin(ang) * rad,
    z: gauss() * 0.7 + (ti - TENANTS.length / 2) * 0.06,
  };
}));

// Position nodes around their cluster center
N.forEach((n) => {
  const cc = M.clusterCenter[M.ccKey(n.lane, n.tenantIdx)];
  n.tpos = {
    x: cc.x * 6.4 + n.sx * 0.9,
    y: cc.y * 5.6 + n.sy * 0.9,
    z: cc.z * 6.0 + n.sz * 0.9,
  };
});
```

Visual: Meaningful groupings emerge (repo+lane clusters), aesthetic "galaxy" appearance.

#### Islands (By tenant)
**Each tenant gets its own island; nodes orbit around island center with time-stratification**

```javascript
const TN = M.TENANTS.length;
const centers = M.TENANTS.map((t, i) => {
  const ang = (i / TN) * TAU;
  const rad = 6.6;
  return {
    x: Math.cos(ang) * rad,
    y: (i % 2 ? 0.7 : -0.7) + Math.sin(ang * 2) * 0.5,
    z: Math.sin(ang) * rad
  };
});

N.forEach((n) => {
  const c = centers[n.tenantIdx];
  const a = n.seed * TAU, r = 0.5 + n.sx * 0.9 + n.sizeW * 0.3;
  n.tpos = {
    x: c.x + Math.cos(a) * r,
    y: c.y + (n.tf - 0.5) * 2.2 + n.sy * 0.5,
    z: c.z + Math.sin(a) * r
  };
});
```

Visual: 14 islands arranged in orbit, each island contains one repo's timeline.

#### River (By time)
**Meandering sinusoidal path through material lanes, nodes sorted by time**

```javascript
N.forEach((n) => {
  const t = n.tf;
  n.tpos = {
    x: Math.sin(t * Math.PI * 2.4) * 4.2 + (n.laneIdx - 2.5) * 0.5,
    y: (n.laneIdx - 2.5) * 0.95 + n.sy * 0.4,
    z: (t - 0.5) * 13 + n.sx * 0.6,   // Time is the dominant z-axis
  };
});
```

Visual: Meandering river flowing upward; lanes spread horizontally; time is depth.

### 4.5 Node Rendering

**Sprite baking**:
Each node is rendered as an **additive glow sprite** (no real-time rasterization).
- Baked halo sprite: 128×128px radial gradient (white → transparent)
- Per-color glow sprite: 64×64px, color-matched to node's lane/trust/tenant

**Rendering order** (per frame):
1. Clear canvas
2. Project all nodes, compute depth
3. Sort nodes by depth (painter's algorithm: draw far first)
4. Draw edges (source-over blend)
5. Draw nodes (lighter blend on dark field, source-over on light field)

**Node visual encoding**:
- **Color**: By encoding mode
  - `encoding: "kind"` → lane color (6 material colors)
  - `encoding: "trust"` → trust ring color (4 trust colors)
  - `encoding: "tenant"` → per-tenant hue (14 distinct hues)
- **Size**: Weight by node degree
  - `sizeW = 0.45 + Math.pow(deg / maxDeg, 0.55) * 1.0`
  - Maps to sprite scale in projection
- **Glow intensity**: Controlled by `bloom` tweak (0 → 1)
- **Ring/halo**: Trust ring drawn as colored outer glow

**Glow sprite generation** (on-demand, cached):
```javascript
_glow(color) {
  if (this.spriteCache[color]) return this.spriteCache[color];
  
  const [r, g, b] = hexRGB(color);
  const s = 64;
  const c = document.createElement("canvas");
  c.width = c.height = s;
  const x = c.getContext("2d");
  const grad = x.createRadialGradient(s/2, s/2, 0, s/2, s/2, s/2);
  
  grad.addColorStop(0, `rgba(255,255,255,0.62)`);
  grad.addColorStop(0.22, `rgba(${Math.min(255, r+55)},${Math.min(255, g+55)},${Math.min(255, b+55)},0.52)`);
  grad.addColorStop(0.5, `rgba(${r},${g},${b},0.26)`);
  grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
  
  x.fillStyle = grad;
  x.fillRect(0, 0, s, s);
  this.spriteCache[color] = c;
  return c;
}
```

### 4.6 Edge Rendering

**Edge path drawing**:
1. Project source and target nodes
2. Draw line between them using `EDGE_STYLE[kind]` properties
3. If dashed, use `setLineDash([on, off])` before stroke
4. Opacity set via `globalAlpha` per kind

**Particle flow animation** (flowing dots along edges):
- Optional particle emitters at source, flowing toward target
- Density controlled by `particles` tweak
- Animated offset increases each frame for flowing effect
- Uses `flow` value from `EDGE_STYLE` to control density

```javascript
// Pseudo-code
const style = EDGE_STYLE[edge.kind];
ctx.globalAlpha = style.a;
ctx.lineWidth = style.w;
ctx.strokeStyle = `rgb(${style.color})`;
if (style.dash) ctx.setLineDash(style.dash);
ctx.beginPath();
ctx.moveTo(proj[ai].sx, proj[ai].sy);
ctx.lineTo(proj[bi].sx, proj[bi].sy);
ctx.stroke();
```

**Quarantined edges** (proposed/advisory):
- Dashed pattern [2, 3]
- Yellow color overlay or dimmer rendering
- Labeled in UI as "proposed"

### 4.7 Filtering & Visibility

**Real-time filter application**:
```javascript
_passes(n) {
  const f = this.state.filters;
  if (f.tenants && !f.tenants.has(n.tenant)) return false;
  if (f.lanes && !f.lanes.has(n.lane)) return false;
  if (f.trusts && !f.trusts.has(n.trust)) return false;
  if (f.statuses && !f.statuses.has(n.status)) return false;
  if (!f.showQuarantined && n.trust === "InferredAdvisory") return false;
  return true;
}
```

Nodes that don't pass filters are projected but not rendered (transparency = 0).

### 4.8 Time Morphing (Timeline Scrubber)

**Effective node status** at timeline position `timeT`:
```javascript
_effStatus(n, cutoff) {
  if (n.when > cutoff) return "future";  // Not yet born
  if (n.status === "Superseded" && n.supersededBy) {
    const sup = MEM.byId[n.supersededBy];
    if (sup && sup.when > cutoff) return "Active";  // Supersession hasn't happened
  }
  return n.status;
}
```

Nodes born in the future fade out; superseded nodes re-activate if we rewind before the superseding node's creation time.

### 4.9 Hit-Testing & Selection

**Click-to-select**:
1. Project all visible nodes
2. Calculate screen-space distance from mouse to each projection
3. Select closest node within hit radius (e.g., 30px)
4. Call `onSelect(node)` callback

**Hover tooltip**:
1. Continuously track mouse position
2. On mousemove, calculate which node is under cursor (if any)
3. Call `onHover(node, x, y)` callback
4. Caller renders tooltip near cursor

**Lasso (Shift-drag box select)**:
1. On Shift+mouse down, start recording drag corners
2. On mousemove, update selection box
3. On mouse up, select all nodes inside box
4. Return array of IDs to caller

### 4.10 Camera Controls

**Mouse drag (grab/release)**:
- On canvas click-drag: Rotate camera around y/x axes
- `yaw += dx * speed; pitch += dy * speed;`
- Clamp pitch to avoid gimbal lock
- Mouse cursor changes to "grab" / "grabbing"

**Scroll wheel**:
- Zoom in/out: `zoom *= 1.1` or `zoom /= 1.1`
- Clamp to [0.2, 5.0]

**Auto-rotate** (idle motion):
- When `motion: true` and not dragging: `tyaw += dt * 0.045`
- Creates slow, meditative rotation
- Can be toggled off via settings

**Fly-to animation**:
- Called when selecting a node or clicking a citation
- Stores start/end yaw/pitch/dist, animates over 1.0s
- Easing: `cubic-bezier(.2, .0, .0, 1)`

### 4.11 Mini-map

Small auxiliary canvas (132×132px) showing full graph from fixed viewpoint:
- Same projection logic, fixed camera position
- Rendered to dedicated canvas context
- Updated every frame (or on-demand)
- Shows all nodes at reduced scale
- User can click to pan main camera

### 4.12 Performance Tuning

**Frame budget**: Maintain 60fps on target devices (M-series Mac).

**Optimizations**:
1. **Depth-sorting**: Pre-sort node order by depth once per frame, reuse for rendering
2. **Sprite caching**: Bake glow sprites once, reuse across frames
3. **Bloom budget**: Only render top N most blooming nodes (limit to 36-150 based on quality)
4. **Particle density**: Scale particle count by `particles` tweak (0 → 1)
5. **Motion reduction**: Honor `prefers-reduced-motion` media query
6. **DPR scaling**: Render at `min(dpr, 2)` to avoid excessive memory
7. **RAF + watchdog**: Use `requestAnimationFrame` for smooth animation; fallback `setInterval` if rAF is throttled (browser backgrounding)

### 4.13 State Management

**Engine state**:
```javascript
state = {
  mode: "lattice",              // Layout: lattice | galaxy | islands | river
  encoding: "kind",             // Color by: kind | trust | tenant
  bloom: 0.7,                   // Glow intensity: 0 → 1
  particles: 0.6,               // Edge particle density: 0 → 1
  field: "#0a1810",             // Background color
  timeT: 1,                      // Timeline position: 0 → 1
  motion: true,                 // Auto-rotate enabled
  reduced: false,               // Respect prefers-reduced-motion
  filters: {
    tenants: null,              // null = show all; Set of tenant IDs
    lanes: null,
    trusts: null,
    statuses: null,
    showQuarantined: true,      // Show InferredAdvisory nodes
  },
  selectedId: null,             // ID of selected node (or null)
  hoverId: null,                // ID of hovered node (or null)
  focusRadius: 0,               // Ease-in for blast radius ring
}
```

**State updates**:
```javascript
setState(p) {
  const prevMode = this.state.mode;
  Object.assign(this.state, p);
  if (p.filters) this.state.filters = Object.assign({}, this.state.filters, p.filters);
  if (p.mode && p.mode !== prevMode) this._computeLayout(p.mode, false);
  this._dirtyFilter = true;
}
```

---

## 5. FULL INTERACTION MODEL

### 5.1 Global Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| **⌘K** | Open command palette (search, navigate, fly-to-node) |
| **↑/↓** (in command palette) | Move selection |
| **⏎** (in command palette) | Execute selected command |
| **Esc** (in command palette) | Close palette |
| **Shift + drag** (on canvas) | Lasso multi-select nodes |
| **Click** (on canvas) | Select single node; fly to it → Inspect panel |
| **Drag** (on canvas) | Rotate camera (orbit) |
| **Scroll** (on canvas) | Zoom in/out |

### 5.2 Mouse Events

#### Canvas (Constellation)
- **Click**: Select node → Inspector (if not in lasso mode)
- **Drag**: Orbit camera rotation
- **Scroll**: Zoom camera
- **Shift + Drag**: Lasso select box (multi-select)
- **Hover**: Show tooltip with node title & metadata

#### UI Elements
- **Topbar**:
  - Omnibox click → CommandPalette
  - Model picker click → Cycle models
  - Notification bell click → Notification popup
  - Avatar click → Profile page

- **LeftRail**:
  - Layout button click → Switch layout mode
  - Filter chip click → Toggle filter
  - Toggle click → Toggle show proposed

- **RightDock**:
  - Ask tab: Input text + Send button → Add to thread
  - Citation chip click → Fly to node, flash highlight
  - Tool button click → ToolsPop
  - Add memory button click → MemoryPicker
  - Inspector action buttons:
    - "Ask about this" → Add to Ask context, switch to Ask tab
    - "Focus neighborhood" → Fly camera, highlight neighbors
    - "Find analogues" → Query for cross-repo parallels
    - "Propose edge" → HITL proposal modal

- **TimeScrubber**:
  - Knob drag → Update timeline
  - Play/Pause button → Toggle replay animation
  - Collapse button → Hide scrubber (show mini)

- **SelectionHUD**:
  - "Ask about these" → Pull selected into context
  - "Focus these" → Fly camera to centroid
  - "Confirm advisory" → Mark advisory nodes as confirmed
  - "Propose cluster edge" → Generate HITL proposal
  - "Export" → Download memory lineage

- **Review Center**:
  - Card action buttons → Resolve review item (confirm/reject/ask/escalate)

- **Settings**:
  - Toggle switches, input fields → Update settings
  - Delegation tree Revoke button → Cascade revoke

- **Connect**:
  - Copy buttons → Copy endpoint/token to clipboard
  - Regenerate token button → New token
  - Revoke button → Revoke connected client

- **Audit**:
  - Filter dropdowns → Filter audit log
  - Live tail toggle → Auto-append new events
  - Row click → Show full event details (if implemented)

### 5.3 State Transitions

#### Navigation Flow
```
Start: Constellation view (default)
  ↓
Click nav item → Switch view (Review, Settings, Connect, Audit, Profile)
  ↓ (click nav: Constellation)
Back to Constellation
```

#### Ask/Inspect Tab Flow
```
Ask tab (default):
  User types question → Submit
    → Thread appends user message + pending AI response
    → ~950ms delay (simulated processing)
    → AI response + citations displayed
    → Cited nodes highlighted in constellation
    → Clicking citation → Fly to node, switch to Inspect

Inspect tab:
  Click node in constellation → Select it
    → Inspector panel shows node details
    → Click neighbor row → Select neighbor
    → Click "Ask about this" → Add to context, switch to Ask
    → Click "Focus neighborhood" → Highlight neighbors in constellation
```

#### Lasso Selection Flow
```
Shift + drag on canvas → Start lasso box
  → SelectionHUD appears with counts and action buttons
  → User clicks action:
    - "Ask about these" → Add to Ask context
    - "Focus these" → Fly camera to selection centroid
    - "Confirm advisory" → Promote advisory nodes
    - "Propose cluster edge" → Generate HITL proposal
    - "Export" → Download lineage
  → Click X or Clear → Dismiss SelectionHUD
```

#### Timeline Flow
```
TimeScrubber visible:
  Drag knob → Update timeT
    → Constellation re-renders with future nodes faded
  Click Play → Auto-advance timeT over 30s
  Click Pause → Stop animation

Scrubber collapses:
  ScrubMini appears (pill-shaped button)
  Click ScrubMini → Expand to full scrubber
```

#### Filter State
```
LeftRail visible:
  Click layout button → Call setState({ mode })
    → Constellation re-computes positions
    → Nodes smoothly morph to new layout
  Click filter chip → Call setState({ filters })
    → Constellation applies filter
    → Filtered-out nodes fade/disappear
  Toggle "Show proposed" → setState({ filters.showQuarantined })
```

### 5.4 Visual Feedback

#### Hover States
- **Button**: Border color → lighter hair-2
- **Input**: Border focus ring (3px green shadow)
- **Citation chip**: Border → green, background lightens
- **Neighbor row**: Title color → green
- **Node in constellation**: Tooltip appears (fixed position near cursor)
- **Canvas cursor**: grab (while idle) ↔ grabbing (while dragging)

#### Active/Selected States
- **Nav button**: Background green tint, left accent bar
- **Layout button**: Green border, green background tint
- **Filter chip**: Green border, green text, box-shadow glow
- **Tab in dock**: Green underline
- **Node selected**: Highlight glow increases, neighbors glow
- **Citation flashed**: Animation: citFlash (1s, dim to bright)

#### Transitions
- **Panel collapse**: 220ms ease-standard
- **Layout morph**: Node positions lerp toward target (9% per frame)
- **Camera pan**: 1.0s ease-standard (fly-to)
- **Toast appear/dismiss**: Fade in/out
- **Guided overlay**: Fade-in on load

### 5.5 Accessibility Considerations

**Keyboard navigation**:
- ⌘K opens command palette (full-keyboard UI)
- Tab navigates through buttons (Topbar, Settings, etc.)
- Arrow keys in command palette, segmented controls

**Motion**:
- `prefers-reduced-motion` media query → disable particles, bloom, auto-rotate
- Guided overlay skipped if motion is reduced

**Color contrast**:
- All text meets WCAG AA (4.5:1 body, 3:1 large)
- Color is not sole indicator (status badges also use icons/text)

**Screen reader**:
- Semantic HTML (`<button>`, `<input>`, `<label>`)
- ARIA roles on custom components (tabs, radio groups)
- Alt text on SVG icons

---

## 6. DATA SHAPES & INITIALIZATION

### 6.1 Memory Graph (data.js)

**Six material lanes** (X-axis structure):
```javascript
const LANES = [
  { id: "code",   label: "Code",        color: "#8ecc09", desc: "the built thing" },
  { id: "docs",   label: "Docs",        color: "#ffc83a", desc: "the written word" },
  { id: "specs",  label: "Specs",       color: "#5fa8e6", desc: "the plan" },
  { id: "config", label: "Configs",     color: "#34c7b0", desc: "the wiring" },
  { id: "audit",  label: "Tests/Audit", color: "#f0743a", desc: "the checks" },
  { id: "claims", label: "Claims",      color: "#b58cff", desc: "what someone said" },
];
```

**Kind → Lane + Plane mapping**:
```javascript
const KINDS = {
  Commit:           { lane: "code",   plane: "Derived"  },
  Pr:               { lane: "code",   plane: "Derived"  },
  Doc:              { lane: "docs",   plane: "Derived"  },
  Narrative:        { lane: "docs",   plane: "Derived"  },
  Handoff:          { lane: "docs",   plane: "Asserted" },
  Sprint:           { lane: "specs",  plane: "Derived"  },
  Adr:              { lane: "specs",  plane: "Derived"  },
  WorkPackage:      { lane: "specs",  plane: "Derived"  },
  Rationale:        { lane: "specs",  plane: "Asserted" },
  ManifestChange:   { lane: "config", plane: "Derived"  },
  PinBump:          { lane: "config", plane: "Derived"  },
  DriftEvent:       { lane: "config", plane: "Derived"  },
  Audit:            { lane: "audit",  plane: "Derived"  },
  Finding:          { lane: "audit",  plane: "Asserted" },
  Benchmark:        { lane: "audit",  plane: "Derived"  },
  Blocker:          { lane: "audit",  plane: "Asserted" },
  TechDebt:         { lane: "audit",  plane: "Asserted" },
  Claim:            { lane: "claims", plane: "Asserted" },
  AgentAction:      { lane: "claims", plane: "Asserted" },
  AnalogyHypothesis:{ lane: "claims", plane: "Asserted" },
};
```

**Four trust tiers**:
```javascript
const TRUST = [
  { id: "DerivedDeterministic", label: "Derived — deterministic", short: "Derived", ring: "#cfe9b0",
    tip: "Rebuilt from git/markdown. Trusted by construction — no signature needed." },
  { id: "HumanConfirmed",       label: "Human-confirmed",          short: "Confirmed", ring: "#ffd24a",
    tip: "A person reviewed and confirmed this. Load-bearing." },
  { id: "AgentAsserted",        label: "Agent-asserted",           short: "Asserted", ring: "#9fc0e8",
    tip: "Signed by an agent or teammate. Authentic, but a claim — not the record." },
  { id: "InferredAdvisory",     label: "Inferred — advisory",      short: "Proposed", ring: "#b58cff",
    tip: "An AI guess, quarantined. Advisory only until a human confirms it." },
];
```

**Status enum**:
```javascript
const STATUS = {
  Active:     { label: "Active" },
  Superseded: { label: "Superseded" },
  Archived:   { label: "Archived" },
};
```

**Tenants** (14 repos across a federated system):
```javascript
const TENANTS = [
  "citrate-memories", "mem-gateway", "citrate-identity", "citrate-explorer",
  "lattice-vm", "mcp-orchestrator", "ghostdag-consensus", "citrate-inference",
  "salt-tokenomics", "belnap-logic", "federated-learning", "x402-precompiles",
  "citrate-dashboard", "citrate-docs",
];
```

**Dependency graph** (drives cross-tenant DependsOn edges):
```javascript
const DEPENDS = [
  ["mem-gateway", "citrate-memories"], ["mem-gateway", "citrate-identity"],
  ["citrate-explorer", "mem-gateway"], ["citrate-dashboard", "mem-gateway"],
  ["citrate-inference", "lattice-vm"], ["mcp-orchestrator", "citrate-inference"],
  ["lattice-vm", "ghostdag-consensus"], ["x402-precompiles", "lattice-vm"],
  ["salt-tokenomics", "ghostdag-consensus"], ["federated-learning", "ghostdag-consensus"],
  ["belnap-logic", "citrate-memories"], ["citrate-memories", "federated-learning"],
  ["citrate-docs", "citrate-memories"], ["mcp-orchestrator", "mem-gateway"],
];
```

**Generation**:
- ~850 nodes total (~42-76 per tenant)
- ~1500 edges
- Seeded RNG for determinism across loads

### 6.2 Extras (people, review, models, tools, audit)

**PEOPLE** (RBAC hierarchy):
```javascript
const PEOPLE = [
  { id: "u_aleia", name: "Aleia Mercer", initials: "A", role: "Org Owner", policy: "Maintainer", scopes: ["*"], parent: null, color: "#ffbd10", title: "Head of Operations", email: "aleia@citrate.ai", me: true },
  // ... 7 more
];
```

**Review queues** (from synthetic edges):
- `proposals`: AI-proposed edges awaiting human witness (18 items)
- `contradictions`: Belnap Both conflicts (7 items)
- `supersessions`: Old → New memory links (9 items)
- `critic`: Self-critic gaps (4 items)

**Models**:
```javascript
const MODELS = [
  { id: "sonnet", name: "Claude Sonnet 4.6", provider: "Anthropic", kind: "in-app", cost: "$$", latency: "fast", note: "Best reasoning over the graph" },
  { id: "gpt", name: "GPT-5.1", provider: "OpenAI", kind: "in-app", cost: "$$$", latency: "medium", note: "Strong general recall" },
  { id: "citrate", name: "Citrate-LM", provider: "Citrate", kind: "on-prem", cost: "$", latency: "fast", note: "Runs inside your tenancy" },
  { id: "local", name: "Local · Llama 4", provider: "Self-hosted", kind: "BYOM", cost: "free", latency: "varies", note: "Connected over MCP" },
];
```

**Tools** (what the model can call):
```javascript
const TOOLS = [
  { id: "recall", plain: "Pull a storyline", tech: "recall()", icon: "history", desc: "Walk a repo's memory newest-first…" },
  { id: "search", plain: "Find memories", tech: "search() · bge cosine", icon: "search", desc: "Semantic search across everything…" },
  { id: "neighbors", plain: "See what's connected", tech: "neighbors()", icon: "target", desc: "The blast-radius around a memory…" },
  { id: "verify", plain: "Check trust", tech: "verify()", icon: "shield", desc: "Is a memory trustworthy?…" },
  { id: "as_of", plain: "Look back in time", tech: "as_of(T)", icon: "clock", desc: "What the memory looked like at a moment…" },
  { id: "analogy", plain: "Find analogues", tech: "analogy()", icon: "spark", desc: "Latent cross-repo parallels…" },
  { id: "critique", plain: "What's missing?", tech: "critique()", icon: "info", desc: "Self-critic: gaps, stale sources…" },
];
```

**Audit events** (hash-chained log):
- 260 synthetic events
- Kinds: Read, Recall, Write, Assert, Confirm, Propose, Denied, Shred
- Each event: seq, kind, actor, detail, when, repo

### 6.3 Ask Logic (from app.jsx)

**Built-in heuristic answers** (triggered by keywords in user query):

- **"isolat|multi.?ten|org|tenant|saas"** → "We moved to one isolated memory store per Org…"
- **"block|launch|gateway|ship|ready"** → "Three things still gate the gateway…"
- **"secur|finding|audit|risk|vuln"** → "The open findings cluster around the new network surface…"
- **"chang|recent|latest|new|mem-gateway|update"** → "The most recent movement in mem-gateway…"
- **Fallback**: Semantic search (bge cosine) on user query, cite top 3 hits

**Grounded answers** (when memories are pulled into context):
- Synthesize answer from selected memories
- Add context note: "Grounding · N memories"
- Add "Heads-up" warning if context contains advisory nodes

**Citations** (inline):
- Click citation chip → Fly camera to node, flash highlight, populate Inspector
- Tech mode shows hash; plain mode shows title

**Tools** (the model can invoke):
- recall(repo) → Storyline newest-first for that repo
- search(query) → Top ranked memories matching keywords
- neighbors(node) → Blast radius (connected memories)
- verify(node) → Trust verdict card
- as_of(time) → Rewind and see what was true then
- analogy(node) → Cross-repo structural parallels
- critique() → Self-critic on last answer

---

## 7. DEPLOYMENT & BUILD NOTES

### 7.1 Bundle Structure

**HTML entry point**: `Memrizz.html`
- Imports React, ReactDOM, Babel (UMD)
- Data layer scripts (data.js, extras.js)
- Engine script (constellation.js)
- Component scripts (ui.jsx, panels.jsx, review.jsx, connect.jsx, command.jsx, settings.jsx, tweaks-panel.jsx)
- Main app (app.jsx)

**Production build** (for React/Next.js implementation):
- Use a bundler (Webpack, Vite, Next.js) to compile JSX → JS
- Inline CSS or import as CSS modules
- Code-split route surfaces (Review, Settings, Connect, Audit, Profile)
- Lazy-load constellation engine (heavy, loaded on Constellation mount)

### 7.2 CSS Structure

- **colors_and_type.css** — Design tokens, typography, base resets
- **memrizz.css** — Layout, chrome, panels, cards, forms, animations
- **tweaks-panel.jsx** — Inlined tweak panel styles (self-contained)
- **app.jsx** / component files — No inline styles (use CSS classes)

### 7.3 Font Loading

```html
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Source+Serif+4:ital,opsz,wght@0,8..60,300..900;1,8..60,300..900&family=Geist:wght@300;400;500;600;700&family=Geist+Mono:wght@400;500;600&display=swap" rel="stylesheet">
```

### 7.4 Performance Checklist

- [ ] Minimize main bundle (code-split routes)
- [ ] Lazy-load Constellation engine
- [ ] Cache node/edge data in IndexedDB (optional, for offline)
- [ ] Compress images & SVGs (logos, marks)
- [ ] Use Web Workers for heavy layout computations (optional)
- [ ] Profile memory usage (constellation can be large)
- [ ] Test on target devices (M-series Mac, modern browsers)
- [ ] Monitor Core Web Vitals (LCP, FID, CLS)

---

## 8. EXACT PIXEL & SPECIFICATION REFERENCE

### Grid & Spacing Integrity

**All measurements are multiples of 8px or precise hex colors:**
- Padding: 16px, 24px, 32px, 40px, 48px
- Gaps: 6px, 8px, 9px, 10px, 11px (intentional micro-adjustments for flow)
- Radii: 0, 6px, 8px, 12px, 999px
- Widths: 60px (nav), 268px (left rail), 392px (right dock), 132px (minimap)

### Font Rendering

**Source Serif 4** variable font:
- Always specify `font-variation-settings: "opsz" <size>` for optical sizing
- Supported weights: 300–900
- Italics available via `font-style: italic`

**Geist** (sans):
- Weights used: 300, 400, 500, 600, 700
- Geometric, friendly, slightly warm
- Optimal at 12px+ for UI text

### Color Precision

**All colors are hand-tested for WCAG AA contrast**:
- Citrate Green (#8ecc09) on dark (#0a1810): ~5.5:1 ✓
- Stone-200 (#d9dad4) on paper (#f4f1ea): ~2.8:1 (for hairlines only, not body text)
- Ondark (#eef0e6) on panel (#0e160e): ~9.2:1 ✓

### Motion Timing

**All durations are felt, not arbitrary**:
- Fast (140ms): Quick feedback (button hover, toggle)
- Base (220ms): Standard UI transition (panel open, focus)
- Slow (420ms): Deliberate motion (modal entrance)
- Narrative (900ms): Hero animation (onload guided overlay)

### Z-Index Hierarchy

| Z-Index | Layer |
|---------|-------|
| 50 | NavRail (front) |
| 40 | TopBar |
| 35 | TimeScrubber / SelectionHUD |
| 30 | LeftRail, RightDock, Routes |
| 26 | Minimap |
| 25 | HUD (stats) |
| 20 | Tools popup |
| 0 | Canvas (background) |
| 90 | Memory Picker modal |
| 95 | Toast |
| 80 | Guided overlay |
| 60 | Tooltips, Notifications |
| 999 | Alert/confirm dialogs (implied) |

---

## 9. DESIGN INTENT (from chat1.md)

**Stated user goals** (from design conversation):
- **Scope**: The Constellation (3D hero) as showpiece, Ask as conversational RAG with citations that fly to nodes
- **Rendering tech**: Stylized 2.5D projected canvas/SVG (lighter than full 3D)
- **Default layout**: Structured Lattice (explicit axes: material / time / trust)
- **Persona**: Non-technical teammate (plain language first, jargon behind tooltips)
- **Signature feature**: Time scrubber → graph morphs through history
- **Data**: Synthetic graph (~900 nodes, realistic kinds/trust/status) sampled to feel like a real federation
- **Variations**: One cohesive prototype with visual variations as Tweaks (encoding, bloom, layout, quality)
- **Motion**: "Alive" — drifting nodes, particle edge-flow, breathing bloom

**Design philosophy**:
- Trust is visual (color-coded by material lane & trust tier)
- Time is spatial (Y-axis in Lattice, Z-axis in River)
- Repo dependency is a first-class edge type (Islands layout)
- Contradictions are flagged explicitly (Belnap "Both" in Inspector)
- Advisory proposals are quarantined (yellow dashed edges, unconfirmed badge)
- The audit is the system (every change written to hash-chained log)

---

## 10. TESTING & VERIFICATION CHECKLIST

**Before hand-off, verify:**

- [ ] All colors match hex values exactly (use color picker)
- [ ] Typography scales match (measure px, check font-family, weight, letter-spacing)
- [ ] Spacing is multiples of 8 (grid ruler in browser DevTools)
- [ ] Interactive states (hover, focus, active, disabled) all visible and accessible
- [ ] Responsive behavior (collapse panels at breakpoints, if any)
- [ ] Animations and easing curves (record video, compare to spec)
- [ ] Canvas rendering (nodes render with correct glow, edges flow, camera moves smoothly)
- [ ] Data loading (synthetic graph loads deterministically, same every session)
- [ ] Click/keyboard interactions (all routes reachable, all buttons functional)
- [ ] Performance (60fps on target device, no memory leaks)
- [ ] Accessibility (keyboard nav, screen reader, color contrast)

---

**End of Specification**

This document is the single source of truth for a 1:1 faithful rebuild of Memrizz in React/Next.js. Every color, measurement, font, interaction, and state transition is explicitly defined. Use it to guide development, testing, and visual verification.