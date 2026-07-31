use strsim::jaro_winkler;

struct SearchMatch {
    shortcode: String,
    similarity: f64,
}

pub fn matching_shortcodes(query: &str) -> Vec<String> {
    search_matches(query)
        .into_iter()
        .map(|matched| matched.shortcode)
        .collect()
}

pub fn get_by_shortcode(shortcode: &str) -> Option<&'static emojis::Emoji> {
    emojis::get_by_shortcode(shortcode).or_else(|| {
        emojis::iter().find(|e| {
            e.shortcodes().next().is_none()
                && synthetic_shortcode(e) == shortcode
        })
    })
}

/// Newer emoji have no shortcodes in the upstream crate. They get a
/// synthetic shortcode derived from their Unicode name.
fn synthetic_shortcode(emoji: &emojis::Emoji) -> String {
    emoji.name().to_lowercase().replace(' ', "_")
}

fn shortcodes_for(emoji: &emojis::Emoji) -> Vec<String> {
    let real: Vec<_> = emoji.shortcodes().map(str::to_string).collect();
    if real.is_empty() {
        vec![synthetic_shortcode(emoji)]
    } else {
        real
    }
}

fn search_matches(query: &str) -> Vec<SearchMatch> {
    let mut filtered = Vec::new();

    for emoji in emojis::iter() {
        let shortcodes = shortcodes_for(emoji);

        let mut any_matched = false;
        for shortcode in &shortcodes {
            if shortcode.contains(query) {
                filtered.push(SearchMatch {
                    similarity: jaro_winkler(query, shortcode),
                    shortcode: shortcode.clone(),
                });
                any_matched = true;
            }
        }

        // Fall back to name search when no shortcode matched, using the first shortcode
        if !any_matched {
            let name = emoji.name();
            if name.to_lowercase().contains(query) {
                filtered.push(SearchMatch {
                    similarity: jaro_winkler(query, name),
                    shortcode: shortcodes[0].clone(),
                });
            }
        }
    }

    filtered.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    filtered
}
