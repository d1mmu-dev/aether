//! `/setup`: create the first admin account (only while the backend reports no account), sign in and
//! mark onboarding complete so the library opens straight away.

use aether_api_types::auth::{Credentials, RegisterResponse};
use aether_api_types::onboarding::OnboardingStatus;
use serde_json::Value as Json;
use vertigo::{DomNode, Resource, Value, bind, dom, get_driver};

use crate::api::client::load;
use crate::brand::PRODUCT_NAME;
use crate::i18n::{Msg, UiError};
use crate::route::Route;
use crate::shell::aether_mark;
use crate::ui::{BTN_PRIMARY, Ctx, LABEL, flag};

/// Mirrors the backend's rules (3-50 character username, 6+ character password) so the common
/// mistakes are caught without a round trip; the backend's own message wins for anything else.
pub fn validate(username: &str, password: &str, confirm: &str) -> Result<Credentials, Msg> {
    let username = username.trim();
    if !(3..=50).contains(&username.chars().count()) {
        return Err(Msg::UsernameLength);
    }
    if password.chars().count() < 6 {
        return Err(Msg::PasswordTooShort);
    }
    if password != confirm {
        return Err(Msg::PasswordsDoNotMatch);
    }
    Ok(Credentials {
        username: username.to_owned(),
        password: password.to_owned(),
    })
}

pub fn render(ctx: &Ctx) -> DomNode {
    let status = load(bind!(ctx, async move {
        ctx.client
            .get::<OnboardingStatus>("/api/onboarding/status")
            .await
    }));
    status.render_value(bind!(ctx, |state| match state {
        Resource::Loading => dom! { <p class="min-h-screen flex items-center justify-center text-sm text-[var(--text-muted)]">{ctx.prefs.text(Msg::Loading)}</p> },
        Resource::Error(message) => dom! { <p class="min-h-screen flex items-center justify-center text-sm text-danger-light">{message}</p> },
        Resource::Ready(s) if s.has_account => {
            // An admin exists already: registration happens elsewhere, sign in instead.
            let router = ctx.router.clone();
            get_driver().spawn(async move { router.set(Route::Login) });
            dom! { <p class="min-h-screen flex items-center justify-center text-sm text-[var(--text-muted)]">{ctx.prefs.text(Msg::AccountExistsRedirecting)}</p> }
        }
        Resource::Ready(_) => form(&ctx),
    }))
}

