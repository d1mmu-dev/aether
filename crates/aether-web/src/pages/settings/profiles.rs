use super::{BUTTON, FIELD, message_view};
use crate::api::profiles as api;
use crate::i18n::{Msg, UiError, profile_count, type_name_to_confirm};
use crate::ui::{Ctx, flag, resource_view};
use aether_api_types::profiles::{
    AVATAR_PRESETS, CreateProfile, Profile, ProfileKind, ProfilesResponse, is_avatar_preset,
};
use std::rc::Rc;
use vertigo::{DomNode, Resource, Value, bind, dom, get_driver};

type Data = Value<Resource<Rc<ProfilesResponse>>>;

fn refresh(ctx: &Ctx, data: Data, error: Value<Option<UiError>>, busy: Value<bool>) {
    get_driver().spawn(bind!(ctx, async move {
        match api::list(&ctx.client).await {
            Ok(answer) => data.set(Resource::Ready(Rc::new(answer))),
            Err(err) => {
                data.set(Resource::Error(err.message.clone()));
                error.set(Some(UiError::Server(err.message)));
            }
        }
        busy.set(false);
    }));
}

pub(super) fn render(ctx: &Ctx) -> DomNode {
    let data: Data = Value::new(Resource::Loading);
    let error = Value::new(None);
    let busy = Value::new(false);
    if get_driver().is_browser() {
        refresh(ctx, data.clone(), error.clone(), busy.clone());
    }
    let content = resource_view(
        &ctx.prefs,
        data.to_computed(),
        bind!(ctx, data, error, busy, |answer: Rc<ProfilesResponse>| {
            let manager = answer
                .profiles
                .iter()
                .find(|p| p.id == answer.current)
                .is_some_and(|p| can_manage(p.kind) && p.is_main);
            let rows: Vec<DomNode> = answer.profiles.iter().map(|p| {
            let picture = crate::pages::profiles::avatar(p);
            let name = p.name.clone();
            let kind = ctx.prefs.text(kind_label(p.kind));
            let edit = if manager { editor(&ctx, Some(p.clone()), data.clone(), error.clone(), busy.clone()) } else { dom! { <span /> } };
            dom! { <div class="border border-white/10 rounded-lg p-4 space-y-3">
                <div class="flex items-center gap-3">{picture}<span class="font-semibold text-white">{name}</span><span class="text-xs text-brand">{kind}</span></div>
                {edit}
            </div> }
        }).collect();
            let add = if can_add(manager, answer.profiles.len(), answer.max) {
                editor(&ctx, None, data.clone(), error.clone(), busy.clone())
            } else if manager {
                dom! { <p class="text-sm text-warning">{ctx.prefs.text(Msg::ProfileLimitReached)}</p> }
            } else {
                dom! { <p class="text-sm text-[var(--text-muted)]">{ctx.prefs.text(Msg::OnlyMainManagesProfiles)}</p> }
            };
            let (len, max) = (answer.profiles.len(), answer.max);
            let count = ctx.prefs.format(move |l| profile_count(l, len, max));
            dom! { <div class="space-y-4"><p class="text-sm text-[var(--text-muted)]">{count}</p>{..rows}{add}</div> }
        }),
    );
    dom! { <section id="profiles" class="space-y-4">
        <h2 class="text-lg font-bold text-white">{ctx.prefs.text(Msg::Profiles)}</h2>
        <p class="text-sm text-[var(--text-muted)]">{ctx.prefs.text(Msg::ProfilesIntro)}</p>
        {content}{message_view(ctx, error)}
    </section> }
}

