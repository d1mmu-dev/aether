//! `/settings`: account password, per-profile settings, profile management and app tokens.

mod appearance;
mod apps;
mod profiles;

const FIELD: &str = "w-full bg-white/5 text-white text-sm px-3 py-2 rounded-lg border border-white/10 outline-none focus:ring-2 focus:ring-brand/30 disabled:opacity-50";
const BUTTON: &str = "inline-flex items-center justify-center rounded-lg border border-white/20 px-3 py-2 text-sm text-white hover:bg-white/10 aria-pressed:bg-brand/20 aria-pressed:border-brand disabled:opacity-50 disabled:cursor-not-allowed";

/// An inline error that follows the language (catalogue messages) or shows the server's body as received.
fn message_view(ctx: &Ctx, message: Value<Option<UiError>>) -> DomNode {
    localized(ctx, message).render_value(|value| match value {
        Some(text) => dom! { <p role="alert" class="text-sm text-danger-light">{text}</p> },
        None => dom! { <span /> },
    })
}

use aether_api_types::auth::{ChangePassword, ChangePasswordResponse, MeResponse};
use vertigo::{DomNode, Value, bind, dom, get_driver};

use crate::api::client::load;
use crate::i18n::{Msg, UiError};
use crate::ui::{Ctx, flag, resource_view};

/// `error` rendered in the current language; changes in place when either the error or the language does.
fn localized(ctx: &Ctx, error: Value<Option<UiError>>) -> vertigo::Computed<Option<String>> {
    let locale = ctx.prefs.locale.clone();
    vertigo::Computed::from(move |c| error.get(c).map(|e| e.text(locale.get(c))))
}

/// The backend's rule (6+ characters) and the confirm field, checked before a round trip; the backend's
/// own message is shown verbatim for anything else (e.g. a wrong current password).
pub fn validate(current: &str, new: &str, confirm: &str) -> Result<ChangePassword, Msg> {
    if current.is_empty() {
        return Err(Msg::EnterCurrentPassword);
    }
    if new.chars().count() < 6 {
        return Err(Msg::NewPasswordTooShort);
    }
    if new != confirm {
        return Err(Msg::PasswordsDoNotMatch);
    }
    Ok(ChangePassword {
        current_password: current.to_owned(),
        new_password: new.to_owned(),
    })
}

/// `components::eyebrow` with text that follows the language.
fn eyebrow_text(text: vertigo::Computed<String>) -> DomNode {
    dom! {
        <p class="t-eyebrow mb-4 flex items-center gap-2">
            <span aria-hidden="true" class="w-1.5 h-1.5 rounded-full bg-brand"></span>
            {text}
        </p>
    }
}