fn form(ctx: &Ctx) -> DomNode {
    let username = Value::new(String::new());
    let password = Value::new(String::new());
    let confirm = Value::new(String::new());
    let error: Value<Option<UiError>> = Value::new(None);
    let busy = Value::new(false);

    let submit = bind!(ctx, username, password, confirm, error, busy, || {
        let creds = match validate(
            &vertigo::transaction(|c| username.get(c)),
            &vertigo::transaction(|c| password.get(c)),
            &vertigo::transaction(|c| confirm.get(c)),
        ) {
            Ok(c) => c,
            Err(message) => {
                error.set(Some(UiError::Msg(message)));
                return;
            }
        };
        error.set(None);
        busy.set(true);
        get_driver().spawn(bind!(ctx, error, busy, async move {
            match ctx
                .client
                .post::<_, RegisterResponse>("/api/onboarding/register", &creds)
                .await
            {
                Ok(done) => {
                    // The new token must be in place before the authenticated `complete` call.
                    ctx.session.sign_in(&done.token);
                    let _ = ctx
                        .client
                        .post_empty::<Json>("/api/onboarding/complete")
                        .await;
                }
                Err(err) => error.set(Some(UiError::Server(err.message))),
            }
            busy.set(false);
        }));
    });

    let error_view = error.render_value(bind!(ctx, |e| match e {
        Some(e) => dom! {
            <div role="alert" class="p-3 rounded-lg bg-danger/10 border border-danger/30 text-sm text-danger-light">{ctx.prefs.format(move |l| e.text(l))}</div>
        },
        None => dom! { <span /> },
    }));
    let t = |m: Msg| ctx.prefs.text(m);
    let input = "w-full px-4 py-3 bg-white/5 border border-white/10 rounded-lg text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-2 focus:ring-brand/30 focus:border-transparent transition-all";

    dom! {
        <div class="min-h-screen relative overflow-hidden">
            <div class="absolute inset-0 bg-black">
                <div class="absolute inset-0 bg-gradient-to-br from-brand/5 via-transparent to-transparent"></div>
            </div>
            <div class="relative z-(--z-raised) min-h-screen flex items-center justify-center p-6">
                <div class="w-full max-w-4xl">
                    <div class="text-center mb-8">
                        <div class="inline-flex items-center gap-4 mb-4">
                            <div class="relative">
                                <div class="absolute inset-0 bg-white/10 blur-xl rounded-full"></div>
                                {aether_mark(48, "aether-mark inline-block shrink-0 relative z-(--z-raised)")}
                            </div>
                            <h1 class="text-4xl font-bold text-white">{PRODUCT_NAME}</h1>
                        </div>
                        <p class="text-[var(--text-secondary)] text-lg">{t(Msg::SetupTagline)}</p>
                    </div>
                    <div class="bg-(--bg-secondary)/90 rounded-lg border border-white/10 overflow-hidden">
                        <div class="p-6 md:p-8">
                            <div class="space-y-6">
                                <div>
                                    <h2 class="text-2xl font-bold text-white mb-1">{t(Msg::CreateAdminAccount)}</h2>
                                    <p class="text-[var(--text-secondary)]">{ctx.prefs.format(|l| crate::i18n::owner_account_intro(l, PRODUCT_NAME))}</p>
                                </div>
                                <form class="space-y-4 mt-8" on_submit={submit}>
                                    <div>
                                        <label for="username" class={LABEL}>{t(Msg::Username)}</label>
                                        <input id="username" type="text" name="username" placeholder={t(Msg::ChooseUsername)} autocomplete="username"
                                            class={input} value={username.clone()} on_input={bind!(username, |v: String| username.set(v))} />
                                    </div>
                                    <div>
                                        <label for="password" class={LABEL}>{t(Msg::Password)}</label>
                                        <input id="password" type="password" name="password" placeholder={t(Msg::AtLeastSixCharacters)} autocomplete="new-password"
                                            class={input} value={password.clone()} on_input={bind!(password, |v: String| password.set(v))} />
                                    </div>
                                    <div>
                                        <label for="confirmPassword" class={LABEL}>{t(Msg::ConfirmPassword)}</label>
                                        <input id="confirmPassword" type="password" name="confirm" placeholder={t(Msg::ConfirmPasswordPlaceholder)} autocomplete="new-password"
                                            class={input} value={confirm.clone()} on_input={bind!(confirm, |v: String| confirm.set(v))} />
                                    </div>
                                    {error_view}
                                    <button type="submit" class={format!("{BTN_PRIMARY} w-full mt-8")} disabled={flag(busy.to_computed())}>{t(Msg::Continue)}</button>
                                </form>
                            </div>
                        </div>
                    </div>
                    <p class="text-center text-[var(--text-muted)] text-sm mt-6">{ctx.prefs.format(crate::brand::footer)}</p>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_the_backend_rules() {
        assert!(validate("ab", "secret1", "secret1").is_err());
        assert!(validate("admin", "short", "short").is_err());
        assert_eq!(
            validate("admin", "secret1", "other").unwrap_err(),
            Msg::PasswordsDoNotMatch
        );
        assert_eq!(
            validate(" admin ", "secret1", "secret1").unwrap().username,
            "admin"
        );
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // Three characters, six bytes.
        assert!(validate("żół", "secret1", "secret1").is_ok());
    }
}
