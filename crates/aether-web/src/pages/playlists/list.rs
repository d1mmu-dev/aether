//! `/playlists`: the user's playlists as a card wall plus a "create" form; creating one opens its detail page.

use std::rc::Rc;

use aether_api_types::playlists::{CreatePlaylist, Playlist, PlaylistDetail, PlaylistListEntry};
use vertigo::{DomNode, Value, bind, dom, get_driver};

use super::validate_name;
use crate::api::client::load;
use crate::components::{
    Card, Kind, eyebrow, media_card, media_grid, mosaic_covers, section_header,
};
use crate::i18n::{Msg, UiError, playlist_count, song_count};
use crate::icons::icon;
use crate::player::queue_item;
use crate::route::Route;
use crate::ui::{Ctx, cover_url, empty_state, flag, resource_view};

/// The mosaic covers of a list entry (its preview tracks), as image URLs.
fn covers_of(entry: &PlaylistListEntry, token: Option<&str>) -> Vec<String> {
    let ranked = mosaic_covers(
        entry
            .items
            .iter()
            .map(|item| item.track.album.cover_art.clone()),
    );
    let size = if ranked.len() == 1 { 300 } else { 200 };
    ranked
        .iter()
        .filter_map(|art| cover_url(art, size, token))
        .collect()
}

const ACTION_BRAND: &str = "flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-semibold transition-all bg-brand text-black hover:bg-brand-hover";
const ACTION_BRAND_ACTIVE: &str = "flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-semibold transition-all bg-brand text-black ring-2 ring-brand/40";
const PANEL_INPUT: &str = "px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:border-brand/50 focus:ring-1 focus:ring-brand/30";

/// Fetches the playlist and plays it from its first track.
fn play_playlist(ctx: &Ctx, id: String) {
    get_driver().spawn(bind!(ctx, async move {
        match ctx
            .client
            .get::<PlaylistDetail>(&format!("/api/playlists/{id}"))
            .await
        {
            Ok(detail) if !detail.items.is_empty() => {
                ctx.player.play_queue(
                    detail.items.iter().map(|i| queue_item(&i.track)).collect(),
                    0,
                );
            }
            Ok(_) => ctx.notify(ctx.prefs.now(Msg::PlaylistHasNoTracks)),
            Err(e) => ctx.notify(e.message),
        }
    }));
}