pub fn render(ctx: &Ctx) -> DomNode {
    let me = load(bind!(ctx, async move {
        ctx.client.get::<MeResponse>("/api/auth/me").await
    }));
    let show_form = Value::new(false);
    let signed_in = resource_view(
        &ctx.prefs,
        me.clone(),
        bind!(ctx, |me: std::rc::Rc<MeResponse>| {
            let username = me.username.clone();
            let role = ctx.prefs.text(if crate::auth::is_admin(&me) {
                Msg::RoleAdministrator
            } else {
                Msg::RoleMember
            });
            row(
                ctx.prefs.text(Msg::SignedInAs),
                dom! { <p class="text-xs tabular-nums text-[var(--text-muted)] mt-0.5">{role}</p> },
                dom! { <span id="account-username" class="text-sm text-white">{username}</span> },
            )
        }),
    );
    let password_row = resource_view(
        &ctx.prefs,
        me,
        bind!(ctx, show_form, |me: std::rc::Rc<MeResponse>| {
            if !me
                .profile
                .as_ref()
                .is_some_and(|p| p.is_main && profiles::can_manage(p.kind))
            {
                return dom! { <span /> };
            }
            show_form.render_value(bind!(ctx, show_form, |open: bool| {
        let (class, label) = if open {
            ("text-xs text-[var(--text-muted)] hover:text-white/70 transition-colors", ctx.prefs.text(Msg::Cancel))
        } else {
            ("text-xs text-brand hover:text-brand-hover transition-colors", ctx.prefs.text(Msg::Change))
        };
        let toggle = bind!(show_form, |_| {
            let now = vertigo::transaction(|c| show_form.get(c));
            show_form.set(!now);
        });
        let form = if open {
            password_form(&ctx)
        } else {
            dom! { <span class="hidden" /> }
        };
        dom! {
            <div>
                {row(
                    ctx.prefs.text(Msg::Password),
                    dom! { <p class="text-xs tabular-nums text-[var(--text-muted)] mt-0.5">{ctx.prefs.text(Msg::ChangePasswordHint)}</p> },
                    dom! { <button type="button" class={class} on_click={toggle}>{label}</button> },
                )}
                {form}
            </div>
        }
        }))
        }),
    );
    let ai_row = podcast_ai_row(ctx);
    let profile_section = profiles::render(ctx);
    let apps_section = apps::render(ctx);
    let appearance_section = appearance::render(ctx);
    let t = |m: crate::i18n::Msg| ctx.prefs.text(m);
    dom! {
        <div class="min-h-screen bg-[var(--bg-primary)]">
            <header class="page-shell pt-8 pb-6">
                {eyebrow_text(t(Msg::SettingsEyebrow))}
                <div class="flex flex-wrap items-end justify-between gap-4">
                    <div class="min-w-0">
                        <h1 class="text-4xl md:text-5xl font-bold tracking-tight text-white leading-none">{t(Msg::SettingsTitle)}</h1>
                    </div>
                </div>
            </header>
            <div class="page-shell pb-8 flex gap-12">
                <nav class="w-48 shrink-0 sticky top-8 self-start hidden md:block" aria-label={t(Msg::SettingsSections)}>
                    <div class="space-y-0.5">
                        <div aria-current="true" class="w-full min-h-[44px] flex items-center text-left px-3 py-2 rounded-lg text-sm tabular-nums transition-all text-brand bg-brand/10 border border-brand/20">{t(Msg::SectionAccount)}</div>
                    </div>
                </nav>
                <div class="flex-1 min-w-0">
                    <div class="section-stack">
                        {appearance_section}
                        <section id="account" class="scroll-mt-24 animate-rise">
                            <div class="mb-4">
                                <div class="flex items-center gap-3 mb-1">
                                    <span aria-hidden="true" class="w-1 rounded-full shrink-0 h-6 bg-brand"></span>
                                    <h2 class="font-bold tracking-tight text-white text-lg">{t(Msg::SectionAccount)}</h2>
                                    <span class="flex-1 border-t border-white/10"></span>
                                </div>
                            </div>
                            <div class="space-y-1">
                                {signed_in}
                                {password_row}
                                {ai_row}
                            </div>
                            <div class="border-t border-white/5 mt-8 mb-8"></div>
                        </section>
                        {profile_section}
                        {apps_section}
                    </div>
                </div>
            </div>
        </div>
    }
}

