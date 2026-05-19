# Header Poster Composition

## Decision

The Lucarne promotional header poster is composed deterministically from real product screenshots, real QR code pixels, and code-rendered typography. Screenshots are placed inside drawn phone shells instead of asking an image model to recreate the product UI.

## Rationale

The poster contains exact marketing copy, a scannable GitHub QR code, and current Telegram / WeChat product screenshots. Those details are factual assets, so they should not be delegated to generative rendering where text, QR modules, or UI content can drift.

Using a deterministic composition keeps the promotional image reproducible and lets the visual polish stay local to layout, device shells, shadows, and background treatment.

## Alternatives

- Generate the whole poster with an image model. That could produce a more photographic mockup, but it risks corrupting the slogan, QR code, and screenshot contents.
- Use raw screenshots without device shells. That is simpler, but it misses the requested mobile-product presentation and reads less like a header campaign asset.
