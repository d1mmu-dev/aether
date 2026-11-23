//! The collection's tracks tab (all, or one album's), paged. The play button queues the visible page from
//! that track.

use std::rc::Rc;

use aether_api_types::library::TracksResponse;
use vertigo::{DomNode, Resource, Value, bind, computed_tuple, dom};

use crate::api::client::load;
use crate::components::{Tab, library_page};
use crate::i18n::{Msg, track_count};
use crate::tracklist::library_rows;
use crate::ui::{Ctx, Refine, empty_state, error_row, loading_row, pager};

pub fn render(ctx: &Ctx, album: Option<String>) -> DomNode {
    // A new Refine choice rebuilds the page, so it starts at page 1 again.
    ctx.refine
        .render_value(bind!(ctx, album, |refine: Refine| page(
            &ctx,
            album.clone(),
            refine
        )))
}

fn page(ctx: &Ctx, album: Option<String>, refine: Refine) -> DomNode {
    let offset = Value::new(0_i64);
    let refresh = ctx.refresh.clone();
    let key = computed_tuple!(offset, refresh);
    key.render_value(bind!(ctx, offset, album, |(off, _): (i64, u32)| {
        let filter = album
            .as_deref()
            .map(|id| format!("&albumId={id}"))
            .unwrap_or_default();
        let resource = load(bind!(ctx, async move {
            ctx.client
                .get::<TracksResponse>(&format!(
                    "/api/library/tracks?limit={}&offset={off}&sortBy={}{filter}",
                    refine.per_page,
                    refine.sort
                ))
                .await
        }));
        resource.render_value(bind!(ctx, offset, |state: Resource<Rc<TracksResponse>>| {
            match state {
                Resource::Loading => library_page(
                    &ctx,
                    Tab::Tracks,
                    None,
                    loading_row(&ctx.prefs),
                ),
                Resource::Error(message) => library_page(
                    &ctx,
                    Tab::Tracks,
                    None,
                    error_row(message),
                ),
                Resource::Ready(page) => {
                    let total = page.total;
                    let count = ctx.prefs.format(move |l| track_count(l, total));
                    let body = if page.tracks.is_empty() {
                        empty_state("music", ctx.prefs.text(Msg::NoTracks), ctx.prefs.text(Msg::NothingHereYet))
                    } else {
                        dom! {
                            <div>
                                {library_rows(&ctx, page.tracks.clone())}
                                {pager(&ctx.prefs, offset.clone(), page.offset, page.tracks.len(), page.total, refine.per_page)}
                            </div>
                        }
                    };
                    library_page(&ctx, Tab::Tracks, Some(count), body)
                }
            }
        }))
    }))
}
