# Third-party assets in aether-web

| Asset | Origin | Licence |
|---|---|---|
| `fonts/montserrat-latin.woff2` | Byte-identical copy of upstream kima-hub `frontend/public/fonts/montserrat-latin.woff2` (verified unchanged at 9450c94b, 2026-11-18). Montserrat by Julieta Ulanovsky et al. | SIL Open Font License 1.1 (upstream licence of Montserrat). The OFL allows redistribution and embedding; the font must not be sold on its own. The file itself carries no licence text in this repository, so the OFL attribution lives here. |
| `aether-mark.svg` | Own Aether logo mark (D-284), drawn for this project | Part of this project: GPL-3.0 |
| `src/icons_data.rs` | Path data of 54 icons generated from `lucide-react` 0.552.0 (npm integrity recorded in the file header; checked against the registry tarball, 2026-11-18) by `tools/gen-aether-web-icons.mjs` | ISC (portions MIT, Cole Bemis 2013-2023, Feather); copyright Lucide Contributors 2025 |
| `aether-base.css` / `aether.css` | Tailwind CSS v4 output compiled from styles ported from upstream kima-hub's frontend by `tools/gen-aether-web-css.mjs` | Tailwind CSS is MIT; the project styles are GPL-3.0 |

ISC licence text of Lucide: <https://github.com/lucide-icons/lucide/blob/main/LICENSE>.
Montserrat OFL 1.1: <https://openfontlicense.org/> and <https://github.com/JulietaUla/Montserrat>.
