# GOAL
Build a complete, production-quality monorepo of client-project templates for a
web agency called DFD. These templates are cloned by AI agents to generate real
client websites with config-only differences. NOTHING here may be AI slop:
every project must build, typecheck, lint, and run with realistic data.

Create the repo at a path like: D:\MyProject\dfd-templates
(If your environment is different, use a folder named dfd-templates.)

# NON-NEGOTIABLE QUALITY BAR
- Every project must pass: `npm ci && npm run lint && npm run typecheck && npm run build` (or the equivalent per stack you choose — state it in each project README).
- No lorem ipsum, no placeholder images, no TODO stubs, no default Vite/Next boilerplate text, no dummy "Example Corp" content. All seed content must be realistic Indonesian small-business content (Indonesian language, Rupiah pricing, Indonesian business context: WhatsApp, QRIS, GoFood/ShopeeFood references where relevant).
- Use TypeScript strict mode. Pin all dependency versions in package.json (no `^` floating majors where avoidable; at minimum lock exact minor).
- Every project gets: README.md (setup, env vars, deploy steps), .env.example with every variable documented, Prisma schema with migrations + seed script, CI workflow (install → lint → typecheck → build → test), vercel.json or deployment config, and a smoke test.
- Mobile-first responsive design (Tailwind). Basic SEO (meta, OG tags, sitemap). Lighthouse ≥ 90 performance target.
- Accessibility: semantic HTML, alt text, focus states, contrast.
- No hardcoded secrets. All external services via env vars. Default config must work locally with zero paid services (use SQLite or a Postgres-compatible default — prefer Prisma with a provider you can switch via env; if that's too complex, SQLite default + documented Postgres switch).
- One design system shared across ALL verticals (see common/). Consistent, premium, modern agency-level design — not generic.

# TECH STACK (all projects)
- Next.js 15 (App Router) + TypeScript (strict) + Tailwind CSS (v4 or v3.4+ — pick one and stay consistent)
- Prisma ORM + SQLite default / Postgres-ready
- lucide-react icons, no heavy UI library (hand-rolled components in common/)
- Deploy target: Vercel (serverless) — include vercel.json where needed
- Auth: email+password with signed cookie sessions (iron-session or jose JWT) — simple, dependency-light, secure (httpOnly, sameSite, secure flag)
- WhatsApp: wa.me deep links everywhere + a reusable WhatsAppButton component; NO Meta API
- Review QR: a QR code generator (qrcode.react) component linking to the business's Google review URL (env-configured)
- Payment: none required in v1 — orders go through WhatsApp checkout (add a "Pay later via WhatsApp/QRIS" flow). Do NOT integrate Midtrans now.
- Optional agentic features use an OpenAI-compatible chat client (env: LLM_API_KEY, LLM_BASE_URL, LLM_MODEL — default to a DeepSeek-compatible base URL, feature-flagged OFF by default so the site works with no API key)

# REPO STRUCTURE
```
dfd-templates/
├── README.md                    # explains repo, how to generate a client project (copy + configure)
├── catalog.json                 # machine-readable metadata: each vertical, features, default pricing band, config schema ref
├── common/                      # shared base assets, COPIED into each project at generation time
│   ├── design-system/           # tokens (colors, typography, spacing), Tailwind preset, button/card/nav/footer/form components
│   ├── lib/                     # auth, prisma client, utils, config loader
│   ├── workflows/               # CI workflow files (fast-checks, staging, release)
│   └── deploy/                  # vercel.json, Dockerfile (optional), nginx sample
├── <vertical>/                  # ONE FOLDER PER VERTICAL (12 total)
│   ├── config.schema.json       # the per-client variables for this vertical (see convention below)
│   ├── regular/                 # complete, buildable project — the plain client site
│   └── agentic/                 # complete project = regular + AI/agent layer
```
Each <vertical>/regular and <vertical>/agentic is a COMPLETE standalone project
(own package.json, app/, prisma/, etc.). They may COPY from common/ and from each
other — duplication between verticals is expected and acceptable; consistency of
design system is not negotiable.

# config.schema.json CONVENTION (used by DFD agents to render a client)
Every vertical defines its variables, e.g.:
```json
{
  "business": { "name": "", "tagline": "", "whatsapp": "", "address": "", "hours": "" },
  "brand": { "primaryColor": "", "accentColor": "", "logo": "" },
  "features": { "onlineOrder": true, "booking": false, "reviewQr": true, "promo": false },
  "content": { "menu": [], "pricing": [], "gallery": [], "testimonials": [] }
}
```
The project loads this from `data/site.config.json` (committed, filled by the agent
generating the client) with env overrides. All user-facing text in the site renders
from this config. Provide a filled realistic example `data/site.config.example.json`.

# THE 12 VERTICALS (build ALL of them)
1. fnb — restaurant/cafe: menu catalog + cart → WhatsApp checkout, table booking, review QR, hours/location. Seed: realistic Indonesian cafe menu with Rupiah prices.
2. salon — salon/barber: service menu, staff list, booking form (service+staff+date), membership packages. Seed: salon with 6 services, 3 stylists.
3. laundry — pickup/delivery ordering: price list by kg/item, order form (pickup address, date), order status tracking page (status by order code), coverage area.
4. klinik — clinic/dental: services, doctor profiles, appointment booking, queue info, teleconsult CTA. Seed: 3 doctors, 8 services.
5. gym — gym/fitness: membership plans, class schedule (weekly), class booking, PT packages, trainer bios.
6. bimbel — tutoring/course: course catalog, class schedule, enrollment form, pricing tiers, parent testimonials, WhatsApp registration.
7. bengkel — auto repair: services, price list, service booking, job status tracking by code, emergency tow CTA.
8. beauty — beauty clinic: treatments catalog, packages, consultation form, before/after gallery (placeholder-safe: use real-looking alt text + colored placeholder that is clearly a placeholder pattern, NOT lorem), product/skincare shop via WhatsApp checkout.
9. travel — travel/tour: tour packages, itinerary display, booking + inquiry form, gallery, travel tips blog section.
10. property — property: listings grid with filters, listing detail page, schedule-viewing form, agent contact, mortgage/promo callout.
11. katering — catering: menu packages (per-pax pricing), event types, order form with date/pax count, testimonials, event gallery.
12. photo — photography/video: portfolio gallery (categories), packages/pricing, booking + availability form, about the photographer, reviews.

# REGULAR vs AGENTIC (the differentiator)
- regular/: the plain high-quality site above. All features are deterministic (forms → WhatsApp/email, admin CRUD if applicable, no AI).
- agentic/: everything in regular/ PLUS an "AI layer", all feature-flagged OFF by default and env-gated:
  - ai-chatbot/ — customer-facing chat widget (product/menu help, hours, order status lookup, FAQ) using the OpenAI-compatible client; graceful fallback text when no API key.
  - owner-assistant/ — simple owner-facing dashboard panel: draft review replies, summarize recent WhatsApp-order messages text (paste-in), weekly summary email/SMS draft. Keep it a pragmatic "AI helpers" panel, not a full product.
  - review-summary — admin page that takes pasted Google review text and returns a summary + action list.
  No fake "AI" — if the LLM key is absent, these pages show a clear "not configured" state.

# common/design-system REQUIREMENTS
Tailwind preset with semantic tokens: primary, accent, neutral, surface, text colors; radius; spacing; type scale (Inter font via next/font). Components: Navbar (sticky, mobile menu), Footer (contact, socials, hours, WhatsApp CTA), Hero, SectionHeading, Card, Button (variants), Badge, FormInput/Textarea/Select, Modal, Toast, WhatsAppButton (fixed bottom-right float), ReviewQrSection, ImageWithFallback, Table. All components typed, exported from an index.

# CI WORKFLOWS (in every project, under .github/workflows/)
- ci.yml: on push/PR — npm ci, lint, typecheck, build, test (vitest smoke). Fast (~2 min).
- staging.yml: on push to staging branch — build + deploy to Vercel preview (document the Vercel env var names; leave hooks as placeholders with comments).
- release.yml: on tag push — build + deploy to Vercel production (placeholder with comments).
Branch rule guidance in README: main protected, dev → staging → main.

# BUILD ORDER (follow this)
1. common/ design system + lib + CI + deploy assets (this is the foundation — make it excellent).
2. fnb/regular — the reference implementation. Polish it to the standard every other vertical copies.
3. fnb/agentic — add the AI layer on top of fnb/regular.
4. Build the remaining 10 verticals by copying fnb's structure and adapting pages/schema/content per the list above. Do not copy sloppily — each vertical's schema, pages, and seed data must actually fit its business.
5. catalog.json + root README.

# VERIFY (required before you say done)
- For EVERY vertical × (regular, agentic): run install/lint/typecheck/build/tests, fix until green, and note the exact commands in the README.
- Boot at least 3 representative projects locally and screenshot each home page; include the screenshot paths in your final report.
- Final report: repo path, structure tree, per-vertical build status table, the exact generation steps an agent (or human) follows to clone a vertical into a new client project, known limitations, and anything you had to deviate from this spec.
