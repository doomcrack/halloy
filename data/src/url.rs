use std::borrow::Cow;
use std::str::FromStr;

use idna::uts46::{AsciiDenyList, Hyphens, Uts46};
use percent_encoding::percent_decode_str;
use unicode_security::confusable_detection::skeleton;
use unicode_security::{RestrictionLevel, RestrictionLevelDetection};

use crate::appearance::theme;

/// Routable app URLs. IRC connection routing (`irc://`…) is gone; theme
/// deep links keep the historical `halloy` scheme so shared themes and
/// the theme website keep working.
#[derive(Debug, Clone)]
pub enum Url {
    Theme { url: String, styles: theme::Styles },
    Unknown(String),
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Url::Theme { url, .. } | Url::Unknown(url) => url,
            }
        )
    }
}

pub fn theme(colors: &theme::Styles) -> String {
    format!("halloy:///theme?e={}", colors.encode_base64())
}

pub fn theme_submit(colors: &theme::Styles) -> String {
    format!(
        "https://themes.halloy.chat/submit?e={}",
        colors.encode_base64()
    )
}

impl Url {
    pub fn find_in(mut args: impl Iterator<Item = String>) -> Option<Self> {
        args.find_map(|arg| arg.parse().ok())
    }
}

impl FromStr for Url {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = s.parse::<url::Url>().map_err(|_| ())?;

        if ["halloy", "frigicom"].contains(&url.scheme()) {
            Ok(parse(url.clone()).unwrap_or(Url::Unknown(url.to_string())))
        } else {
            Err(())
        }
    }
}

fn parse(url: url::Url) -> Result<Url, Error> {
    match url.scheme().to_lowercase().as_str() {
        "halloy" | "frigicom" if url.path() == "/theme" => {
            let (_, encoded) = url
                .query_pairs()
                .find(|(key, _)| key == "e")
                .ok_or(Error::MissingQueryPair)?;

            let styles = theme::Styles::decode_base64(&encoded)?;

            Ok(Url::Theme {
                url: url.into(),
                styles,
            })
        }
        _ => Err(Error::Unknown),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    ParseUrl(#[from] url::ParseError),
    #[error("unknown route")]
    Unknown,
    #[error("missing query pair")]
    MissingQueryPair,
    #[error("failed to parse encoded theme: {0}")]
    ParseEncodedTheme(#[from] theme::Error),
}

/// Returns the human-readable form of a URL.
///
/// If hosts don't pass a specific validation criteria, they are displayed as punycode.
/// The path segment of a URL is always `percent-encoding` decoded.
///
/// We apply a best effort to handle international domain names in a way that matches browser
/// status-quo; The host is rendered according to UTS #46 when it passes UTS #39 "Highly Restrictive"
/// level, and its skeleton is not pure ASCII (which guards against homograph attacks).
///
/// See https://unicode.org/reports/tr46/ and https://www.unicode.org/reports/tr39/ for
/// motivations. https://chromium.googlesource.com/chromium/src/+/main/docs/idn.md
/// is not a bad read either - though note that chat clients have very different threat
/// models than browsers.
///
pub fn display(u: &url::Url) -> Cow<'_, str> {
    u.host_str()
        .and_then(|host| {
            let (host_str, _) = Uts46::new().to_user_interface(
                host.as_bytes(),
                AsciiDenyList::EMPTY,
                Hyphens::Allow,
                |label, _tld, _is_bidi| {
                    let label_str: String = label.iter().collect();
                    let s = label_str.as_str();
                    // reject mixed-script confusables
                    s.check_restriction_level(RestrictionLevel::HighlyRestrictive)
                        // OR if every character maps to an ASCII confusable prototype;
                        // not something the spec mandates, but browsers do this
                        && !skeleton(s).all(|c| c.is_ascii())
                },
            );
            u.as_str().split_once(host).map(|(scheme, path)| {
                // https://ja.wikipedia.org/wiki/%E9%87%8D%E9%9F%B3%E3%83%86%E3%83%88
                // -> https://ja.wikipedia.org/wiki/重音テト
                let path = percent_decode_str(path).decode_utf8_lossy();
                Cow::Owned(format!("{scheme}{host_str}{path}"))
            })
        })
        .unwrap_or(Cow::Borrowed(u.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_app_schemes() {
        assert!(Url::from_str("https://example.com").is_err());
        assert!(Url::from_str("irc://irc.libera.chat/#halloy").is_err());
    }

    #[test]
    fn unparsable_theme_route_falls_back_to_unknown() {
        let url = Url::from_str("halloy:///theme").expect("app scheme parses");
        assert!(matches!(url, Url::Unknown(_)));
    }

    #[test]
    fn display_latin_umlaut() {
        // bücher.de — latin with umlaut, safe to show as unicode according to UTS #46 and #39
        let u = url::Url::parse("https://bücher.de/").unwrap();
        assert_eq!(display(&u), "https://bücher.de/");
    }

    #[test]
    fn display_percent_encoded_path() {
        // ASCII host, percent-encoded unicode path. path should be percent-decoded.
        let u = url::Url::parse(
            "https://ja.wikipedia.org/wiki/%E9%87%8D%E9%9F%B3%E3%83%86%E3%83%88",
        )
        .unwrap();
        assert_eq!(display(&u), "https://ja.wikipedia.org/wiki/重音テト");
    }

    #[test]
    fn display_homograph_attack_stays_punycode() {
        // all-cyrillic that looks identical to apple.com.
        // got famous in 2017 and made browsers change their logic!
        let u = url::Url::parse("https://www.аррӏе.com").unwrap();
        assert_eq!(display(&u), "https://www.xn--80ak6aa92e.com/");
    }

    #[test]
    fn display_mixed_script_stays_punycode() {
        // cyrillic 'а' mixed with latin 'pple'
        let u = url::Url::parse("https://аpple.com/").unwrap();
        assert_eq!(display(&u), "https://xn--pple-43d.com/");
    }

    #[test]
    fn display_japanese_domain() {
        // CJK characters have no ASCII-confusable prototypes
        let u = url::Url::parse("https://日本語.jp/").unwrap();
        assert_eq!(display(&u), "https://日本語.jp/");
    }
}
