//! One module per page.

pub mod albums;
pub mod artists;
pub mod login;
pub mod mixes;
pub mod onboarding;
pub mod playlists;
pub mod podcasts;
pub mod profiles;
pub mod radio;
pub mod search;
pub mod settings;
pub mod tracks;
pub mod vibe;

use vertigo::{DomNode, dom};

use crate::i18n::Msg;
use crate::prefs::Prefs;
use crate::route::Route;

/// Unknown URL, inside the shell.
pub fn not_found(prefs: &Prefs) -> DomNode {
    dom! {
        <div class="page-shell py-12">
            <h1 class="text-2xl font-semibold mb-3 tracking-tight">{prefs.text(Msg::PageNotFound)}</h1>
            <a class="text-brand hover:underline" href={Route::Artists.to_string()}>{prefs.text(Msg::BackToLibrary)}</a>
        </div>
    }
}