/// The persisted podcast AI switch (aether-only, D-207): AI-labelled shows and episodes are hidden
/// unless this is on. The pill on the podcast pages starts from it.
fn podcast_ai_row(ctx: &Ctx) -> DomNode {
    let on = Value::new(false);
    let busy = Value::new(false);
    if get_driver().is_browser() {
        get_driver().spawn(bind!(ctx, on, async move {
            if let Ok(show) = crate::api::podcasts::ai_setting(&ctx.client).await {
                on.set(show);
            }
        }));
    }
    let toggle = bind!(ctx, on, busy, |_| {
        let want = !vertigo::transaction(|c| on.get(c));
        busy.set(true);
        get_driver().spawn(bind!(ctx, on, busy, async move {
            match crate::api::podcasts::save_ai_setting(&ctx.client, want).await {
                Ok(show) => on.set(show),
                // A server failure body is shown as received.
                Err(err) => ctx.notify(err.message),
            }
            busy.set(false);
        }));
    });
    let checked = on.map(|v| v.to_string());
    let class = on.map(|v| {
        format!(
            "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 {}",
            if v { "bg-brand" } else { "bg-white/15" }
        )
    });
    let knob = on.map(|v| {
        format!(
            "inline-block h-5 w-5 rounded-full bg-white transition-transform {}",
            if v {
                "translate-x-5"
            } else {
                "translate-x-0.5"
            }
        )
    });
    row(
        ctx.prefs.text(Msg::ShowAiPodcasts),
        dom! { <p class="text-xs tabular-nums text-[var(--text-muted)] mt-0.5">{ctx.prefs.text(Msg::ShowAiPodcastsHint)}</p> },
        dom! {
            <button type="button" role="switch" name="podcast-ai" aria-label={ctx.prefs.text(Msg::ShowAiPodcasts)}
                aria-checked={checked} class={class} on_click={toggle}
                disabled={flag(busy.to_computed())}>
                <span class={knob}></span>
            </button>
        },
    )
}

/// `SettingsRow`: label and description on the left, the control on the right (stacked on phones).
fn row(label: vertigo::Computed<String>, description: DomNode, control: DomNode) -> DomNode {
    dom! {
        <div class="flex flex-col gap-1.5 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:min-h-[56px]">
            <div class="sm:flex-1">
                <span class="text-sm font-medium text-white">{label}</span>
                {description}
            </div>
            <div class="w-full sm:w-auto sm:shrink-0">{control}</div>
        </div>
    }
}