fn editor(
    ctx: &Ctx,
    profile: Option<Profile>,
    data: Data,
    error: Value<Option<UiError>>,
    busy: Value<bool>,
) -> DomNode {
    let name = Value::new(profile.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let kind = Value::new(
        profile
            .as_ref()
            .map(|p| p.kind)
            .unwrap_or(ProfileKind::Standard),
    );
    let avatar = Value::new(
        profile
            .as_ref()
            .and_then(|p| p.avatar.clone())
            .unwrap_or_default(),
    );
    let is_main = profile
        .as_ref()
        .is_some_and(|p| p.kind == ProfileKind::Main);
    let title = ctx.prefs.text(if profile.is_some() {
        Msg::EditProfile
    } else {
        Msg::AddProfile
    });
    let submit = bind!(ctx, profile, name, kind, avatar, data, error, busy, || {
        if vertigo::transaction(|c| busy.get(c)) {
            return;
        }
        let (n, k, a) = vertigo::transaction(|c| (name.get(c), kind.get(c), avatar.get(c)));
        let update = match update_form(&n, k, &a) {
            Ok(b) => b,
            Err(e) => {
                error.set(Some(UiError::Msg(e)));
                return;
            }
        };
        // Creating a main profile is never possible; main's kind is omitted on edit.
        let create = if profile.is_none() {
            match profile_form(&n, k, &a) {
                Ok(b) => Some(b),
                Err(e) => {
                    error.set(Some(UiError::Msg(e)));
                    return;
                }
            }
        } else {
            None
        };
        busy.set(true);
        error.set(None);
        get_driver().spawn(bind!(ctx, profile, data, error, busy, async move {
            let result = match (profile, create) {
                (Some(p), _) => api::update(&ctx.client, p.id, &update).await,
                (_, Some(body)) => api::create(&ctx.client, &body).await,
                _ => return,
            };
            match result {
                Ok(_) => {
                    api::invalidate(&ctx.profiles_rev);
                    ctx.notify(ctx.prefs.now(Msg::ProfileSaved));
                    refresh(&ctx, data, error, busy);
                }
                Err(err) => {
                    error.set(Some(UiError::Server(err.message)));
                    busy.set(false);
                }
            }
        }));
    });
    let kinds = if is_main {
        dom! { <p class="text-xs text-[var(--text-muted)]">{ctx.prefs.text(Msg::MainKindFixed)}</p> }
    } else {
        let choices: Vec<DomNode> = [ProfileKind::Standard, ProfileKind::Kids].into_iter().map(|k| {
            let label = ctx.prefs.text(kind_label(k));
            let checked = kind.map(move |v| (v == k).to_string());
            let click = bind!(kind, |_| kind.set(k));
            dom! { <button type="button" class={BUTTON} aria-pressed={checked} on_click={click} disabled={flag(busy.to_computed())}>{label}</button> }
        }).collect();
        dom! { <div role="group" aria-label={ctx.prefs.text(Msg::ProfileKindGroup)} class="flex gap-2">{..choices}</div> }
    };
    let avatars: Vec<DomNode> = std::iter::once("").chain(AVATAR_PRESETS).map(|preset| {
        let label = match avatar_label(preset) {
            Some(name) if !preset.is_empty() => {
                let glyph = crate::pages::profiles::avatar_text(Some(preset), "");
                ctx.prefs.format(move |l| format!("{glyph} {}", name.text(l)))
            }
            Some(name) => ctx.prefs.text(name),
            // A preset without a catalogue name (never shipped) keeps its identifier.
            None => vertigo::Computed::from(move |_| preset.to_owned()),
        };
        let checked = avatar.map(move |v| (v == preset).to_string());
        let click = bind!(avatar, |_| avatar.set(preset.into()));
        dom! { <button type="button" class={BUTTON} aria-pressed={checked} on_click={click} disabled={flag(busy.to_computed())}>{label}</button> }
    }).collect();
    let delete = match profile {
        Some(p) if !is_main => deletion(ctx, p, data, error, busy.clone()),
        _ => dom! { <span /> },
    };
    dom! { <details class="space-y-3">
        <summary class="text-brand cursor-pointer">{title}</summary>
        <form on_submit={submit} class="space-y-3 py-3">
            <input class={FIELD} aria-label={ctx.prefs.text(Msg::ProfileName)} placeholder={ctx.prefs.text(Msg::ProfileNamePlaceholder)} maxlength="50" value={name.clone()} on_input={bind!(name, |v: String| name.set(v))} disabled={flag(busy.to_computed())} />
            {kinds}<div role="group" aria-label={ctx.prefs.text(Msg::AvatarPreset)} class="flex flex-wrap gap-2">{..avatars}</div>
            <button type="submit" class={BUTTON} disabled={flag(busy.to_computed())}>{ctx.prefs.text(Msg::SaveProfile)}</button>
        </form>{delete}
    </details> }
}

fn deletion(
    ctx: &Ctx,
    profile: Profile,
    data: Data,
    error: Value<Option<UiError>>,
    busy: Value<bool>,
) -> DomNode {
    let typed = Value::new(String::new());
    // Track typed text explicitly as well as busy.
    let allowed = {
        let typed = typed.clone();
        let busy = busy.clone();
        let p = profile.clone();
        vertigo::Computed::from(move |c| !busy.get(c) && can_delete(p.kind, &p.name, &typed.get(c)))
    };
    let click = bind!(ctx, profile, data, error, busy, typed, |_| {
        if vertigo::transaction(|c| {
            busy.get(c) || !can_delete(profile.kind, &profile.name, &typed.get(c))
        }) {
            return;
        }
        busy.set(true);
        error.set(None);
        get_driver().spawn(bind!(ctx, profile, data, error, busy, async move {
            match api::delete(&ctx.client, profile.id).await {
                Ok(_) => {
                    api::invalidate(&ctx.profiles_rev);
                    ctx.notify(ctx.prefs.now(Msg::ProfileDeleted));
                    refresh(&ctx, data, error, busy);
                }
                Err(err) => {
                    error.set(Some(UiError::Server(err.message)));
                    busy.set(false);
                }
            }
        }));
    });
    let name = profile.name.clone();
    let prompt = ctx.prefs.format(move |l| type_name_to_confirm(l, &name));
    dom! { <details class="border-t border-white/10 py-3 space-y-3">
        <summary class="text-danger-light cursor-pointer">{ctx.prefs.text(Msg::DeleteProfilePermanently)}</summary>
        <p class="text-sm text-warning">{ctx.prefs.text(Msg::DeleteProfileWarning)}</p>
        <label class="text-sm text-white">{prompt}<input class={FIELD} aria-label={ctx.prefs.text(Msg::ConfirmProfileName)} autocomplete="off" value={typed.clone()} on_input={bind!(typed, |v: String| typed.set(v))} /></label>
        <button type="button" class={BUTTON} disabled={flag(allowed.map(|v| !v))} on_click={click}>{ctx.prefs.text(Msg::DeletePermanently)}</button>
    </details> }
}

fn profile_form(name: &str, kind: ProfileKind, avatar: &str) -> Result<CreateProfile, Msg> {
    let name = name.trim();
    if !(1..=50).contains(&name.chars().count()) {
        return Err(Msg::ProfileNameLength);
    }
    if !matches!(kind, ProfileKind::Kids | ProfileKind::Standard) {
        return Err(Msg::InvalidProfileKind);
    }
    if !avatar.is_empty() && !is_avatar_preset(avatar) {
        return Err(Msg::UnknownAvatar);
    }
    Ok(CreateProfile {
        name: name.into(),
        kind,
        avatar: (!avatar.is_empty()).then(|| avatar.into()),
    })
}

/// The role label of a profile kind (a future kind the client does not know reads "Unknown").
fn kind_label(kind: ProfileKind) -> Msg {
    match kind {
        ProfileKind::Main => Msg::KindMain,
        ProfileKind::Kids => Msg::KindKids,
        ProfileKind::Standard => Msg::KindStandard,
        ProfileKind::Unknown => Msg::KindUnknown,
    }
}

/// The name of an avatar choice: `""` is the profile's initial.
fn avatar_label(preset: &str) -> Option<Msg> {
    Some(match preset {
        "" => Msg::AvatarInitial,
        "cat" => Msg::AvatarCat,
        "dog" => Msg::AvatarDog,
        "fox" => Msg::AvatarFox,
        "owl" => Msg::AvatarOwl,
        "bear" => Msg::AvatarBear,
        "panda" => Msg::AvatarPanda,
        "rabbit" => Msg::AvatarRabbit,
        "penguin" => Msg::AvatarPenguin,
        "robot" => Msg::AvatarRobot,
        "rocket" => Msg::AvatarRocket,
        "star" => Msg::AvatarStar,
        "music" => Msg::AvatarMusic,
        _ => return None,
    })
}

fn can_add(manager: bool, count: usize, max: u32) -> bool {
    manager && count < max as usize
}

pub(super) fn can_manage(kind: ProfileKind) -> bool {
    kind == ProfileKind::Main
}

fn can_delete(kind: ProfileKind, name: &str, typed: &str) -> bool {
    matches!(kind, ProfileKind::Kids | ProfileKind::Standard) && !name.is_empty() && name == typed
}

fn update_form(
    name: &str,
    kind: ProfileKind,
    avatar: &str,
) -> Result<aether_api_types::profiles::UpdateProfile, Msg> {
    let body = profile_form(
        name,
        if kind == ProfileKind::Main {
            ProfileKind::Standard
        } else {
            kind
        },
        avatar,
    )?;
    Ok(aether_api_types::profiles::UpdateProfile {
        name: Some(body.name),
        kind: (kind != ProfileKind::Main).then_some(kind),
        avatar: Some(body.avatar),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_api_types::profiles::ProfileKind;

    #[test]
    fn add_stops_at_advertised_limit_without_restricting_kids_count() {
        assert!(can_add(true, 4, 5));
        assert!(!can_add(true, 5, 5));
        assert!(!can_add(false, 1, 5));
        assert!(profile_form("Child one", ProfileKind::Kids, "cat").is_ok());
        assert!(profile_form("Child two", ProfileKind::Kids, "dog").is_ok());
    }

    #[test]
    fn management_and_password_require_main_not_admin_role() {
        assert!(can_manage(ProfileKind::Main));
        for kind in [
            ProfileKind::Kids,
            ProfileKind::Standard,
            ProfileKind::Unknown,
        ] {
            assert!(!can_manage(kind));
        }
    }

    #[test]
    fn deletion_requires_exact_name_and_never_main() {
        assert!(can_delete(ProfileKind::Kids, "Kids", "Kids"));
        assert!(!can_delete(ProfileKind::Kids, "Kids", "kids"));
        assert!(!can_delete(ProfileKind::Kids, "Kids", " Kids "));
        assert!(!can_delete(ProfileKind::Main, "Main", "Main"));
        assert!(!can_delete(ProfileKind::Unknown, "Future", "Future"));
    }

    #[test]
    fn editing_main_keeps_kind_and_can_clear_avatar() {
        let update = update_form(" Renamed ", ProfileKind::Main, "").unwrap();
        assert_eq!(update.name.as_deref(), Some("Renamed"));
        assert_eq!(update.kind, None);
        assert_eq!(update.avatar, Some(None));
        let update = update_form("Child", ProfileKind::Kids, "dog").unwrap();
        assert_eq!(update.kind, Some(ProfileKind::Kids));
        assert_eq!(update.avatar, Some(Some("dog".into())));
    }

    #[test]
    fn every_avatar_preset_and_kind_has_a_translated_label() {
        use crate::i18n::Locale;
        for preset in AVATAR_PRESETS {
            assert!(avatar_label(preset).is_some(), "{preset} has no label");
        }
        assert_eq!(avatar_label("upload"), None);
        assert_eq!(avatar_label("").unwrap().text(Locale::Pl), "Inicjał");
        assert_eq!(kind_label(ProfileKind::Kids).text(Locale::Pl), "Dziecięcy");
        assert_eq!(kind_label(ProfileKind::Main).text(Locale::En), "Main");
        assert_eq!(
            kind_label(ProfileKind::Unknown).text(Locale::Pl),
            "Nieznany"
        );
    }

    #[test]
    fn profile_form_errors_are_catalogue_messages() {
        assert_eq!(
            profile_form(" ", ProfileKind::Kids, "").unwrap_err(),
            Msg::ProfileNameLength
        );
        assert_eq!(
            profile_form("A", ProfileKind::Main, "").unwrap_err(),
            Msg::InvalidProfileKind
        );
        assert_eq!(
            profile_form("A", ProfileKind::Kids, "upload").unwrap_err(),
            Msg::UnknownAvatar
        );
    }

    #[test]
    fn profile_form_trims_counts_unicode_and_only_accepts_presets() {
        let body = profile_form("  Żółw 🐢  ", ProfileKind::Kids, "cat").unwrap();
        assert_eq!(body.name, "Żółw 🐢");
        assert_eq!(body.avatar.as_deref(), Some("cat"));
        assert!(profile_form(" ", ProfileKind::Kids, "").is_err());
        assert!(profile_form(&"ż".repeat(50), ProfileKind::Standard, "").is_ok());
        assert!(profile_form(&"ż".repeat(51), ProfileKind::Kids, "").is_err());
        assert!(profile_form("Kids", ProfileKind::Main, "").is_err());
        assert!(profile_form("Kids", ProfileKind::Unknown, "").is_err());
        assert!(profile_form("Kids", ProfileKind::Kids, "upload").is_err());
    }
}
