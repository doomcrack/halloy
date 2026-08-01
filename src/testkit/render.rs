//! View-as-text: lays out the real `Frigicom::view` headlessly and returns
//! every text node that is actually on screen, in tree order.
//!
//! No pixels are ever compared. A renderer exists only because layout has
//! to measure text; `ICED_TEST_BACKEND=tiny-skia` (set in
//! `.cargo/config.toml`) keeps that off the GPU, so a call costs well under
//! a millisecond after the first and works on any runner.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock, Mutex};

use iced::Size;
use iced_test::Simulator;
use iced_test::selector::{Candidate, Selector};

use crate::widget::Renderer;
use crate::{Frigicom, Message, Theme, font, theme, window};

/// The viewport every view-as-text call lays out into. Wide enough for the
/// sidebar plus a pane, so nothing is clipped away by accident.
const VIEWPORT: Size = Size::new(1280.0, 800.0);

/// The embedded fonts are pushed into the process-wide font system once;
/// re-loading them on every call would grow it without bound.
static FONTS: LazyLock<()> = LazyLock::new(|| {
    let element: crate::widget::Element<'_, Message> =
        iced::widget::Space::new().into();

    let _: Simulator<'_, Message, Theme, Renderer> =
        Simulator::with_size(settings(font::load()), VIEWPORT, element);
});

/// Lays out `app.view(id)` and collects the visible text.
pub fn text(app: &Frigicom, id: window::Id) -> Vec<String> {
    LazyLock::force(&FONTS);

    let collected = Arc::new(Mutex::new(Vec::new()));

    let mut ui: Simulator<'_, Message, Theme, Renderer> =
        Simulator::with_size(settings(vec![]), VIEWPORT, app.view(id));

    // `Collect` never yields, so the traversal visits the whole tree and
    // the (expected) `SelectorNotFound` is how it reports it is done.
    let _ = ui.find(Collect(Arc::clone(&collected)));

    collected.lock().expect("collector not poisoned").clone()
}

/// Counts the text entry points `app.view(id)` puts on screen.
///
/// The composer is a `text_editor`, which reports itself to a traversal as a
/// *focusable* rather than a text input — so this counts focusables. Nothing
/// else on the dashboard is one: buttons, scrollables and text are each their
/// own kind of candidate. A pane that cannot be typed into therefore
/// contributes zero, which is the view-level statement of "read-only".
pub fn composers(app: &Frigicom, id: window::Id) -> usize {
    LazyLock::force(&FONTS);

    let counted = Arc::new(Mutex::new(0));

    let mut ui: Simulator<'_, Message, Theme, Renderer> =
        Simulator::with_size(settings(vec![]), VIEWPORT, app.view(id));

    let _ = ui.find(CountComposers(Arc::clone(&counted)));

    *counted.lock().expect("counter not poisoned")
}

fn settings(fonts: Vec<Cow<'static, [u8]>>) -> iced::Settings {
    iced::Settings {
        default_font: font::MONO.clone().into(),
        default_text_size: theme::TEXT_SIZE.into(),
        fonts,
        ..iced::Settings::default()
    }
}

#[derive(Clone)]
struct Collect(Arc<Mutex<Vec<String>>>);

impl Selector for Collect {
    type Output = ();

    fn select(&mut self, candidate: Candidate<'_>) -> Option<()> {
        let Candidate::Text {
            content,
            visible_bounds,
            ..
        } = candidate
        else {
            return None;
        };

        let content = content.trim();

        if visible_bounds.is_some() && !content.is_empty() {
            self.0
                .lock()
                .expect("collector not poisoned")
                .push(content.to_owned());
        }

        None
    }

    fn description(&self) -> String {
        "every visible text node".to_owned()
    }
}

#[derive(Clone)]
struct CountComposers(Arc<Mutex<usize>>);

impl Selector for CountComposers {
    type Output = ();

    fn select(&mut self, candidate: Candidate<'_>) -> Option<()> {
        if matches!(
            candidate,
            Candidate::Focusable {
                visible_bounds: Some(_),
                ..
            }
        ) {
            *self.0.lock().expect("counter not poisoned") += 1;
        }

        None
    }

    fn description(&self) -> String {
        "every visible text entry point".to_owned()
    }
}