fn password_form(ctx: &Ctx) -> DomNode {
    let current = Value::new(String::new());
    let new = Value::new(String::new());
    let confirm = Value::new(String::new());
    let error: Value<Option<UiError>> = Value::new(None);
    // Only ever the catalogue's success message: the server's English body is not shown.
    let done: Value<Option<Msg>> = Value::new(None);
    let busy = Value::new(false);

    let submit = bind!(ctx, current, new, confirm, error, done, busy, || {
        let body = match validate(
            &vertigo::transaction(|c| current.get(c)),
            &vertigo::transaction(|c| new.get(c)),
            &vertigo::transaction(|c| confirm.get(c)),
        ) {
            Ok(b) => b,
            Err(message) => {
                done.set(None);
                error.set(Some(UiError::Msg(message)));
                return;
            }
        };
        error.set(None);
        done.set(None);
        busy.set(true);
        get_driver().spawn(bind!(
            ctx,
            current,
            new,
            confirm,
            error,
            done,
            busy,
            async move {
                match ctx
                    .client
                    .post::<_, ChangePasswordResponse>("/api/auth/change-password", &body)
                    .await
                {
                    Ok(answer) => {
                        // The backend revokes the old tokens; the reply carries the replacement.
                        ctx.session
                            .replace_pair(&answer.token, &answer.refresh_token);
                        current.set(String::new());
                        new.set(String::new());
                        confirm.set(String::new());
                        done.set(Some(Msg::PasswordChanged));
                    }
                    Err(err) => error.set(Some(UiError::Server(err.message))),
                }
                busy.set(false);
            }
        ));
    });

    let error_view = localized(ctx, error).render_value(|e| match e {
        Some(message) => {
            dom! { <p class="inline-flex items-center gap-1.5 text-sm text-danger-light" id="password-error">{message}</p> }
        }
        None => dom! { <span class="hidden" /> },
    });
    let done_text = {
        let locale = ctx.prefs.locale.clone();
        vertigo::Computed::from(move |c| done.get(c).map(|m| m.text(locale.get(c)).to_owned()))
    };
    let done_view = done_text.render_value(|d| match d {
        Some(message) => {
            dom! { <p class="inline-flex items-center gap-1.5 text-sm text-success" id="password-done">{message}</p> }
        }
        None => dom! { <span class="hidden" /> },
    });

    const FIELD: &str = "w-full bg-white/5 text-white text-sm tabular-nums px-3 py-2 rounded-lg border border-white/10 outline-none focus:ring-2 focus:ring-brand/30 focus:border-brand/40 placeholder:text-[var(--text-muted)] transition-all hover:bg-white/[0.08] hover:border-white/20 focus:bg-white/[0.08]";
    dom! {
        <form class="py-4 space-y-3 border-t border-b border-white/5" on_submit={submit}>
            <div class="relative">
                <input id="settings-current" type="password" name="current" aria-label={ctx.prefs.text(Msg::CurrentPassword)} placeholder={ctx.prefs.text(Msg::CurrentPassword)} autocomplete="current-password"
                    class={FIELD} value={current.clone()} on_input={bind!(current, |v: String| current.set(v))} />
            </div>
            <div class="relative">
                <input id="settings-new" type="password" name="new" aria-label={ctx.prefs.text(Msg::NewPasswordLabel)} placeholder={ctx.prefs.text(Msg::NewPasswordPlaceholder)} autocomplete="new-password"
                    class={FIELD} value={new.clone()} on_input={bind!(new, |v: String| new.set(v))} />
            </div>
            <div class="relative">
                <input id="settings-confirm" type="password" name="confirm" aria-label={ctx.prefs.text(Msg::ConfirmNewPassword)} placeholder={ctx.prefs.text(Msg::ConfirmNewPassword)} autocomplete="new-password"
                    class={FIELD} value={confirm.clone()} on_input={bind!(confirm, |v: String| confirm.set(v))} />
            </div>
            <div class="inline-flex items-center gap-3">
                <button type="submit" class="inline-flex items-center justify-center gap-2 rounded-lg font-semibold transition-colors duration-200 disabled:opacity-50 disabled:cursor-not-allowed bg-brand hover:bg-brand-hover text-black px-4 py-2 text-xs" disabled={flag(busy.to_computed())}>{ctx.prefs.text(Msg::ChangePasswordButton)}</button>
                {error_view}
                {done_view}
            </div>
            <p class="text-xs text-warning">{ctx.prefs.text(Msg::PasswordChangeWarning)}</p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    #[test]
    fn password_validation_errors_are_catalogue_messages() {
        assert_eq!(
            validate("", "secret1", "secret1").unwrap_err(),
            Msg::EnterCurrentPassword
        );
        assert_eq!(
            validate("old", "short", "short").unwrap_err(),
            Msg::NewPasswordTooShort
        );
        assert_eq!(
            validate("old", "secret1", "secret2").unwrap_err(),
            Msg::PasswordsDoNotMatch
        );
        // Six characters counted as characters, not bytes.
        let body = validate("old", "żółćźę", "żółćźę").unwrap();
        assert_eq!(body.new_password, "żółćźę");
        assert_eq!(
            Msg::NewPasswordTooShort.text(Locale::Pl),
            "Nowe hasło musi mieć co najmniej 6 znaków"
        );
    }

    #[test]
    fn no_hardcoded_english_left_in_settings_views() {
        for (file, src) in [
            ("settings.rs", include_str!("settings.rs")),
            ("settings/profiles.rs", include_str!("settings/profiles.rs")),
            ("settings/apps.rs", include_str!("settings/apps.rs")),
        ] {
            let view = src.split("#[cfg(test)]").next().unwrap();
            for text in [
                "\"Signed in as\"",
                "\"Change Password\"",
                "\"Profile saved\"",
                "\"Delete permanently\"",
                "\"Create app token\"",
                "\"App token copied\"",
                "\"Revoke\"",
                "\"Apps\"",
                "\"Profiles\"",
                "aria-label=\"",
                "placeholder=\"",
            ] {
                assert!(!view.contains(text), "{file} still hardcodes {text}");
            }
        }
    }
}
