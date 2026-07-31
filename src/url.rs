use futures::stream::BoxStream;
use iced::advanced::subscription::{self, Hasher};
use iced::{self, Subscription};

/// Routes reaching the app from outside: handed over by a later instance
/// of ourselves, and on macOS delivered by the system as well.
pub fn listen() -> Subscription<String> {
    let handoff = subscription::from_recipe(Handoff);

    #[cfg(target_os = "macos")]
    {
        Subscription::batch([handoff, subscription::from_recipe(OnUrl)])
    }
    #[cfg(not(target_os = "macos"))]
    {
        handoff
    }
}

/// Payloads the single-instance guard collects from later instances.
struct Handoff;

impl subscription::Recipe for Handoff {
    type Output = String;

    fn hash(&self, state: &mut Hasher) {
        use std::hash::Hash;

        struct Marker;
        std::any::TypeId::of::<Marker>().hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: subscription::EventStream,
    ) -> BoxStream<'static, Self::Output> {
        ipc::listen()
    }
}

#[cfg(target_os = "macos")]
struct OnUrl;

#[cfg(target_os = "macos")]
impl subscription::Recipe for OnUrl {
    type Output = String;

    fn hash(&self, state: &mut Hasher) {
        use std::hash::Hash;

        struct Marker;
        std::any::TypeId::of::<Marker>().hash(state);
    }

    fn stream(
        self: Box<Self>,
        input: subscription::EventStream,
    ) -> BoxStream<'static, Self::Output> {
        use futures::stream::StreamExt;
        use iced::advanced::graphics::futures::subscription::{
            Event, MacOS, PlatformSpecific,
        };

        input
            .filter_map(move |event| {
                if let Event::Interaction { status, .. } = &event
                    && *status == iced::event::Status::Captured
                {
                    return futures::future::ready(None);
                }

                let result = match event {
                    Event::PlatformSpecific(event) => match event {
                        PlatformSpecific::MacOS(macos) => match macos {
                            MacOS::ReceivedUrl(url) => Some(url),
                        },
                    },
                    _ => None,
                };

                futures::future::ready(result)
            })
            .boxed()
    }
}
