# Bundled renderer font

`NotoSansMono-Regular.ttf` is the renderer's deterministic display font. It
is loaded from `include_bytes!`; runtime font lookup is intentionally absent.

- Upstream: `notofonts/noto-fonts`,
  `hinted/ttf/NotoSansMono/NotoSansMono-Regular.ttf`
- Retrieved: 2026-08-23
- SHA-256: `d9e2b23d19f8230be7146f409a52b1d23117e635e28f2e2892cf91b7382f325b`
- License: SIL Open Font License 1.1, reproduced in
  `NotoSansMono-OFL.txt`

The pure-Rust rasterizer is `fontdue 0.9.4`, vendored with its transitive
dependencies under `rust/vendor/`. Font size, center alignment, and quarter-
turn rotation are renderer policy and do not inherit KLayout or OS settings.
