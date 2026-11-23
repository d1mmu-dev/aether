//! Settings -> Appearance & language: the language and theme selectors (localization/themes plan, task 5).
//! A choice applies at once (reactive `<html lang>` / `data-theme`), with no reload and no player change,
//! and is saved to the signed-in profile (D-287); a failed save is undone and reported. The selectors are
//! disabled while the profile's settings load and while a save is pending (one change at a time).

use aether_api_types::settings::{AppearancePatch, UserSettings};
use vertigo::{DomNode, bind, dom, get_driver};

use crate::i18n::{Locale, Msg};
use crate::prefs::Change;
use crate::theme::Theme;
use crate::ui::{Ctx, flag};

const CHOICE: &str = "disabled:opacity-50 disabled:cursor-not-allowed inline-flex items-center gap-2 rounded-lg border border-white/20 px-3 py-2 text-sm text-white hover:bg-white/10 aria-pressed:bg-brand/20 aria-pressed:border-brand";

fn locale_button(ctx: &Ctx, locale: Locale) -> DomNode {
    let pressed = ctx
        .prefs
        .locale
        .to_computed()
        .map(move |l| (l == locale).to_string());
    let on_click = bind!(ctx, |_| {
        // Refused (`Busy`) while the profile's settings load or another save is pending.
        if let Change::Save(ticket) = ctx.prefs.set_locale(locale) {
            let body = AppearancePatch {
                ui_locale: Some(locale.id().to_owned()),
                ..AppearancePatch::default()
            };
            get_driver().spawn(bind!(ctx, async move {
                let saved = save(&ctx, &body).await.is_ok();
                if ctx.prefs.finish_locale(ticket, saved) {
                    ctx.notify(
                        Msg::AppearanceSaveFailed
                            .text(vertigo::transaction(|c| ctx.prefs.locale.get(c))),
                    );
                }
            }));
        }
    });
    // Native names keep their own case; the body is lowercased by the base stylesheet.
    dom! {
        <button type="button" class={CHOICE} style="text-transform: none" lang={locale.id()}
            data-locale={locale.id()} aria-pressed={pressed} disabled={flag(ctx.prefs.busy())} on_click={on_click}>{locale.native_name()}</button>
    }
}

fn theme_button(ctx: &Ctx, theme: Theme) -> DomNode {
    let pressed = ctx
        .prefs
        .theme
        .to_computed()
        .map(move |t| (t == theme).to_string());
    let on_click = bind!(ctx, |_| {
        // Refused (`Busy`) while the profile's settings load or another save is pending.
        if let Change::Save(ticket) = ctx.prefs.set_theme(theme) {
            let body = AppearancePatch {
                ui_theme: Some(theme.id().to_owned()),
                ..AppearancePatch::default()
            };
            get_driver().spawn(bind!(ctx, async move {
                let saved = save(&ctx, &body).await.is_ok();
                if ctx.prefs.finish_theme(ticket, saved) {
                    ctx.notify(
                        Msg::AppearanceSaveFailed
                            .text(vertigo::transaction(|c| ctx.prefs.locale.get(c))),
                    );
                }
            }));
        }
    });
    let (ground, accent, text) = theme.swatch();
    let swatch = format!(
        "display:inline-block;width:2.25rem;height:1.25rem;border-radius:0.375rem;border:1px solid {text};background:linear-gradient(90deg,{ground} 0 50%,{accent} 50% 100%)"
    );
    dom! {
        <button type="button" class={CHOICE} data-theme-choice={theme.id()} aria-pressed={pressed} disabled={flag(ctx.prefs.busy())} on_click={on_click}>
            <span aria-hidden="true" style={swatch}></span>
            <span>{theme.name()}</span>
        </button>
    }
}

async fn save(
    ctx: &Ctx,
    body: &AppearancePatch,
) -> Result<UserSettings, crate::api::client::ApiError> {
    ctx.client.post("/api/settings", body).await
}

fn row(ctx: &Ctx, title: Msg, hint: Msg, controls: DomNode) -> DomNode {
    dom! {
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 py-3">
            <div class="min-w-0">
                <p class="text-sm text-white">{ctx.prefs.text(title)}</p>
                <p class="text-xs text-[var(--text-muted)] mt-0.5">{ctx.prefs.text(hint)}</p>
            </div>
            {controls}
        </div>
    }
}

pub fn render(ctx: &Ctx) -> DomNode {
    let locale_buttons: Vec<DomNode> = Locale::ALL.iter().map(|l| locale_button(ctx, *l)).collect();
    let locales = dom! {
        <div class="flex flex-wrap gap-2" role="group" id="settings-language">
            {..locale_buttons}
        </div>
    };
    // Every bundled theme, in `Theme::ALL` order.
    let theme_buttons: Vec<DomNode> = Theme::ALL.iter().map(|t| theme_button(ctx, *t)).collect();
    let themes = dom! {
        <div class="flex flex-wrap gap-2" role="group" id="settings-theme">
            {..theme_buttons}
        </div>
    };
    let language_row = row(ctx, Msg::Language, Msg::LanguageHint, locales);
    let theme_row = row(ctx, Msg::Theme, Msg::ThemeHint, themes);
    dom! {
        <section id="appearance" class="scroll-mt-24 animate-rise">
            <div class="mb-4">
                <div class="flex items-center gap-3 mb-1">
                    <span aria-hidden="true" class="w-1 rounded-full shrink-0 h-6 bg-brand"></span>
                    <h2 class="font-bold tracking-tight text-white text-lg">{ctx.prefs.text(Msg::SectionAppearance)}</h2>
                    <span class="flex-1 border-t border-white/10"></span>
                </div>
            </div>
            <div class="space-y-1">
                {language_row}
                {theme_row}
                <p class="text-xs text-[var(--text-muted)]">{ctx.prefs.text(Msg::SavedToProfile)}</p>
            </div>
            <div class="border-t border-white/5 mt-8 mb-8"></div>
        </section>
    }
}