pub fn render(ctx: &Ctx) -> DomNode {
    let name = Value::new(String::new());
    let error: Value<Option<UiError>> = Value::new(None);
    let busy = Value::new(false);
    let creating = Value::new(false);

    let submit = bind!(ctx, name, error, busy, || {
        let body = match validate_name(&vertigo::transaction(|c| name.get(c))) {
            Ok(name) => CreatePlaylist {
                name,
                is_public: false,
            },
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
                .post::<_, Playlist>("/api/playlists", &body)
                .await
            {
                Ok(created) => {
                    ctx.playlists_rev.change(|n| *n += 1);
                    ctx.router.set(Route::Playlist {
                        id: created.id.to_string(),
                    });
                }
                Err(err) => error.set(Some(UiError::Server(err.message))),
            }
            busy.set(false);
        }));
    });
    let prefs = ctx.prefs.clone();
    let panel = creating.render_value(bind!(creating, name, busy, submit, error, prefs, |open: bool| {
        if !open {
            return dom! { <span class="hidden" /> };
        }
        let error_view = error.render_value(bind!(prefs, |e: Option<UiError>| match e {
            // A catalogue message follows the language in place; a server body is shown as received.
            Some(e) => {
                let message = prefs.format(move |l| e.text(l));
                dom! { <p class="w-full text-xs tabular-nums text-danger-light mt-1">{message}</p> }
            }
            None => dom! { <span class="hidden" /> },
        }));
        let close = bind!(creating, |_| creating.set(false));
        dom! {
            <div class="mt-4 p-4 rounded-lg bg-white/[0.02] border border-white/10">
                <form class="flex flex-wrap items-center gap-3" on_submit={submit.clone()}>
                    <input class={format!("flex-1 min-w-[180px] {PANEL_INPUT}")} type="text" name="name" autofocus="true"
                        placeholder={prefs.text(Msg::PlaylistNamePlaceholder)} value={name.clone()} on_input={bind!(name, |v: String| name.set(v))} />
                    <button class="inline-flex items-center justify-center gap-2 rounded-lg font-semibold transition-colors duration-200 disabled:opacity-50 disabled:cursor-not-allowed bg-brand hover:bg-brand-hover text-black px-4 py-2 text-xs"
                        type="submit" disabled={flag(busy.to_computed())}>
                        {icon("check", "w-3.5 h-3.5")}
                        {prefs.text(Msg::Create)}
                    </button>
                    <button type="button" aria-label={prefs.text(Msg::Close)} on_click={close}
                        class="px-3 py-2 rounded-lg text-xs tabular-nums text-[var(--text-muted)] hover:text-white/70 transition-colors">
                        {icon("x", "w-4 h-4")}
                    </button>
                    {error_view}
                </form>
            </div>
        }
    }));
    let toggle = bind!(creating, |_| {
        let now = vertigo::transaction(|c| creating.get(c));
        creating.set(!now);
    });
    let create_button = creating.render_value(bind!(toggle, prefs, |open: bool| {
        let class = if open { ACTION_BRAND_ACTIVE } else { ACTION_BRAND };
        dom! {
            <button class={class} name="create-playlist" aria-label={prefs.text(Msg::CreatePlaylist)} on_click={toggle.clone()}>
                {icon("plus", "w-4 h-4")}
                <span class="hidden sm:inline">{prefs.text(Msg::Create)}</span>
            </button>
        }
    }));

    let resource = load(bind!(ctx, async move {
        ctx.client
            .get::<Vec<PlaylistListEntry>>("/api/playlists?limit=100")
            .await
    }));
    let description = {
        let (resource, locale) = (resource.clone(), ctx.prefs.locale.clone());
        vertigo::Computed::from(move |c| match resource.get(c) {
            vertigo::Resource::Ready(lists) => playlist_count(locale.get(c), lists.len() as i64),
            _ => "\u{a0}".to_owned(),
        })
    };
    let lists = resource_view(
        &ctx.prefs,
        resource,
        bind!(ctx, |lists: Rc<Vec<PlaylistListEntry>>| {
            if lists.is_empty() {
                return empty_state(
                    "list-music",
                    ctx.prefs.text(Msg::NoPlaylistsYet),
                    ctx.prefs.text(Msg::NoPlaylistsHelp),
                );
            }
            let token = ctx.session.current_token();
            let locale = ctx.prefs.locale.to_computed();
            let cards: Vec<DomNode> = lists
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let id = p.summary.playlist.id.to_string();
                    let play: Rc<dyn Fn()> = {
                        let (ctx, id) = (ctx.clone(), id.clone());
                        Rc::new(move || play_playlist(&ctx, id.clone()))
                    };
                    media_card(
                        locale.clone(),
                        Card {
                            href: Route::Playlist { id }.to_string(),
                            covers: covers_of(p, token.as_deref()),
                            title: p.summary.playlist.name.clone(),
                            subtitle: {
                                let songs = p.summary.track_count;
                                ctx.prefs.format(move |l| song_count(l, songs))
                            },
                            kind: Kind::Playlist,
                            on_play: Some(play),
                            index: i,
                        },
                    )
                })
                .collect();
            dom! {
                <div>
                    {section_header(Kind::Playlist, ctx.prefs.text(Msg::AllPlaylists), lists.len() as i64)}
                    {media_grid(cards)}
                </div>
            }
        }),
    );

    dom! {
        <div class="min-h-screen bg-gradient-to-b from-(--bg-primary) to-black section-stack">
            <header class="page-shell pt-8 pb-6">
                {eyebrow(ctx.prefs.text(Msg::YourLibrary))}
                <div class="flex flex-wrap items-end justify-between gap-4">
                    <div class="min-w-0">
                        <h1 class="text-4xl md:text-5xl font-bold tracking-tight text-white leading-none">{ctx.prefs.text(Msg::NavPlaylists)}</h1>
                        <p class="mt-3 text-sm text-[var(--text-muted)]">{description}</p>
                    </div>
                </div>
                <div class="flex flex-wrap items-center gap-2 mt-5">{create_button}</div>
                {panel}
            </header>
            <div class="relative page-shell pb-24 animate-rise">{lists}</div>
        </div>
    }
}
